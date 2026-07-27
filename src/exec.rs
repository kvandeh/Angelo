use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::batch;
use crate::config::{self, Config};
use crate::coverage::{self, Coverage, TestCoverage};
use crate::db::Database;
use crate::diff::{ChangedLines, Scope};
use crate::mutate::{self, Mutant};
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
}

pub fn run(options: Options) -> Result<()> {
    let config = config::load_or_init()?;
    let angelo_dir = Path::new(ANGELO_DIR);
    let database = Database::open(angelo_dir)?;

    if database.mutant_count()? == 0 {
        enumerate(&database, &config, options.scope.as_ref())?;
        sample(&database, options.sample.unwrap_or(config.sample))?;
    } else {
        println!(
            "existing {ANGELO_DIR}/angelo.db found, resuming (delete {ANGELO_DIR}/ for a fresh run)"
        );
    }

    let pending = database.pending_mutants()?;
    if options.init_only {
        println!(
            "{} mutants pending, stopping here (--init-only)",
            pending.len()
        );
        return Ok(());
    }
    if pending.is_empty() {
        println!("nothing pending");
        return summarise(&database);
    }

    let test_command = config.test_command_parts()?;
    println!("baseline: running the unmutated suite once, with per-test coverage");
    let (baseline, coverage) = coverage::baseline(Path::new("."), &test_command, angelo_dir)?;
    if coverage.is_none() {
        println!(
            "no per-test coverage (needs `pip install coverage` and the default pytest command), no batching, no test selection"
        );
    }
    let budget = Budget::new(baseline, config.timeout_factor);
    println!(
        "baseline green in {:.1}s, timeout {:.1}s for a whole-suite run, from its own tests for a selected one",
        baseline.as_secs_f64(),
        budget.whole_suite().as_secs_f64()
    );

    let (testable, untested) = split_untested(pending, coverage.as_ref());
    if !untested.is_empty() {
        database.record_survived_unrun(&untested)?;
        println!(
            "{} mutants sit on lines no test executes, survived without a single run",
            untested.len()
        );
    }
    if testable.is_empty() {
        return summarise(&database);
    }

    let mut progress = Progress::new(&testable);
    let batches = batch::compose(testable, coverage.as_ref(), config.batch_size);
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
    runner.run_all(&batches, coverage.as_ref(), workers, |outcome| {
        progress.print(&outcome);
        database.record_batch(&outcome)
    })?;

    summarise(&database)
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

    let files = config.python_files()?;
    if files.is_empty() {
        bail!(
            "no Python files found under paths {:?}, check angelo.conf",
            config.paths
        );
    }
    let mut mutants = Vec::new();
    for file in &files {
        mutants.extend(mutate::enumerate_file(file)?);
    }
    println!(
        "enumerated {} mutants across {} files",
        mutants.len(),
        files.len()
    );

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
fn split_untested(pending: Vec<Mutant>, coverage: Option<&Coverage>) -> (Vec<Mutant>, Vec<Mutant>) {
    let Some(coverage) = coverage else {
        return (pending, Vec::new());
    };
    pending
        .into_iter()
        .partition(|mutant| !matches!(coverage.classify(mutant), TestCoverage::Untested))
}

fn summarise(database: &Database) -> Result<()> {
    let counts = database.status_counts()?;
    if counts.is_empty() {
        // An empty pool is not a pass. A docs-only branch scoped by --diff-base
        // lands here, and a score over nothing would read as one.
        println!("no mutants in scope: nothing was measured, so there is no score");
        return Ok(());
    }
    report::print_summary(&counts, &database.survivors()?);
    Ok(())
}
