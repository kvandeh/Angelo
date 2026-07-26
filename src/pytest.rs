use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::mutate::Status;

const JUNIT_FILE: &str = "angelo-junit.xml";

/// How a pytest run ended. Exit codes: 0 passed, 1 failures, 2-4 usage or
/// internal error, 5 nothing collected.
pub enum SuiteResult {
    Finished(i64),
    TimedOut,
}

impl SuiteResult {
    pub fn status(&self) -> Status {
        match self {
            SuiteResult::TimedOut => Status::Timeout,
            SuiteResult::Finished(0) => Status::Survived,
            SuiteResult::Finished(1) => Status::Killed,
            SuiteResult::Finished(_) => Status::Error,
        }
    }

    pub fn exit_code(&self) -> Option<i64> {
        match self {
            SuiteResult::Finished(code) => Some(*code),
            SuiteResult::TimedOut => None,
        }
    }

    /// Why an `error` verdict happened, for the progress line.
    pub fn explain(&self) -> Option<String> {
        match self {
            SuiteResult::Finished(5) => Some(
                "pytest collected no tests, a selected test id may no longer exist".to_string(),
            ),
            SuiteResult::Finished(code) if self.status() == Status::Error => Some(format!(
                "pytest exited with {code}, usually a mutant that broke the syntax or an import"
            )),
            _ => None,
        }
    }
}

/// Which tests one run should execute. Empty ids mean the whole suite, which is
/// always safe, selection can only ever be an optimisation.
pub struct Selection {
    pub test_ids: Vec<String>,
    /// Only for a run judging a single mutant: the first failure settles it, so
    /// there is nothing to learn from the rest of the suite. A batch must see
    /// every failure to attribute them, so it never stops early.
    pub stop_at_first_failure: bool,
}

impl Selection {
    pub fn whole_suite() -> Selection {
        Selection {
            test_ids: Vec::new(),
            stop_at_first_failure: false,
        }
    }

    fn apply(&self, command: &mut Command) {
        if self.stop_at_first_failure {
            command.arg("-x");
        }
        command.args(&self.test_ids);
    }
}

pub struct TestCase {
    /// Dotted module path, plus class names for methods.
    pub classname: String,
    pub name: String,
    pub failed: bool,
}

impl TestCase {
    /// How coverage.py names this test under `dynamic_context = test_function`.
    /// Parameters are dropped: coverage records one context per function.
    pub fn context_name(&self) -> String {
        let base = self.name.split('[').next().unwrap_or(&self.name);
        format!("{}.{}", self.classname, base)
    }

    /// The pytest node id, e.g. `tests/test_x.py::TestY::test_z`. The dotted
    /// classname does not say where the module ends and classes begin, so the
    /// longest prefix that exists as a file on disk wins.
    pub fn node_id(&self, root: &Path) -> Option<String> {
        let parts: Vec<&str> = self.classname.split('.').collect();
        for split in (1..=parts.len()).rev() {
            let module = format!("{}.py", parts[..split].join("/"));
            if !root.join(&module).is_file() {
                continue;
            }
            let mut id = module;
            for class in &parts[split..] {
                id.push_str("::");
                id.push_str(class);
            }
            id.push_str("::");
            id.push_str(self.name.split('[').next().unwrap_or(&self.name));
            return Some(id);
        }
        None
    }
}

/// Run the untouched suite once: proves it is green, times a normal run, and
/// (via the junit report) lists every test for later node-id resolution.
pub fn run_baseline(
    project_root: &Path,
    test_command: &[String],
    junit_path: &Path,
) -> Result<(Duration, Vec<TestCase>)> {
    let started = Instant::now();
    let output = build_command(test_command, project_root)?
        .arg(format!("--junit-xml={}", junit_path.display()))
        .output()
        .context("running the baseline suite (is python on PATH?)")?;
    if !output.status.success() {
        let code = output.status.code().map(i64::from).unwrap_or(-1);
        bail!(
            "angelo needs a green baseline, but pytest exited {code}.\n{}\n\n{}",
            diagnose_baseline(code),
            tail(&String::from_utf8_lossy(&output.stdout), 40)
        );
    }
    let elapsed = started.elapsed();
    let xml = fs::read_to_string(junit_path)
        .with_context(|| format!("reading {}", junit_path.display()))?;
    Ok((elapsed, parse_testcases(&xml)?))
}

/// Exit 1 means the suite is genuinely red. Everything else means pytest never
/// got as far as judging the code, which is usually a missing dependency.
fn diagnose_baseline(code: i64) -> &'static str {
    match code {
        1 => {
            "Tests are failing. Fix them first, angelo can only tell you what a passing suite misses."
        }
        2 => "pytest was interrupted.",
        3 => {
            "pytest hit an internal error, usually a plugin or config problem: an option in \
              pyproject.toml/pytest.ini that belongs to a plugin you have not installed."
        }
        4 => "pytest usage error, check test_command in angelo.conf.",
        5 => "pytest collected no tests, check test_command and its path in angelo.conf.",
        _ => "pytest could not run the suite.",
    }
}

/// Collection failures bury the useful line under hundreds of tracebacks.
fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

