use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::batch::Batch;
use crate::config::SKIP_DIRS;
use crate::coverage::Coverage;
use crate::mutate::{Mutant, Status};
use crate::pytest::{self, Budget, Selection, SuiteResult};
use crate::warm::WarmWorker;

/// One decisive pytest run and the verdicts it settled.
pub struct BatchOutcome {
    pub duration_ms: u64,
    pub exit_code: Option<i64>,
    pub failed_tests: Vec<String>,
    pub verdicts: Vec<(i64, Status)>,
    pub error: Option<String>,
}

impl BatchOutcome {
    fn survived(mutants: &[&Mutant], duration: Duration) -> BatchOutcome {
        BatchOutcome {
            duration_ms: duration.as_millis() as u64,
            exit_code: Some(0),
            failed_tests: Vec::new(),
            verdicts: mutants.iter().map(|m| (m.id, Status::Survived)).collect(),
            error: None,
        }
    }

    fn attributed(
        verdicts: Vec<(i64, Status)>,
        failed_tests: Vec<String>,
        duration: Duration,
    ) -> BatchOutcome {
        BatchOutcome {
            duration_ms: duration.as_millis() as u64,
            exit_code: Some(1),
            failed_tests,
            verdicts,
            error: None,
        }
    }

    fn failed(mutant: &Mutant, error: &anyhow::Error, duration: Duration) -> BatchOutcome {
        BatchOutcome {
            duration_ms: duration.as_millis() as u64,
            exit_code: None,
            failed_tests: Vec::new(),
            verdicts: vec![(mutant.id, Status::Error)],
            error: Some(format!("{error:#}")),
        }
    }
}

pub struct TestRunner {
    pub project_root: PathBuf,
    pub test_command: Vec<String>,
    /// How long a run may take, worked out per run from the tests it selects.
    pub budget: Budget,
    /// Run only the tests covering a batch instead of the whole suite.
    pub test_selection: bool,
    /// Host runs in a long-lived pytest process instead of spawning one each time.
    pub warm_workers: bool,
    /// Restart that process every N runs, bounding any state it accumulates.
    pub warm_recycle_after: usize,
}

impl TestRunner {
    pub fn run_all(
        &self,
        batches: &[Batch],
        coverage: Option<&Coverage>,
        worker_count: usize,
        mut on_outcome: impl FnMut(BatchOutcome) -> Result<()>,
    ) -> Result<()> {
        let next_batch = AtomicUsize::new(0);
        let (sender, receiver) = mpsc::channel::<BatchOutcome>();

        thread::scope(|scope| {
            for worker_id in 0..worker_count {
                let sender = sender.clone();
                let next_batch = &next_batch;
                scope.spawn(
                    move || match Worker::start(self, coverage, sender, worker_id) {
                        // Unclaimed batches stay pending, so a re-run picks them up.
                        Err(error) => eprintln!("worker could not copy the project: {error:#}"),
                        Ok(worker) => worker.take_batches(batches, next_batch),
                    },
                );
            }
            // Drop our own sender so the receiver ends when the workers finish.
            drop(sender);

            for outcome in receiver {
                on_outcome(outcome)?;
            }
            Ok(())
        })
    }
}

/// One thread with its own copy of the project. Borrows the runner and the
/// coverage map for as long as it lives, which `thread::scope` guarantees is
/// shorter than `run_all`.
struct Worker<'a> {
    runner: &'a TestRunner,
    coverage: Option<&'a Coverage>,
    copy: WorkerCopy,
    outcomes: Sender<BatchOutcome>,
    /// None when warm running is off or unavailable; refreshed when exhausted.
    warm: RefCell<Option<WarmWorker>>,
}

/// A send failure means the main thread stopped listening: quit the worker.
type Listening = Result<(), mpsc::SendError<BatchOutcome>>;

