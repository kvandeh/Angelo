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

    /// `name` may be nested, so a test can build the directory an exclude
    /// pattern is meant to prune.
    fn write(self, name: &str, contents: &str) -> Project {
        let path = self.root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("creating a test directory");
        }
        fs::write(path, contents).expect("writing a test file");
        self
    }

    /// The child's log level must not depend on where the suite is run from.
    /// `CI` and `RUST_LOG` both outrank the default, and most tests here read
    /// the commentary `log::info!` puts on stderr — so under GitHub Actions,
    /// which exports `CI=true`, the default drops to `warn` and takes that
    /// commentary away. `choose` covers those precedences in a unit test.
    fn angelo(&self, args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_angelo"))
            .args(args)
            .current_dir(&self.root)
            .env_remove("CI")
            .env_remove("RUST_LOG")
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
    /// Both streams, joined. Angelo puts its report on stdout and its running
    /// commentary on stderr, and most of these tests ask whether a run *said*
    /// something rather than which stream carried it. The two that do care
    /// about the split say so: `expect_report` and `expect_failure_stdout`.
    fn expect_success(self) -> String {
        assert!(
            self.succeeded,
            "angelo failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout, self.stderr
        );
        format!("{}\n{}", self.stderr, self.stdout)
    }

    /// stdout alone, which is the report and nothing else.
    fn expect_report(self) -> String {
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

    /// A threshold failure is a verdict, not a crash: it exits non-zero with
    /// its line on stdout, where the rest of the report is.
    fn expect_failure_stdout(self) -> String {
        assert!(
            !self.succeeded,
            "angelo should have failed:\n{}",
            self.stdout
        );
        self.stdout
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

/// `add` is covered by a test that already fails, `double` by one that passes.
/// Both of `double`'s mutants die, so a red suite still produces a real score.
const HALF_RED: &str = "\
def add(a, b):
    return a + b


def double(x):
    return x * 2
";

const HALF_RED_TESTS: &str = "\
from calculator import add, double


def test_add():
    assert add(2, 3) == 6


def test_double():
    assert double(3) == 6
";

fn half_red_project(name: &str) -> Project {
    Project::new(name)
        .write("calculator.py", HALF_RED)
        .write("test_calculator.py", HALF_RED_TESTS)
}

/// A handful of known-red tests should not block the rest of the codebase. The
/// mutants those tests cover are refused, everything else is scored as usual.
#[test]
fn a_red_baseline_scores_what_the_failing_tests_do_not_cover() {
    let project = half_red_project("red-baseline");
    let output = project.angelo(&["exec"]).expect_success();

    assert!(output.contains("baseline RED"), "{output}");
    assert!(output.contains("1 tests were already failing"), "{output}");
    // add's `+` is covered by the red test alone, so no run could judge it.
    assert!(output.contains("1 mutants set untestable"), "{output}");
    assert!(output.contains("untestable: 1"), "{output}");
    // double's two mutants are covered by a green test and die there.
    assert!(output.contains("killed: 2"), "{output}");
    assert!(output.contains("score: 100.0%"), "{output}");
}

/// Without per-test coverage every run executes the whole red suite, exits 1,
/// and scores every mutant killed. Inventing a perfect score is worse than
/// refusing to run, so that combination still stops.
#[test]
fn a_red_baseline_without_test_selection_still_stops() {
    let project = half_red_project("red-baseline-unselectable").write(
        "angelo.conf",
        "paths = [\".\"]\ntest_command = \"python -m pytest\"\ntest_selection = false\n",
    );

    let stderr = project.angelo(&["exec"]).expect_failure();
    assert!(stderr.contains("baseline suite is red"), "{stderr}");
    assert!(stderr.contains("test_selection = true"), "{stderr}");
}

/// Exit 2 to 5 mean pytest never judged the code at all, so there is no
/// duration to measure and no report to read. Those still stop.
#[test]
fn a_baseline_that_never_ran_still_stops() {
    let project = calculator_project("no-tests-collected").write(
        "angelo.conf",
        "paths = [\".\"]\ntest_command = \"python -m pytest nothing_here.py\"\n",
    );

    let stderr = project.angelo(&["exec"]).expect_failure();
    assert!(
        stderr.contains("could not run the baseline suite"),
        "{stderr}"
    );
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
fn exclude_prunes_a_directory_and_reports_the_count() {
    let project = calculator_project("exclude")
        .write("generated/client.py", CALCULATOR)
        .write(
            "angelo.conf",
            "paths = [\".\"]\nexclude = [\"**/generated/**\", \"typo/*.py\"]\n",
        );

    let stdout = project.angelo(&["exec", "--init-only"]).expect_success();

    assert!(stdout.contains("across 1 files"), "{stdout}");
    assert!(
        stdout.contains("1 path excluded by angelo.conf"),
        "{stdout}"
    );
    // A typo in an exclude is invisible otherwise: it silently raises the score.
    assert!(stdout.contains("\"typo/*.py\" matched nothing"), "{stdout}");
}

#[test]
fn excluding_everything_blames_exclude_and_not_paths() {
    let project = calculator_project("exclude-all")
        .write("angelo.conf", "paths = [\".\"]\nexclude = [\"*.py\"]\n");

    let stderr = project.angelo(&["exec", "--init-only"]).expect_failure();
    assert!(stderr.contains("was excluded by"), "{stderr}");
}

#[test]
fn no_exclude_leaves_the_message_alone() {
    let project = calculator_project("exclude-none").write("generated/client.py", CALCULATOR);

    let stdout = project.angelo(&["exec", "--init-only"]).expect_success();

    assert!(stdout.contains("across 2 files"), "{stdout}");
    assert!(!stdout.contains("excluded"), "{stdout}");
}

#[test]
fn sampling_above_the_pool_size_changes_nothing() {
    let project = calculator_project("sample-big");
    let stdout = project
        .angelo(&["exec", "--init-only", "--sample", "10000"])
        .expect_success();

    assert!(!stdout.contains("dropped at random"), "{stdout}");
}

/// The calculator always leaves `unused` alive and always kills `add`, so its
/// score sits strictly between 1% and 100% whatever the machine's timing does
/// to the mutants in between. Bracketing it that way tests the gate rather than
/// the score. Every case after the first re-runs the same finished database,
/// which resumes to "nothing pending" and costs no pytest run.
#[test]
fn fail_under_gates_the_score() {
    let project = calculator_project("fail-under");

    let below = project
        .angelo(&["exec", "--fail-under", "100"])
        .expect_failure_stdout();
    assert!(below.contains("is below --fail-under 100.0%"), "{below}");

    let above = project
        .angelo(&["exec", "--fail-under", "1"])
        .expect_success();
    assert!(above.contains("score:"), "{above}");
    assert!(!above.contains("--fail-under"), "{above}");

    // No threshold: a terrible score is still a successful run.
    let ungated = project.angelo(&["exec"]).expect_success();
    assert!(ungated.contains("score:"), "{ungated}");

    // The config key sets the same threshold for every run.
    fs::write(
        project.root.join("angelo.conf"),
        "paths = [\".\"]\ntest_command = \"python -m pytest\"\nfail_under = 100.0\n",
    )
    .unwrap();
    let configured = project.angelo(&["exec"]).expect_failure_stdout();
    assert!(
        configured.contains("is below --fail-under 100.0%"),
        "{configured}"
    );

    // And the flag beats the file.
    project
        .angelo(&["exec", "--fail-under", "1"])
        .expect_success();
}

/// Mutating `break` to `return` at module level is a syntax error, so this
/// project's one mutant can only come back as `error`. That leaves no score at
/// all, which is the signature of a broken test command and must never satisfy
/// a threshold.
const LOOPER: &str = "\
def values():
    return [None]


for value in values():
    break
";

#[test]
fn fail_under_rejects_a_run_it_could_not_score() {
    let project = Project::new("fail-under-unmeasured")
        .write("looper.py", LOOPER)
        .write(
            "test_looper.py",
            "from looper import values\n\n\ndef test_values():\n    assert values() == [None]\n",
        );

    let stdout = project
        .angelo(&["exec", "--fail-under", "50"])
        .expect_failure_stdout();

    assert!(stdout.contains("error: 1"), "{stdout}");
    assert!(!stdout.contains("score:"), "{stdout}");
    assert!(stdout.contains("every mutant errored"), "{stdout}");
}

/// A docs-only pull request enumerates nothing. Zero mutants is zero
/// information, so it neither prints a score nor fails the threshold.
#[test]
fn fail_under_passes_an_empty_pool_without_a_score() {
    let project = calculator_project("fail-under-empty").committed();
    let stdout = project
        .angelo(&["exec", "--diff", "--fail-under", "90"])
        .expect_success();

    assert!(stdout.contains("no mutants in scope"), "{stdout}");
    assert!(!stdout.contains("score:"), "{stdout}");
}

#[test]
fn exec_reports_nothing_to_mutate() {
    let project =
        Project::new("empty").write("test_nothing.py", "def test_ok():\n    assert True\n");

    let stderr = project.angelo(&["exec"]).expect_failure();
    assert!(stderr.contains("no Python files found"), "{stderr}");
}

/// The property CI depends on: the report is the program's *output*, not
/// commentary about it, so silencing the commentary must not silence the score.
/// `verdict-matrix.sh` and `bench-repo.sh` both grep these lines out of a run.
#[test]
fn the_report_still_prints_at_the_quietest_verbosity() {
    let project = calculator_project("verbosity");
    let run = project.angelo(&["exec", "--workers", "2", "--verbosity", "error"]);
    let stderr = run.stderr.clone();
    let stdout = run.expect_report();

    assert!(stdout.contains("=== mutation report ==="), "{stdout}");
    assert!(stdout.contains("score:"), "{stdout}");
    assert!(stdout.contains("survivors"), "{stdout}");
    // The commentary is what went away.
    assert!(!stderr.contains("INFO"), "{stderr}");
    assert!(!stderr.contains("enumerated"), "{stderr}");
}

/// A bar is drawn with control characters, and a redirected run must not carry
/// any. `indicatif` hides itself off a TTY, and this is what proves it.
#[test]
fn a_redirected_run_writes_no_control_characters() {
    let project = calculator_project("no-escapes");
    let run = project.angelo(&["exec", "--workers", "2"]);
    for (stream, text) in [("stdout", &run.stdout), ("stderr", &run.stderr)] {
        assert!(!text.contains('\r'), "{stream} carried a carriage return");
        assert!(!text.contains('\u{1b}'), "{stream} carried an ANSI escape");
    }
}

/// Both report files, and the one thing that must be true of them: they agree
/// with the terminal, because all three ask the same `Summary` for the score.
#[test]
fn the_report_files_agree_with_the_terminal() {
    let project = calculator_project("report-files");
    let stdout = project
        .angelo(&[
            "exec",
            "--workers",
            "2",
            "--report",
            "run.json",
            "--html-report",
            "run.html",
        ])
        .expect_report();
    assert!(stdout.contains("score: 20.0%"), "{stdout}");

    let json = fs::read_to_string(project.root.join("run.json")).expect("run.json");
    assert!(json.contains("\"schemaVersion\": \"2.0\""), "{json}");
    assert!(
        json.contains("\"framework\": { \"name\": \"Angelo\""),
        "{json}"
    );
    // 1 killed and 4 survived, and the schema splits those four: the two on
    // `unused` are on a line no test runs at all, which is a different finding
    // from the two a test ran and failed to notice.
    assert_eq!(json.matches("\"status\": \"Killed\"").count(), 1, "{json}");
    assert_eq!(
        json.matches("\"status\": \"Survived\"").count(),
        2,
        "{json}"
    );
    assert_eq!(
        json.matches("\"status\": \"NoCoverage\"").count(),
        2,
        "{json}"
    );
    // A root would be stripped downstream by literal match, and a Windows one
    // against forward-slashed keys strips nothing at all.
    assert!(!json.contains("projectRoot"), "{json}");
    assert!(json.contains("\"calculator.py\""), "{json}");

    let html = fs::read_to_string(project.root.join("run.html")).expect("run.html");
    assert!(html.contains("20.0%"), "the html score must match stdout");
    assert!(!html.contains("{{"), "a placeholder went unfilled");
    assert!(
        !html.contains("<script"),
        "the report must not carry script"
    );
}

/// A report is output and never a verdict. Writing one must not move the score,
/// which is the same claim `verdict-matrix.sh` makes about the speed features.
#[test]
fn writing_a_report_does_not_change_the_verdict() {
    let plain = calculator_project("report-neutral-off")
        .angelo(&["exec", "--workers", "2"])
        .expect_report();
    let reported = calculator_project("report-neutral-on")
        .angelo(&["exec", "--workers", "2", "--report", "run.json"])
        .expect_report();

    let counts = |text: &str| {
        text.lines()
            .filter(|line| line.trim_start().starts_with(|c: char| c.is_alphabetic()))
            .filter(|line| line.contains(':'))
            .map(str::trim)
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(counts(&plain), counts(&reported));
}

/// `--init-only` never runs a mutant, and a report that needed a run to exist
/// would be a report nobody could produce from a resumed database.
#[test]
fn a_report_is_written_without_running_anything() {
    let project = calculator_project("report-init-only");
    project
        .angelo(&["exec", "--init-only", "--report", "run.json"])
        .expect_success();

    let json = fs::read_to_string(project.root.join("run.json")).expect("run.json");
    assert_eq!(json.matches("\"status\": \"Pending\"").count(), 5, "{json}");
}
