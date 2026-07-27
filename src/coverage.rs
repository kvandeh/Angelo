use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::db::{self, CoverageRow};
use crate::mutate::{Mutant, Status};
use crate::pytest::{self, Selection, TestCase};

/// Which test cases executed which lines, from one coverage.py baseline run.
pub struct Coverage {
    /// file (forward slashes) -> line -> ids of test contexts that ran it
    lines: HashMap<String, HashMap<u32, HashSet<i64>>>,
    /// normalized "module.function" -> context id
    tests: HashMap<String, i64>,
    /// lines only executed outside any test (import time)
    import_lines: HashMap<String, HashSet<u32>>,
    /// context id -> pytest node id, empty until the baseline resolves them
    node_ids: HashMap<i64, String>,
    /// context id -> what that node id took in the baseline run
    durations: HashMap<i64, Duration>,
}

pub enum TestCoverage {
    /// Executed by these test contexts.
    Tested(HashSet<i64>),
    /// Executed only at import time, affects every test, cannot batch.
    ImportOnly,
    /// Never executed, no test can kill it.
    Untested,
}

/// Run the baseline suite. With coverage.py available and the default pytest
/// command, the same single run also yields the per-test coverage map.
pub fn baseline(
    project_root: &Path,
    test_command: &[String],
    angelo_dir: &Path,
) -> Result<(Duration, Option<Coverage>)> {
    let junit = pytest::baseline_junit_path(angelo_dir);
    let rcfile = angelo_dir.join("coveragerc");
    let data_file = angelo_dir.join("coverage.db");

    let wrapped = coverage_command(test_command, &rcfile);
    let Some(wrapped) = wrapped.filter(|_| coverage_is_installed()) else {
        let (duration, _) = pytest::run_baseline(project_root, test_command, &junit)?;
        return Ok((duration, None));
    };

    fs::write(
        &rcfile,
        format!(
            "[run]\ndata_file = {}\ndynamic_context = test_function\nrelative_files = true\n",
            data_file.display()
        ),
    )
    .with_context(|| format!("writing {}", rcfile.display()))?;

    let (duration, testcases) = pytest::run_baseline(project_root, &wrapped, &junit)?;
    let rows = db::read_coverage_rows(&data_file)?;
    let mut coverage = Coverage::build(rows);
    coverage.resolve_node_ids(&testcases, project_root);
    Ok((duration, Some(coverage)))
}

impl Coverage {
    pub(crate) fn build(rows: Vec<CoverageRow>) -> Coverage {
        let mut coverage = Coverage {
            lines: HashMap::new(),
            tests: HashMap::new(),
            import_lines: HashMap::new(),
            node_ids: HashMap::new(),
            durations: HashMap::new(),
        };
        for row in rows {
            for line in lines_in(&row.numbits) {
                if row.context.is_empty() {
                    coverage
                        .import_lines
                        .entry(row.file.clone())
                        .or_default()
                        .insert(line);
                    continue;
                }
                coverage
                    .lines
                    .entry(row.file.clone())
                    .or_default()
                    .entry(line)
                    .or_default()
                    .insert(row.context_id);
            }
            if !row.context.is_empty() {
                coverage.tests.insert(row.context.clone(), row.context_id);
            }
        }
        coverage
    }

    /// Pair each coverage context with the pytest node id that can run it, and
    /// with what that node id cost. Contexts that cannot be resolved simply
    /// stay out, which costs a full-suite run rather than a wrong one.
    pub(crate) fn resolve_node_ids(&mut self, testcases: &[TestCase], root: &Path) {
        for test in testcases {
            let Some(context_id) = self.tests.get(&test.context_name()) else {
                continue;
            };
            let Some(node_id) = test.node_id(root) else {
                continue;
            };
            self.node_ids.insert(*context_id, node_id);
            // Parametrised cases collapse onto one context and one node id, and
            // running that id runs every parameter, so their times add up.
            *self.durations.entry(*context_id).or_default() += test.duration;
        }
    }

