use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

use crate::batch;
use crate::config::{self, Config};
use crate::coverage::{self, Baseline, Coverage, TestCoverage};
use crate::db::Database;
use crate::diff::{ChangedLines, Scope};
use crate::mutate::{self, Mutant, Status};
use crate::pytest::Budget;
use crate::report::{self, Progress};
use crate::runner::TestRunner;
use crate::warm;

const ANGELO_DIR: &str = ".angelo";

pub struct Options {
    pub workers: Option<usize>,
    pub init_only: bool,
    /// Mutate only the lines a change touched, rather than the whole codebase.
    pub scope: Option<Scope>,
    /// Cap the mutant pool, dropping the overflow at random.
    pub sample: Option<usize>,
    /// Fail the run when the score lands under this percentage.
    pub fail_under: Option<f64>,
}

pub fn run(options: Options) -> Result<ExitCode> {
    let config = config::load_or_init()?;
    let fail_under = options.fail_under.unwrap_or(config.fail_under);
    let angelo_dir = Path::new(ANGELO_DIR);
    let database = Database::open(angelo_dir)?;

    if database.mutant_count()? == 0 {
        enumerate(&database, &config, options.scope.as_ref())?;
        sample(&database, options.sample.unwrap_or(config.sample))?;
    } else {
        // The pool was fixed when it was enumerated, so paths, exclude and
        // --diff have nothing left to filter. Say so, or a new exclude looks
        // ignored.
        println!(
            "existing {ANGELO_DIR}/angelo.db found, resuming. paths, exclude and --diff apply at \
             enumeration only, so delete {ANGELO_DIR}/ for a fresh run"
        );
    }

    let pending = database.pending_mutants()?;
    if options.init_only {
        println!(
            "{} mutants pending, stopping here (--init-only)",
            pending.len()
        );
        return Ok(ExitCode::SUCCESS);
    }
    if pending.is_empty() {
        println!("nothing pending");
        return summarise(&database, fail_under);
    }

    let test_command = config.test_command_parts()?;
    println!("baseline: running the unmutated suite once, with per-test coverage");
    let baseline = coverage::baseline(Path::new("."), &test_command, angelo_dir)?;
    let coverage = baseline.coverage.as_ref();
    if coverage.is_none() {
        println!(
            "no per-test coverage (needs `pip install coverage` and a `python -m pytest` test command), no batching, no test selection"
        );
    }
    let budget = Budget::new(baseline.duration, config.timeout_factor);
    println!(
        "baseline {} in {:.1}s, timeout {:.1}s for a whole-suite run, from its own tests for a selected one",
        if baseline.is_green() { "green" } else { "RED" },
        baseline.duration.as_secs_f64(),
        budget.whole_suite().as_secs_f64()
    );

    let (testable, untested) = split_untested(pending, coverage);
    if !untested.is_empty() {
        database.set_status(&untested, Status::Survived)?;
        println!(
            "{} mutants sit on lines no test executes, survived without a single run",
            untested.len()
        );
        if coverage.is_some() && testable.is_empty() {
            eprintln!(
                "warning: per-test coverage was collected but matched no mutant at all, so every \
                 mutant survived without a run and the score below measures nothing. Check that \
                 `paths` names the same code the suite imports."
            );
        }
    }

    let (testable, untestable) = split_untestable(testable, &baseline, config.test_selection)?;
    // Said whenever the baseline is red, not only when it cost a mutant: a
    // suite the reader believed was green is news on its own.
    if !baseline.is_green() {
        eprintln!(
            "warning: {} tests were already failing before any mutant was planted",
            baseline.already_failing.len()
        );
    }
    if !untestable.is_empty() {
        database.set_status(&untestable, Status::Untestable)?;
        eprintln!(
            "warning: {} mutants set untestable, the tests covering them are already red, so a \
             run could not tell a kill from the failure that was already there",
            untestable.len()
        );
    }
    if testable.is_empty() {
        return summarise(&database, fail_under);
    }

    let mut progress = Progress::new(&testable, config.show_loading);
    let batches = batch::compose(testable, coverage, config.batch_size);
    let workers = config
        .effective_workers(options.workers)
        .min(batches.len())
        .max(1);
    println!(
        "running {} batches on {workers} workers{}",
        batches.len(),
        if config.test_selection && coverage.is_some() {
            ", covering tests only"
        } else {
            ""
        }
    );

    let runner = TestRunner {
        project_root: std::env::current_dir().context("finding the project root")?,
        warm_workers: config.warm_workers && warm::hostable(&test_command),
        warm_recycle_after: config.warm_recycle_after.max(1),
        test_command,
        budget,
        test_selection: config.test_selection,
    };
    runner.run_all(&batches, coverage, workers, |outcome| {
        progress.print(&outcome);
        database.record_batch(&outcome)
    })?;
    progress.finish();

    summarise(&database, fail_under)
}