impl<'a> Worker<'a> {
    fn start(
        runner: &'a TestRunner,
        coverage: Option<&'a Coverage>,
        outcomes: Sender<BatchOutcome>,
        worker_id: usize,
    ) -> Result<Worker<'a>> {
        Ok(Worker {
            copy: WorkerCopy::create(&runner.project_root, worker_id)?,
            runner,
            coverage,
            outcomes,
            warm: RefCell::new(None),
        })
    }

    fn take_batches(&self, batches: &[Batch], next_batch: &AtomicUsize) {
        loop {
            let index = next_batch.fetch_add(1, Ordering::Relaxed);
            let Some(batch) = batches.get(index) else {
                return;
            };
            let mutants: Vec<&Mutant> = batch.mutants.iter().collect();
            if self.judge(&mutants).is_err() {
                return;
            }
        }
    }

    /// Green run: everyone survived. Red run: a failed test kills the batch
    /// member it covers. When a result cannot be attributed (timeout, crash,
    /// a failure no member explains), split in half and re-run, a verdict
    /// must come from a run that proves it.
    fn judge(&self, mutants: &[&Mutant]) -> Listening {
        let started = Instant::now();
        let report = self.run_suite(mutants);
        let elapsed = started.elapsed();

        if matches!(&report, Ok(report) if report.survived()) {
            return self.outcomes.send(BatchOutcome::survived(mutants, elapsed));
        }
        if mutants.len() > 1 {
            if let Some(outcome) = self.attribute(&report, mutants, elapsed) {
                return self.outcomes.send(outcome);
            }
            let (left, right) = mutants.split_at(mutants.len() / 2);
            self.judge(left)?;
            return self.judge(right);
        }

        let outcome = match report {
            Ok(report) => report.into_outcome(mutants[0], elapsed),
            Err(error) => BatchOutcome::failed(mutants[0], &error, elapsed),
        };
        self.outcomes.send(outcome)
    }

    /// Charge each failed test to the one batch member it covers. None when
    /// anything is unexplained, which sends the caller to bisection.
    fn attribute(
        &self,
        report: &Result<RunReport>,
        mutants: &[&Mutant],
        elapsed: Duration,
    ) -> Option<BatchOutcome> {
        let report = report.as_ref().ok()?;
        if report.result.status() != Status::Killed {
            return None;
        }
        let verdicts = self.coverage?.attribute(mutants, &report.failed_tests)?;
        Some(BatchOutcome::attributed(
            verdicts,
            report.failed_tests.clone(),
            elapsed,
        ))
    }

    fn run_suite(&self, mutants: &[&Mutant]) -> Result<RunReport> {
        let selection = match self.coverage {
            Some(coverage) if self.runner.test_selection => coverage.select(mutants),
            _ => Selection::whole_suite(),
        };
        // A run that names its tests is budgeted from them: waiting out the
        // whole suite's budget for two tests worth 50ms tells us nothing.
        let timeout = self.runner.budget.for_selection(&selection);
        let _patched = PatchedFiles::apply(&self.copy.root, mutants)?;
        match self.run_warm(&selection, timeout) {
            Some(report) => Ok(report),
            None => self.run_subprocess(&selection, timeout),
        }
    }

    /// None means warm running is off, unavailable, or just failed, the caller
    /// then pays for a fresh subprocess, which is always correct.
    fn run_warm(&self, selection: &Selection, timeout: Duration) -> Option<RunReport> {
        if !self.runner.warm_workers {
            return None;
        }
        let mut slot = self.warm.borrow_mut();
        if slot.as_ref().is_some_and(WarmWorker::is_exhausted) {
            *slot = None;
        }
        if slot.is_none() {
            *slot = WarmWorker::start(
                &self.runner.test_command,
                &self.copy.root,
                self.runner.warm_recycle_after,
            )
            .ok();
        }

        let run = slot.as_mut()?.run(selection, timeout);
        match run {
            // A timed-out worker is still stuck on that test: retire it, but the
            // timeout itself is a real verdict.
            Ok(run) if matches!(run.result, SuiteResult::TimedOut) => {
                *slot = None;
                Some(RunReport {
                    result: SuiteResult::TimedOut,
                    failed_tests: Vec::new(),
                })
            }
            Ok(run) => Some(RunReport {
                result: run.result,
                failed_tests: run.failed_tests,
            }),
            Err(_) => {
                *slot = None;
                None
            }
        }
    }

    fn run_subprocess(&self, selection: &Selection, timeout: Duration) -> Result<RunReport> {
        let result = pytest::run(
            &self.runner.test_command,
            &self.copy.root,
            timeout,
            selection,
        )?;
        let failed_tests = match result.status() {
            Status::Killed => pytest::failed_tests(&self.copy.root).unwrap_or_default(),
            _ => Vec::new(),
        };
        Ok(RunReport {
            result,
            failed_tests,
        })
    }
}

/// One pytest run, however it was executed.
struct RunReport {
    result: SuiteResult,
    failed_tests: Vec<String>,
}

impl RunReport {
    fn survived(&self) -> bool {
        self.result.status() == Status::Survived
    }

    fn into_outcome(self, mutant: &Mutant, elapsed: Duration) -> BatchOutcome {
        BatchOutcome {
            duration_ms: elapsed.as_millis() as u64,
            exit_code: self.result.exit_code(),
            verdicts: vec![(mutant.id, self.result.status())],
            error: self.result.explain(),
            failed_tests: self.failed_tests,
        }
    }
}