    pub fn classify(&self, mutant: &Mutant) -> TestCoverage {
        let file = mutant.coverage_file();
        let covering = self
            .lines
            .get(&file)
            .and_then(|lines| lines.get(&mutant.line));
        if let Some(tests) = covering {
            return TestCoverage::Tested(tests.clone());
        }
        let ran_at_import = self
            .import_lines
            .get(&file)
            .is_some_and(|lines| lines.contains(&mutant.line));
        if ran_at_import {
            return TestCoverage::ImportOnly;
        }
        TestCoverage::Untested
    }

    /// The tests that can possibly kill this batch, and what they cost in the
    /// baseline. Falls back to the whole suite whenever a member's tests cannot
    /// be named exactly, running too many tests is slow, running too few would
    /// invent survivors.
    pub fn select(&self, mutants: &[&Mutant]) -> Selection {
        let mut contexts = HashSet::new();
        for mutant in mutants {
            let TestCoverage::Tested(covering) = self.classify(mutant) else {
                return Selection::whole_suite();
            };
            contexts.extend(covering);
        }

        let mut test_ids = HashSet::new();
        // Times come from a run wrapped in coverage.py, so they are longer than
        // an unwrapped run's. Erring long is the safe direction for a budget.
        let mut baseline_time = Duration::ZERO;
        for context_id in contexts {
            let Some(node_id) = self.node_ids.get(&context_id) else {
                return Selection::whole_suite();
            };
            if test_ids.insert(node_id.clone()) {
                baseline_time += self.durations.get(&context_id).copied().unwrap_or_default();
            }
        }
        if test_ids.is_empty() {
            return Selection::whole_suite();
        }
        let mut test_ids: Vec<String> = test_ids.into_iter().collect();
        test_ids.sort();
        Selection {
            test_ids,
            stop_at_first_failure: mutants.len() == 1,
            baseline_time: Some(baseline_time),
        }
    }

    /// Map a red batch run to per-mutant verdicts: a failed test kills the
    /// batch member it covers. None = a failure nobody explains, caller must
    /// split the batch instead.
    pub fn attribute(
        &self,
        mutants: &[&Mutant],
        failed_tests: &[String],
    ) -> Option<Vec<(i64, Status)>> {
        let mut verdicts: Vec<(i64, Status)> =
            mutants.iter().map(|m| (m.id, Status::Survived)).collect();
        for name in failed_tests {
            let context_id = self.tests.get(&normalize(name))?;
            let mut explained = false;
            for (index, mutant) in mutants.iter().enumerate() {
                let TestCoverage::Tested(covering) = self.classify(mutant) else {
                    continue;
                };
                if covering.contains(context_id) {
                    verdicts[index].1 = Status::Killed;
                    explained = true;
                }
            }
            if !explained {
                return None;
            }
        }
        Some(verdicts)
    }
}

/// Both spellings of a test id become the coverage context "module.test_name":
/// junit gives "pkg.test_mod::test_x", the warm worker gives the pytest node id
/// "pkg/test_mod.py::test_x". Parameters are dropped either way.
fn normalize(test_id: &str) -> String {
    let without_params = test_id.split('[').next().unwrap_or(test_id);
    let mut parts = without_params.split("::");
    let Some(module) = parts.next() else {
        return without_params.to_string();
    };
    let module = module
        .strip_suffix(".py")
        .unwrap_or(module)
        .replace(['/', '\\'], ".");
    std::iter::once(module)
        .chain(parts.map(String::from))
        .collect::<Vec<_>>()
        .join(".")
}

/// coverage.py stores executed lines as a bitmap: bit N set = line N ran.
fn lines_in(numbits: &[u8]) -> Vec<u32> {
    let mut lines = Vec::new();
    for (byte_index, byte) in numbits.iter().enumerate() {
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                lines.push((byte_index * 8 + bit) as u32);
            }
        }
    }
    lines
}

fn coverage_command(test_command: &[String], rcfile: &Path) -> Option<Vec<String>> {
    let [python, dash_m, pytest, rest @ ..] = test_command else {
        return None;
    };
    if !(python == "python" && dash_m == "-m" && pytest == "pytest") {
        return None;
    }
    let mut command: Vec<String> = ["python", "-m", "coverage", "run", "--rcfile"]
        .map(String::from)
        .to_vec();
    command.push(rcfile.display().to_string());
    command.extend(["-m".to_string(), "pytest".to_string()]);
    command.extend(rest.iter().cloned());
    Some(command)
}