pub fn run(
    test_command: &[String],
    cwd: &Path,
    timeout: Duration,
    selection: &Selection,
) -> Result<SuiteResult> {
    let started = Instant::now();
    let mut command = build_command(test_command, cwd)?;
    command
        .arg(format!("--junit-xml={JUNIT_FILE}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    selection.apply(&mut command);

    let mut child = command.spawn().context("spawning pytest")?;
    loop {
        if let Some(status) = child.try_wait().context("polling pytest")? {
            // code() is None when a signal killed pytest; -1 lands that in `error`.
            return Ok(SuiteResult::Finished(
                status.code().map(i64::from).unwrap_or(-1),
            ));
        }
        if started.elapsed() > timeout {
            child.kill().context("killing timed-out pytest")?;
            child.wait().context("reaping timed-out pytest")?;
            return Ok(SuiteResult::TimedOut);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn failed_tests(project: &Path) -> Result<Vec<String>> {
    let path = project.join(JUNIT_FILE);
    let xml = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(parse_testcases(&xml)?
        .into_iter()
        .filter(|test| test.failed)
        .map(|test| format!("{}::{}", test.classname, test.name))
        .collect())
}

fn parse_testcases(xml: &str) -> Result<Vec<TestCase>> {
    let document = roxmltree::Document::parse(xml).context("parsing the junit report")?;
    let mut tests = Vec::new();
    for node in document
        .descendants()
        .filter(|n| n.has_tag_name("testcase"))
    {
        tests.push(TestCase {
            classname: node.attribute("classname").unwrap_or("").to_string(),
            name: node.attribute("name").unwrap_or("?").to_string(),
            failed: node
                .children()
                .any(|child| child.has_tag_name("failure") || child.has_tag_name("error")),
        });
    }
    Ok(tests)
}

fn build_command(test_command: &[String], cwd: &Path) -> Result<Command> {
    let (program, args) = test_command
        .split_first()
        .context("test_command is empty")?;
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    // The copy must win over a `pip install -e` of the real project: PYTHONPATH
    // entries come before site-packages. src/ covers src-layout projects.
    let python_path = std::env::join_paths([cwd.to_path_buf(), cwd.join("src")])
        .context("building PYTHONPATH")?;
    command.env("PYTHONPATH", python_path);
    // A .pyc is reused when the source's (mtime_seconds, size) still match. A
    // same-size splice (`+` -> `-`) written in the same second as the previous
    // one therefore runs the OLD bytecode, and the mutant survives for free.
    // Worker copies never carry a __pycache__ in, so never writing one keeps
    // every run honest. Costs a recompile per run; buys correct verdicts.
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    Ok(command)
}

/// Where the baseline writes its report, under .angelo, never the user's tree.
pub fn baseline_junit_path(angelo_dir: &Path) -> PathBuf {
    angelo_dir.join("baseline-junit.xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<testsuites><testsuite>
        <testcase classname="test_calc" name="test_add"><failure message="boom"/></testcase>
        <testcase classname="test_calc" name="test_crash"><error message="import"/></testcase>
        <testcase classname="test_calc" name="test_fine"/>
    </testsuite></testsuites>"#;

    #[test]
    fn exit_codes_map_to_statuses() {
        assert_eq!(SuiteResult::Finished(0).status(), Status::Survived);
        assert_eq!(SuiteResult::Finished(1).status(), Status::Killed);
        assert_eq!(SuiteResult::Finished(2).status(), Status::Error);
        assert_eq!(SuiteResult::Finished(5).status(), Status::Error);
        assert_eq!(SuiteResult::TimedOut.status(), Status::Timeout);
    }

    #[test]
    fn only_error_exits_are_explained() {
        assert!(SuiteResult::Finished(0).explain().is_none());
        assert!(SuiteResult::Finished(1).explain().is_none());
        assert!(
            SuiteResult::Finished(5)
                .explain()
                .unwrap()
                .contains("no tests")
        );
        assert!(
            SuiteResult::Finished(2)
                .explain()
                .unwrap()
                .contains("exited with 2")
        );
    }

    #[test]
    fn reports_failed_and_errored_tests() {
        let failed: Vec<_> = parse_testcases(SAMPLE)
            .unwrap()
            .into_iter()
            .filter(|t| t.failed)
            .map(|t| t.name)
            .collect();
        assert_eq!(failed, ["test_add", "test_crash"]);
    }

    #[test]
    fn a_red_baseline_is_diagnosed_by_exit_code() {
        assert!(diagnose_baseline(1).contains("Tests are failing"));
        assert!(diagnose_baseline(3).contains("plugin"));
        assert!(diagnose_baseline(5).contains("collected no tests"));
    }

    #[test]
    fn tail_keeps_the_end_of_a_long_log() {
        let log = (1..=100)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let kept = tail(&log, 3);
        assert_eq!(kept, "98\n99\n100");
        assert_eq!(tail("one\ntwo", 10), "one\ntwo");
    }

    #[test]
    fn context_names_drop_parameters() {
        let test = TestCase {
            classname: "pkg.test_mod".to_string(),
            name: "test_x[3-abc]".to_string(),
            failed: false,
        };
        assert_eq!(test.context_name(), "pkg.test_mod.test_x");
    }

    /// Without this, a same-size splice in the same second reuses stale
    /// bytecode and the mutant survives for free.
    #[test]
    fn bytecode_caching_is_disabled() {
        let command = build_command(&["python".to_string()], Path::new(".")).unwrap();
        let names: Vec<_> = command
            .get_envs()
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"PYTHONDONTWRITEBYTECODE".to_string()));
    }

    #[test]
    fn selection_only_adds_x_for_single_mutants() {
        let mut command = Command::new("pytest");
        Selection {
            test_ids: vec!["a.py::test_x".to_string()],
            stop_at_first_failure: true,
        }
        .apply(&mut command);
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-x", "a.py::test_x"]);

        let mut batch = Command::new("pytest");
        Selection {
            test_ids: vec!["a.py::test_x".to_string()],
            stop_at_first_failure: false,
        }
        .apply(&mut batch);
        let batch_args: Vec<_> = batch
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(batch_args, ["a.py::test_x"]);
    }

    #[test]
    fn whole_suite_selection_adds_nothing() {
        let mut command = Command::new("pytest");
        Selection::whole_suite().apply(&mut command);
        assert_eq!(command.get_args().count(), 0);
    }
}