/// Mutated files, restored when this drops.
struct PatchedFiles {
    originals: Vec<(PathBuf, String)>,
}

impl PatchedFiles {
    fn apply(root: &Path, mutants: &[&Mutant]) -> Result<PatchedFiles> {
        let mut by_file: HashMap<&Path, Vec<&Mutant>> = HashMap::new();
        for mutant in mutants {
            by_file
                .entry(mutant.file.as_path())
                .or_default()
                .push(mutant);
        }

        let mut patched = PatchedFiles {
            originals: Vec::new(),
        };
        for (file, mutants) in by_file {
            let path = root.join(file);
            let source =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            fs::write(&path, splice_all(&source, &mutants, file)?)
                .with_context(|| format!("writing mutants into {}", path.display()))?;
            patched.originals.push((path, source));
        }
        Ok(patched)
    }
}

impl Drop for PatchedFiles {
    // Drop cannot return errors. A failed restore leaves the copy mutated,
    // which the next batch's original-bytes check turns into an error verdict.
    fn drop(&mut self) {
        for (path, source) in &self.originals {
            let _ = fs::write(path, source);
        }
    }
}

fn splice_all(source: &str, mutants: &[&Mutant], file: &Path) -> Result<String> {
    for mutant in mutants {
        if source.get(mutant.byte_start..mutant.byte_end) != Some(mutant.original.as_str()) {
            bail!(
                "{} changed since enumeration, delete .angelo/ and re-run",
                file.display()
            );
        }
    }
    // Splice back to front so earlier byte offsets stay valid.
    let mut ordered: Vec<&&Mutant> = mutants.iter().collect();
    ordered.sort_by_key(|m| std::cmp::Reverse(m.byte_start));

    let mut mutated = source.to_string();
    for mutant in ordered {
        mutant.splice_into(&mut mutated);
    }
    Ok(mutated)
}

/// A private copy of the project, deleted when this drops.
struct WorkerCopy {
    root: PathBuf,
}

impl WorkerCopy {
    fn create(project_root: &Path, worker_id: usize) -> Result<WorkerCopy> {
        let root = env::temp_dir().join(format!("angelo-{}-{worker_id}", process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).with_context(|| format!("clearing {}", root.display()))?;
        }
        copy_project(project_root, &root)?;
        Ok(WorkerCopy { root })
    }
}

impl Drop for WorkerCopy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn copy_project(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry.with_context(|| format!("reading an entry of {}", from.display()))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if !source.is_dir() {
            fs::copy(&source, &target).with_context(|| format!("copying {}", source.display()))?;
            continue;
        }
        if !is_skipped(&source) {
            copy_project(&source, &target)?;
        }
    }
    Ok(())
}

fn is_skipped(dir: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    SKIP_DIRS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mutant(byte_start: usize, original: &str, replacement: &str) -> Mutant {
        Mutant {
            id: 0,
            file: PathBuf::from("calc.py"),
            line: 1,
            byte_start,
            byte_end: byte_start + original.len(),
            original: original.to_string(),
            replacement: replacement.to_string(),
        }
    }

    #[test]
    fn splices_several_mutants_into_one_file() {
        let source = "x = 1 + 2 * 3\n";
        let plus = mutant(6, "+", "-");
        let star = mutant(10, "*", "/");
        let mutated = splice_all(source, &[&plus, &star], Path::new("calc.py")).unwrap();
        assert_eq!(mutated, "x = 1 - 2 / 3\n");
    }

    #[test]
    fn splicing_rejects_a_file_that_changed() {
        let stale = mutant(6, "+", "-");
        let error = splice_all("x = 1 - 2\n", &[&stale], Path::new("calc.py"));
        assert!(error.is_err());
    }

    /// Splicing writes into the string in place, and a byte range that is not a
    /// character boundary panics. The original-bytes check in front of the
    /// splice is what proves the range is one, so a file full of multi-byte
    /// characters has to come out intact.
    #[test]
    fn splices_around_multibyte_characters() {
        let source = "s = 'héllo wörld'\nx = 1 + 2\n";
        let plus = mutant(source.find('+').unwrap(), "+", "-");
        let mutated = splice_all(source, &[&plus], Path::new("calc.py")).unwrap();
        assert_eq!(mutated, "s = 'héllo wörld'\nx = 1 - 2\n");
    }

    #[test]
    fn replacements_of_a_different_length_still_line_up() {
        let source = "if a >= b and c:\n";
        let compare = mutant(5, ">=", ">");
        let and = mutant(10, "and", "or");
        let mutated = splice_all(source, &[&compare, &and], Path::new("calc.py")).unwrap();
        assert_eq!(mutated, "if a > b or c:\n");
    }
}