fn coverage_is_installed() -> bool {
    Command::new("python")
        .args(["-m", "coverage", "--version"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mutant(id: i64, file: &str, line: u32) -> Mutant {
        Mutant {
            id,
            file: PathBuf::from(file),
            line,
            byte_start: 0,
            byte_end: 1,
            original: "+".to_string(),
            replacement: "-".to_string(),
        }
    }

    fn coverage() -> Coverage {
        Coverage::build(vec![
            CoverageRow {
                file: "calc.py".to_string(),
                context_id: 1,
                context: "test_calc.test_add".to_string(),
                numbits: vec![0b0000_0100], // line 2
            },
            CoverageRow {
                file: "calc.py".to_string(),
                context_id: 2,
                context: "test_calc.test_both".to_string(),
                numbits: vec![0b0001_0100], // lines 2 and 4
            },
            CoverageRow {
                file: "calc.py".to_string(),
                context_id: 0,
                context: String::new(),
                numbits: vec![0b0100_0000], // line 6, import time
            },
        ])
    }

    /// Pretends every context resolved, without touching the filesystem.
    /// Context N takes N hundred milliseconds, so a sum names its members.
    fn with_node_ids(mut coverage: Coverage) -> Coverage {
        for (name, id) in coverage.tests.clone() {
            let function = name.rsplit('.').next().unwrap_or(&name).to_string();
            coverage
                .node_ids
                .insert(id, format!("test_calc.py::{function}"));
            coverage
                .durations
                .insert(id, Duration::from_millis(100 * id as u64));
        }
        coverage
    }

    #[test]
    fn decodes_numbits() {
        assert_eq!(lines_in(&[0b0000_0101, 0b0000_0001]), [0, 2, 8]);
    }

    #[test]
    fn classifies_tested_import_only_and_untested() {
        let coverage = coverage();
        assert!(matches!(
            coverage.classify(&mutant(1, "calc.py", 2)),
            TestCoverage::Tested(tests) if tests.len() == 2
        ));
        assert!(matches!(
            coverage.classify(&mutant(2, "calc.py", 6)),
            TestCoverage::ImportOnly
        ));
        assert!(matches!(
            coverage.classify(&mutant(3, "calc.py", 99)),
            TestCoverage::Untested
        ));
    }

    #[test]
    fn selects_only_the_covering_tests() {
        let coverage = with_node_ids(coverage());
        let only_add = mutant(1, "calc.py", 4);
        let selection = coverage.select(&[&only_add]);
        assert_eq!(selection.test_ids, ["test_calc.py::test_both"]);
        assert!(selection.stop_at_first_failure);
    }

    #[test]
    fn a_batch_selects_the_union_and_never_stops_early() {
        let coverage = with_node_ids(coverage());
        let shared = mutant(1, "calc.py", 2);
        let selection = coverage.select(&[&shared]);
        assert_eq!(
            selection.test_ids,
            ["test_calc.py::test_add", "test_calc.py::test_both"]
        );

        let second = mutant(2, "calc.py", 4);
        let batch = coverage.select(&[&shared, &second]);
        assert_eq!(batch.test_ids.len(), 2);
        assert!(!batch.stop_at_first_failure);
    }

    #[test]
    fn unresolvable_or_import_time_mutants_fall_back_to_everything() {
        let resolved = with_node_ids(coverage());
        let import_time = mutant(1, "calc.py", 6);
        assert!(resolved.select(&[&import_time]).test_ids.is_empty());

        // No node ids resolved at all.
        let unresolved = coverage();
        let tested = mutant(2, "calc.py", 2);
        assert!(unresolved.select(&[&tested]).test_ids.is_empty());
    }

    /// The budget for a run comes from this number, so a selection that names
    /// its tests must also cost them.
    #[test]
    fn a_selection_carries_what_its_tests_cost() {
        let coverage = with_node_ids(coverage());
        let only_both = mutant(1, "calc.py", 4);
        assert_eq!(
            coverage.select(&[&only_both]).baseline_time,
            Some(Duration::from_millis(200))
        );

        // A batch is charged for the union, matching the tests it selects.
        let shared = mutant(2, "calc.py", 2);
        assert_eq!(
            coverage.select(&[&only_both, &shared]).baseline_time,
            Some(Duration::from_millis(300))
        );
    }

    /// An uncosted run has to keep paying the whole suite's budget, otherwise
    /// the fallback silently becomes the tightest path instead of the safest.
    #[test]
    fn a_whole_suite_fallback_refuses_to_be_costed() {
        let coverage = with_node_ids(coverage());
        let import_time = mutant(1, "calc.py", 6);
        assert!(coverage.select(&[&import_time]).baseline_time.is_none());
    }

    /// Parameters collapse onto one context and one node id, and running that
    /// id runs every parameter, so the budget has to cover all of them.
    #[test]
    fn parametrised_cases_add_up_to_one_node_id() {
        let mut coverage = Coverage::build(vec![CoverageRow {
            file: "calc.py".to_string(),
            context_id: 7,
            context: "src.runner.worker.test_x".to_string(),
            numbits: vec![0b0000_0100],
        }]);
        // Resolution walks real paths, so this borrows a .py file the repo has.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cases: Vec<TestCase> = ["test_x[1]", "test_x[2]"]
            .iter()
            .map(|name| TestCase {
                classname: "src.runner.worker".to_string(),
                name: name.to_string(),
                failed: false,
                duration: Duration::from_millis(120),
            })
            .collect();

        coverage.resolve_node_ids(&cases, root);
        assert_eq!(coverage.node_ids[&7], "src/runner/worker.py::test_x");
        assert_eq!(coverage.durations[&7], Duration::from_millis(240));
    }

    #[test]
    fn attributes_a_failed_test_to_the_covered_mutant() {
        let coverage = coverage();
        let line2 = mutant(1, "calc.py", 2);
        let line4 = mutant(2, "calc.py", 4);
        let verdicts = coverage
            .attribute(&[&line2, &line4], &["test_calc.test_add".to_string()])
            .unwrap();
        assert_eq!(verdicts[0], (1, Status::Killed));
        assert_eq!(verdicts[1], (2, Status::Survived));
    }

    #[test]
    fn refuses_to_attribute_an_unexplained_failure() {
        let coverage = coverage();
        let line4 = mutant(2, "calc.py", 4);
        let unknown = coverage.attribute(&[&line4], &["test_calc.test_mystery".to_string()]);
        assert!(unknown.is_none());
        let uncovered = coverage.attribute(&[&line4], &["test_calc.test_add".to_string()]);
        assert!(uncovered.is_none());
    }

    #[test]
    fn normalizes_junit_names_to_contexts() {
        assert_eq!(normalize("test_calc::test_add[3]"), "test_calc.test_add");
        assert_eq!(
            normalize("pkg.test_mod::TestX::test_y"),
            "pkg.test_mod.TestX.test_y"
        );
    }

    /// The warm worker reports node ids, not junit classnames; both must land
    /// on the same context or attribution silently degrades to bisection.
    #[test]
    fn normalizes_node_ids_to_the_same_contexts() {
        assert_eq!(normalize("test_calc.py::test_add"), "test_calc.test_add");
        assert_eq!(normalize("test_calc.py::test_add[3]"), "test_calc.test_add");
        assert_eq!(
            normalize("pkg/test_mod.py::TestX::test_y"),
            "pkg.test_mod.TestX.test_y"
        );
        assert_eq!(normalize("pkg\\test_mod.py::test_y"), "pkg.test_mod.test_y");
    }

    #[test]
    fn wraps_only_the_default_pytest_command() {
        let rcfile = Path::new(".angelo/coveragerc");
        let default = ["python", "-m", "pytest", "-q"].map(String::from);
        let wrapped = coverage_command(&default, rcfile).unwrap();
        assert_eq!(
            wrapped[..5],
            ["python", "-m", "coverage", "run", "--rcfile"]
        );
        assert_eq!(wrapped[6..], ["-m", "pytest", "-q"]);

        let custom = ["pytest".to_string()];
        assert!(coverage_command(&custom, rcfile).is_none());
    }
}
