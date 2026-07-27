//! Runs the real binary against throwaway Python projects.
//! Needs python and pytest on PATH; coverage.py unlocks the batching paths.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A throwaway Python project in the temp dir, deleted when the test ends.
struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let root = std::env::temp_dir().join(format!("angelo-it-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("creating the test project");
        Project { root }
    }

    fn write(self, name: &str, contents: &str) -> Project {
        fs::write(self.root.join(name), contents).expect("writing a test file");
        self
    }

    fn angelo(&self, args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_angelo"))
            .args(args)
            .current_dir(&self.root)
            .output()
            .expect("running angelo");
        Run {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            succeeded: output.status.success(),
        }
    }

    fn has(&self, name: &str) -> bool {
        self.root.join(name).exists()
    }

    fn git(&self, args: &[&str]) -> &Project {
        let status = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("running git");
        assert!(status.success(), "git {args:?} failed");
        self
    }

    /// Rewrite a file and commit it, one step of a branch's history.
    fn commit(&self, name: &str, contents: &str) -> &Project {
        fs::write(self.root.join(name), contents).expect("writing a test file");
        self.git(&["commit", "-aqm", &format!("edit {name}")])
    }

    /// A repo with everything committed, so `--diff HEAD` starts from clean.
    fn committed(self) -> Project {
        self.git(&["init", "-q"])
            .git(&["config", "user.email", "test@angelo.invalid"])
            .git(&["config", "user.name", "angelo tests"])
            .git(&["add", "-A"])
            .git(&["commit", "-qm", "baseline"]);
        self
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Run {
    stdout: String,
    stderr: String,
    succeeded: bool,
}

impl Run {
    fn expect_success(self) -> String {
        assert!(
            self.succeeded,
            "angelo failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout, self.stderr
        );
        self.stdout
    }

    fn expect_failure(self) -> String {
        assert!(
            !self.succeeded,
            "angelo should have failed:\n{}",
            self.stdout
        );
        self.stderr
    }
}

/// `add` is tested and killable, `is_big` is tested but its mutant survives,
/// `unused` is never executed by any test.
const CALCULATOR: &str = "\
def add(a, b):
    return a + b


def is_big(n):
    return n > 10


def unused(x):
    return x * 2
";

const TESTS: &str = "\
from calculator import add, is_big


def test_add():
    assert add(2, 3) == 5


def test_is_big():
    assert is_big(50)
    assert not is_big(1)
";

fn calculator_project(name: &str) -> Project {
    Project::new(name)
        .write("calculator.py", CALCULATOR)
        .write("test_calculator.py", TESTS)
}

/// One mutable token, `not`, and removing it spins forever on `spin(True)`.
/// Any other operator here would add mutants that are not the point.
const SPINNER: &str = "\
def spin(flag):
    while not flag:
        pass
    return flag
";

const SPINNER_TESTS: &str = "\
from spinner import spin


def test_spin():
    assert spin(True)
";

/// A hanging mutant must still be caught once the budget comes from the tests
/// rather than the suite. Too tight a budget would be worse than slowness: a
/// timeout counts as detected, so it would invent kills.
fn spins_until_the_budget_runs_out(name: &str, warm_workers: bool) {
    let project = Project::new(name)
        .write("spinner.py", SPINNER)
        .write("test_spinner.py", SPINNER_TESTS)
        .write(
            "angelo.conf",
            &format!(
                "paths = [\".\"]\ntest_command = \"python -m pytest\"\nworkers = 1\n\
                 batch_size = 1\ntest_selection = true\nwarm_workers = {warm_workers}\n\
                 warm_recycle_after = 50\ntimeout_factor = 2.0\n"
            ),
        );

    let stdout = project.angelo(&["exec"]).expect_success();
    assert!(stdout.contains("enumerated 1 mutants"), "{stdout}");
    assert!(stdout.contains("timeout: 1"), "{stdout}");
    assert!(stdout.contains("score: 100.0%"), "{stdout}");
}

#[test]
fn a_hanging_mutant_times_out_on_the_warm_path() {
    spins_until_the_budget_runs_out("timeout-warm", true);
}

#[test]
fn a_hanging_mutant_times_out_on_the_cold_path() {
    spins_until_the_budget_runs_out("timeout-cold", false);
}

#[test]
fn init_writes_a_config() {
    let project = Project::new("init");
    let stdout = project.angelo(&["init"]).expect_success();

    assert!(stdout.contains("angelo.conf"));
    assert!(project.has("angelo.conf"));
    let config = fs::read_to_string(project.root.join("angelo.conf")).unwrap();
    assert!(config.contains("test_command = \"python -m pytest\""));
    assert!(config.contains("batch_size"));
}

#[test]
fn init_refuses_to_overwrite() {
    let project = Project::new("init-twice");
    project.angelo(&["init"]).expect_success();

    let stderr = project.angelo(&["init"]).expect_failure();
    assert!(stderr.contains("already exists"));
}

#[test]
fn exec_scores_the_suite_and_names_survivors() {
    let project = calculator_project("exec");
    let stdout = project.angelo(&["exec", "--workers", "2"]).expect_success();

    assert!(stdout.contains("enumerated 5 mutants"), "{stdout}");
    assert!(stdout.contains("killed: 1"), "{stdout}");
    assert!(stdout.contains("survived: 4"), "{stdout}");
    assert!(stdout.contains("score: 20.0%"), "{stdout}");
    // The mutant of `unused` is on a line no test runs.
    assert!(stdout.contains("survived without a single run"), "{stdout}");
    assert!(stdout.contains("> -> >="), "{stdout}");
    assert!(project.has(".angelo"));
}

#[test]
fn exec_leaves_the_source_untouched() {
    let project = calculator_project("untouched");
    project.angelo(&["exec"]).expect_success();

    let source = fs::read_to_string(project.root.join("calculator.py")).unwrap();
    assert_eq!(source, CALCULATOR);
}

#[test]
fn exec_resumes_instead_of_rerunning() {
    let project = calculator_project("resume");
    project.angelo(&["exec"]).expect_success();

    let stdout = project.angelo(&["exec"]).expect_success();
    assert!(stdout.contains("resuming"), "{stdout}");
    assert!(stdout.contains("nothing pending"), "{stdout}");
    assert!(!stdout.contains("baseline"), "{stdout}");
    assert!(stdout.contains("killed: 1"), "{stdout}");
}

#[test]
fn init_only_enumerates_without_running() {
    let project = calculator_project("init-only");
    let stdout = project.angelo(&["exec", "--init-only"]).expect_success();

    assert!(stdout.contains("5 mutants pending"), "{stdout}");
    assert!(!stdout.contains("baseline"), "{stdout}");

    let resumed = project.angelo(&["exec"]).expect_success();
    assert!(resumed.contains("killed: 1"), "{resumed}");
}

#[test]
fn exec_refuses_a_red_baseline() {
    let project = Project::new("red-baseline")
        .write("calculator.py", CALCULATOR)
        .write(
            "test_calculator.py",
            "from calculator import add\n\n\ndef test_add():\n    assert add(2, 3) == 6\n",
        );

    let stderr = project.angelo(&["exec"]).expect_failure();
    assert!(stderr.contains("green baseline"), "{stderr}");
    // Exit 1 must read as "your tests fail", not as a setup problem.
    assert!(stderr.contains("Tests are failing"), "{stderr}");
}

/// The whole point of diff mode: an untouched repo has nothing to mutate,
/// however large it is.
#[test]
fn diff_mode_skips_an_unchanged_tree() {
    let project = calculator_project("diff-clean").committed();
    let stdout = project.angelo(&["exec", "--diff"]).expect_success();

    assert!(
        stdout.contains("0 of 5 mutants sit on changed lines"),
        "{stdout}"
    );
    assert!(stdout.contains("nothing changed"), "{stdout}");
    assert!(!stdout.contains("baseline"), "{stdout}");
}

#[test]
fn diff_mode_mutates_only_the_changed_line() {
    let project = calculator_project("diff-changed").committed();
    // Touch only is_big's line, leaving add and unused alone.
    fs::write(
        project.root.join("calculator.py"),
        CALCULATOR.replace("return n > 10", "return n > 11"),
    )
    .unwrap();

    let stdout = project.angelo(&["exec", "--diff", "HEAD"]).expect_success();
    assert!(stdout.contains("enumerated 5 mutants"), "{stdout}");
    assert!(
        stdout.contains("2 of 5 mutants sit on changed lines"),
        "{stdout}"
    );
    // Only is_big's comparison is in scope: add's `+` is never even considered.
    assert!(stdout.contains("> -> >="), "{stdout}");
    assert!(!stdout.contains("+ -> -"), "{stdout}");
    assert!(stdout.contains("survived: 2"), "{stdout}");
}

/// A pull request is many commits, so the unit that matters is the branch
/// against its merge base. A line added in the first commit and deleted in the
/// third has to count for nothing.
#[test]
fn diff_base_mutates_what_the_branch_net_added() {
    let project = calculator_project("diff-base").committed();
    project.git(&["branch", "base"]);
    project.git(&["checkout", "-b", "feature"]);

    let raised = CALCULATOR.replace("return n > 10", "return n > 12");
    let with_triple = format!("{raised}\n\ndef triple(x):\n    return x * 3\n");
    project.commit(
        "calculator.py",
        &format!("{CALCULATOR}\n\ndef triple(x):\n    return x * 3\n"),
    );
    project.commit("calculator.py", &with_triple);
    project.commit("calculator.py", &raised);

    let stdout = project
        .angelo(&["exec", "--diff-base", "base"])
        .expect_success();
    assert!(stdout.contains("diff vs base...HEAD"), "{stdout}");
    // Only is_big's line survives the round trip: triple came and went, and
    // add was never touched.
    assert!(
        stdout.contains("2 of 5 mutants sit on changed lines"),
        "{stdout}"
    );
    assert!(stdout.contains("> -> >="), "{stdout}");
    assert!(!stdout.contains("+ -> -"), "{stdout}");
    assert!(stdout.contains("survived: 2"), "{stdout}");
}

/// The bug that `--diff-base` exists to fix: a two-dot diff sees the base
/// branch's own new commits, backwards, and mutates lines the author never
/// wrote.
#[test]
fn diff_base_ignores_what_the_base_gained() {
    let project = calculator_project("diff-base-moved").committed();
    project.git(&["branch", "base"]);
    project.git(&["checkout", "-b", "feature"]);
    project.commit(
        "calculator.py",
        &CALCULATOR.replace("return n > 10", "return n > 12"),
    );

    project.git(&["checkout", "base"]);
    project.commit(
        "calculator.py",
        &CALCULATOR.replace("return x * 2", "return x * 4"),
    );
    project.git(&["checkout", "feature"]);

    let stdout = project
        .angelo(&["exec", "--init-only", "--diff-base", "base"])
        .expect_success();
    // Two dots would find four: unused's line differs from base too.
    assert!(
        stdout.contains("2 of 5 mutants sit on changed lines"),
        "{stdout}"
    );
    assert!(stdout.contains("2 mutants pending"), "{stdout}");
}

/// Silently preferring one flag over the other is a way to score the wrong
/// lines, so clap refuses the pair outright.
#[test]
fn diff_and_diff_base_cannot_both_be_given() {
    let project = calculator_project("diff-both").committed();
    let stderr = project
        .angelo(&["exec", "--diff", "HEAD", "--diff-base", "base"])
        .expect_failure();

    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

/// Sampling drops mutants from the pool, it does not merely defer them: the
/// database must end up holding exactly the sample.
#[test]
fn sampling_caps_the_pool_and_says_so() {
    let project = calculator_project("sample");
    let stdout = project
        .angelo(&["exec", "--init-only", "--sample", "4"])
        .expect_success();

    assert!(stdout.contains("sampled 4 of"), "{stdout}");
    assert!(stdout.contains("dropped at random"), "{stdout}");
    assert!(stdout.contains("ESTIMATE"), "{stdout}");
    assert!(stdout.contains("4 mutants pending"), "{stdout}");
}

#[test]
fn sampling_above_the_pool_size_changes_nothing() {
    let project = calculator_project("sample-big");
    let stdout = project
        .angelo(&["exec", "--init-only", "--sample", "10000"])
        .expect_success();

    assert!(!stdout.contains("dropped at random"), "{stdout}");
}

#[test]
fn exec_reports_nothing_to_mutate() {
    let project =
        Project::new("empty").write("test_nothing.py", "def test_ok():\n    assert True\n");

    let stderr = project.angelo(&["exec"]).expect_failure();
    assert!(stderr.contains("no Python files found"), "{stderr}");
}