fn enumerate(database: &Database, config: &Config, scope: Option<&Scope>) -> Result<()> {
    // Ask git first: a shallow clone or an unknown revision should stop the
    // run before it parses the whole codebase, not after.
    let scoped = match scope {
        Some(scope) => {
            let range = scope.range()?;
            Some((ChangedLines::over(&range)?, range))
        }
        None => None,
    };

    let sources = config.python_files()?;
    if sources.excluded_everything() {
        bail!(
            "every Python file under paths {:?} was excluded by {:?}, check angelo.conf",
            config.paths,
            config.exclude
        );
    }
    if sources.files.is_empty() {
        bail!(
            "no Python files found under paths {:?}, check angelo.conf",
            config.paths
        );
    }
    let mut mutants = Vec::new();
    for file in &sources.files {
        mutants.extend(mutate::enumerate_file(file)?);
    }
    println!(
        "enumerated {} mutants across {} files{}",
        mutants.len(),
        sources.files.len(),
        sources.exclusion_note()
    );
    for pattern in sources.unused_patterns() {
        println!("warning: exclude pattern {pattern:?} matched nothing");
    }

    if let Some((changed, range)) = scoped {
        let before = mutants.len();
        mutants = changed.filter(mutants);
        println!(
            "diff vs {range}: {} of {before} mutants sit on changed lines",
            mutants.len()
        );
        if mutants.is_empty() {
            println!("nothing changed. Delete {ANGELO_DIR}/ and re-run to widen the scope");
        }
    }
    database.insert_mutants(&mutants)
}

/// Cap the pool by deleting mutants at random.
///
/// This is not "run only the first N". The overflow is dropped from the
/// database entirely, so what remains is a random sample of the whole
/// codebase, and the score is an estimate over that sample, not a complete
/// census of whichever files happened to be enumerated first.
fn sample(database: &Database, keep: usize) -> Result<()> {
    if keep == 0 {
        return Ok(());
    }
    let total = database.mutant_count()?;
    let dropped = database.sample_down_to(keep)?;
    if dropped == 0 {
        return Ok(());
    }
    println!(
        "sampled {keep} of {total} mutants, {dropped} dropped at random, so the score is an \
         ESTIMATE over a random sample, not a full census"
    );
    Ok(())
}

/// Mutants no test executes cannot be killed, so they never need a run.
///
/// This has to happen before the fair-trial split: an untested mutant survives
/// without a run whether the baseline is green or red, and its empty selection
/// would otherwise read as untestable.
fn split_untested(pending: Vec<Mutant>, coverage: Option<&Coverage>) -> (Vec<Mutant>, Vec<Mutant>) {
    let Some(coverage) = coverage else {
        return (pending, Vec::new());
    };
    pending
        .into_iter()
        .partition(|mutant| !matches!(coverage.classify(mutant), TestCoverage::Untested))
}

/// Under a red baseline, keep only the mutants whose run can avoid every test
/// that was already failing. The rest are `untestable`: a run would exit 1
/// because of the failure that was already there and score them `killed`.
///
/// Working around a red baseline needs both a coverage map and test selection.
/// Without either, every run executes the whole red suite, nothing distinguishes
/// a contaminated mutant from a clean one, and every mutant in the project would
/// come back detected. Inventing a perfect score is worse than refusing to run,
/// so that combination still stops.
fn split_untestable(
    pending: Vec<Mutant>,
    baseline: &Baseline,
    test_selection: bool,
) -> Result<(Vec<Mutant>, Vec<Mutant>)> {
    if baseline.is_green() {
        return Ok((pending, Vec::new()));
    }
    let failing = baseline.already_failing.len();
    let Some(coverage) = baseline.coverage.as_ref().filter(|_| test_selection) else {
        bail!(
            "the baseline suite is red ({failing} tests already failing) and angelo cannot work \
             around that here.\nSkipping the mutants those tests cover needs per-test coverage \
             and test selection: `pip install coverage`, keep a `python -m pytest` \
             test_command, and leave test_selection = true in {}.\nOtherwise every run executes \
             the whole red suite, exits 1, and every mutant is scored killed.",
            config::CONFIG_FILE
        );
    };
    Ok(pending
        .into_iter()
        .partition(|mutant| coverage.gets_a_fair_trial(mutant, &baseline.already_failing)))
}

/// Print the report, then hand CI an exit code. A threshold failure is a
/// verdict rather than a crash, so it says its piece on stdout with the rest of
/// the report; a red baseline still comes out of `anyhow` on stderr.
fn summarise(database: &Database, fail_under: f64) -> Result<ExitCode> {
    let counts = database.status_counts()?;
    if counts.is_empty() {
        // An empty pool is not a pass. A docs-only branch scoped by --diff-base
        // lands here, and a score over nothing would read as one. There is
        // nothing for a threshold to judge either, so it is not a failure.
        println!("no mutants in scope: nothing was measured, so there is no score");
        return Ok(ExitCode::SUCCESS);
    }
    let summary = report::print_summary(&counts, &database.survivors()?);
    let Some(failure) = summary.gate(fail_under).failure() else {
        return Ok(ExitCode::SUCCESS);
    };
    println!("{failure}");
    Ok(ExitCode::FAILURE)
}
