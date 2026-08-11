use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use indicatif::MultiProgress;

use crate::batch;
use crate::config::{self, Config};
use crate::coverage::{self, Baseline, Coverage, TestCoverage};
use crate::db::Database;
use crate::diff::{ChangedLines, Scope};
use crate::html;
use crate::mutate::{self, Mutant, Status};
use crate::pytest::Budget;
use crate::report::{self, Diagnostics, Phase, Progress};
use crate::runner::TestRunner;
use crate::schemata::Schemata;
use crate::sonar;
use crate::stryker;
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
    /// Write the mutation-testing-report JSON here.
    pub report: Option<PathBuf>,
    /// Write the self-contained HTML report here.
    pub html_report: Option<PathBuf>,
    /// Write SonarQube's generic issue import format here.
    pub sonar_report: Option<PathBuf>,
}

pub fn run(options: Options, bars: &MultiProgress) -> Result<ExitCode> {
    let config = config::load_or_init()?;
    let fail_under = options.fail_under.unwrap_or(config.fail_under);
    let reports = Reports::of(&options, &config);
    let angelo_dir = Path::new(ANGELO_DIR);
    let database = Database::open(angelo_dir)?;
    let mut diagnostics = Diagnostics::default();

    if database.mutant_count()? == 0 {
        enumerate(
            &database,
            &config,
            options.scope.as_ref(),
            bars,
            &mut diagnostics,
        )?;
        sample(
            &database,
            options.sample.unwrap_or(config.sample),
            &mut diagnostics,
        )?;
    } else {
        // The pool was fixed when it was enumerated, so paths, exclude and
        // --diff have nothing left to filter. Say so, or a new exclude looks
        // ignored.
        diagnostics.note(format!(
            "existing {ANGELO_DIR}/angelo.db found, resuming. paths, exclude and --diff apply at \
             enumeration only, so delete {ANGELO_DIR}/ for a fresh run"
        ));
    }

    let pending = database.pending_mutants()?;
    if options.init_only {
        log::info!(
            "{} mutants pending, stopping here (--init-only)",
            pending.len()
        );
        reports.write(&database, &diagnostics, fail_under)?;
        return Ok(ExitCode::SUCCESS);
    }
    if pending.is_empty() {
        log::info!("nothing pending");
        return summarise(&database, fail_under, &diagnostics, &reports);
    }

    let test_command = config.test_command_parts()?;
    let baseline = {
        // The longest single wait in a run, and the one nobody can put a length
        // on in advance, so it spins rather than pretending to fill.
        let phase = Phase::spinner(bars, "baseline");
        phase.say("running the unmutated suite once, with per-test coverage");
        let baseline = coverage::baseline(Path::new("."), &test_command, angelo_dir)?;
        phase.done();
        baseline
    };
    let coverage = baseline.coverage.as_ref();
    if coverage.is_none() {
        diagnostics.warn(
            "no per-test coverage (needs `pip install coverage` and a `python -m pytest` test \
             command), no batching, no test selection",
        );
    }
    let budget = Budget::new(baseline.duration, config.timeout_factor);
    log::info!(
        "baseline {} in {:.1}s, timeout {:.1}s for a whole-suite run, from its own tests for a selected one",
        if baseline.is_green() { "green" } else { "RED" },
        baseline.duration.as_secs_f64(),
        budget.whole_suite().as_secs_f64()
    );
    diagnostics.fact(
        "baseline",
        format!(
            "{} in {:.1}s",
            if baseline.is_green() { "green" } else { "RED" },
            baseline.duration.as_secs_f64()
        ),
    );
    diagnostics.fact("per-test coverage", on_or_off(coverage.is_some()));

    let (testable, untested) = split_untested(pending, coverage);
    // Before the statuses are written, or resuming would find nothing pending
    // and report the fabricated score this refuses to print.
    if coverage.is_some() && testable.is_empty() && !untested.is_empty() {
        bail!(
            "per-test coverage was collected but matched no mutant at all, so every mutant would \
             survive without a run and the score would measure nothing.\nTwo things do this. A \
             pytest plugin in the project's own addopts takes the coverage run over -- pytest-cov \
             through --cov, pytest-xdist through -n auto -- in which case add `--no-cov` or `-o \
             addopts=` to test_command, alongside `warm_workers = false`. Otherwise `paths` names \
             code the suite never imports."
        );
    }
    if !untested.is_empty() {
        database.set_status(&untested, Status::Survived)?;
        diagnostics.note(format!(
            "{} mutants sit on lines no test executes, survived without a single run",
            untested.len()
        ));
    }

    let (testable, untestable) = split_untestable(testable, &baseline, config.test_selection)?;
    // Said whenever the baseline is red, not only when it cost a mutant: a
    // suite the reader believed was green is news on its own.
    if !baseline.is_green() {
        diagnostics.warn(format!(
            "{} tests were already failing before any mutant was planted",
            baseline.already_failing.len()
        ));
    }
    if !untestable.is_empty() {
        database.set_status(&untestable, Status::Untestable)?;
        diagnostics.warn(format!(
            "{} mutants set untestable, the tests covering them are already red, so a run could \
             not tell a kill from the failure that was already there",
            untestable.len()
        ));
    }
    if testable.is_empty() {
        return summarise(&database, fail_under, &diagnostics, &reports);
    }

    let mut progress = Progress::new(bars, &testable);
    let schemata = build_schemata(&config, &testable, &mut diagnostics)?;
    let batches = {
        let phase = Phase::counted(bars, "batching", testable.len());
        let batches = compose_batches(testable, coverage, &config, schemata.as_ref(), || {
            phase.tick()
        });
        phase.done();
        batches
    };
    let workers = config
        .effective_workers(options.workers)
        .min(batches.len())
        .max(1);
    let selecting = config.test_selection && coverage.is_some();
    log::info!(
        "running {} batches on {workers} workers{}",
        batches.len(),
        match selecting {
            true => ", covering tests only",
            false => "",
        }
    );
    diagnostics.fact("workers", workers.to_string());
    diagnostics.fact("batch size", config.batch_size.to_string());
    diagnostics.fact("test selection", on_or_off(selecting));
    let warm_workers = config.warm_workers && warm::hostable(&test_command);
    diagnostics.fact("warm workers", on_or_off(warm_workers));

    let runner = TestRunner {
        project_root: std::env::current_dir().context("finding the project root")?,
        warm_workers,
        warm_recycle_after: config.warm_recycle_after.max(1),
        test_command,
        budget,
        test_selection: config.test_selection,
        schemata,
    };
    runner.run_all(&batches, coverage, workers, |outcome| {
        progress.print(&outcome);
        database.record_batch(&outcome)
    })?;
    progress.finish();

    summarise(&database, fail_under, &diagnostics, &reports)
}

fn on_or_off(enabled: bool) -> &'static str {
    match enabled {
        true => "on",
        false => "off",
    }
}

/// Where a run's report files go. Empty means off, which is the default, and
/// the flag beats the config key the same way `--fail-under` does.
struct Reports {
    json: Option<PathBuf>,
    html: Option<PathBuf>,
    sonar: Option<PathBuf>,
}

impl Reports {
    fn of(options: &Options, config: &Config) -> Reports {
        let configured = |path: &str| match path.is_empty() {
            true => None,
            false => Some(PathBuf::from(path)),
        };
        Reports {
            json: options
                .report
                .clone()
                .or_else(|| configured(&config.report)),
            html: options
                .html_report
                .clone()
                .or_else(|| configured(&config.html_report)),
            sonar: options
                .sonar_report
                .clone()
                .or_else(|| configured(&config.sonar_report)),
        }
    }

    /// Read from the database rather than from the run, so `--init-only`, a
    /// resumed run and a run with nothing pending all produce a report.
    ///
    /// A report is output and never a verdict: writing one must not change what
    /// the run decided, which is what keeps `verdict-matrix.sh` agreeing with
    /// itself with the flags on.
    fn write(&self, database: &Database, diagnostics: &Diagnostics, fail_under: f64) -> Result<()> {
        if self.json.is_none() && self.html.is_none() && self.sonar.is_none() {
            return Ok(());
        }
        let settled = database.all_mutants()?;
        if let Some(path) = &self.json {
            stryker::write(path, &settled, fail_under)?;
            log::info!(
                "wrote {} in the mutation-testing-report schema",
                path.display()
            );
        }
        if let Some(path) = &self.html {
            html::write(path, &settled, &database.status_counts()?, diagnostics)?;
            log::info!("wrote {}", path.display());
        }
        if let Some(path) = &self.sonar {
            sonar::write(path, &settled)?;
            log::info!(
                "wrote {} in SonarQube's generic issue import format",
                path.display()
            );
        }
        Ok(())
    }
}

/// Batch the hosted mutants apart from the spliced ones.
///
/// A batch runs on the schemata path only if **every** member is hosted, since
/// one spliced member needs the file written and the re-import that comes with
/// it. Mixing them therefore wastes the feature rather than sharing it: at two
/// mutants in three hosted, a batch of eight is all-hosted about one time in
/// forty, so nearly every batch would take the slow path. Splitting first makes
/// every hosted batch a fast one, and puts them ahead of the rest so a worker
/// switches paths once.
fn compose_batches(
    testable: Vec<Mutant>,
    coverage: Option<&Coverage>,
    config: &Config,
    schemata: Option<&Schemata>,
    placed: impl Fn(),
) -> Vec<batch::Batch> {
    let Some(schemata) = schemata else {
        return batch::compose(testable, coverage, config.batch_size, None, &placed);
    };
    let (hosted, spliced): (Vec<Mutant>, Vec<Mutant>) = testable
        .into_iter()
        .partition(|mutant| schemata.hosts(mutant));
    let mut batches = batch::compose(hosted, coverage, config.batch_size, Some(schemata), &placed);
    batches.extend(batch::compose(
        spliced,
        coverage,
        config.batch_size,
        None,
        &placed,
    ));
    batches
}

/// Compile every mutant into its file at once, if this platform can run the
/// result honestly.
///
/// Schemata mean a run re-imports nothing, which is the whole saving and also
/// the whole risk: nothing resets the process between mutants either. Only the
/// fork worker gives each mutant a process no other mutant has touched, and
/// `fork()` does not exist on Windows. Rather than leak state there, Windows
/// keeps splicing, which re-imports and is slower and correct.
fn build_schemata(
    config: &Config,
    mutants: &[Mutant],
    diagnostics: &mut Diagnostics,
) -> Result<Option<Schemata>> {
    if !config.schemata {
        return Ok(None);
    }
    if !cfg!(unix) || !config.warm_workers {
        diagnostics.note(
            "schemata off: they need the fork worker, which needs warm_workers and a platform \
             with fork(). Mutants are spliced into the files instead",
        );
        return Ok(None);
    }
    let schemata = Schemata::build(Path::new("."), mutants)?;
    diagnostics.fact(
        "schemata",
        format!(
            "{} of {} mutants compiled in, the rest spliced",
            schemata.hosted_count(),
            mutants.len()
        ),
    );
    Ok((!schemata.is_empty()).then_some(schemata))
}

fn enumerate(
    database: &Database,
    config: &Config,
    scope: Option<&Scope>,
    bars: &MultiProgress,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
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

    let phase = Phase::counted(bars, "parsing", sources.files.len());
    let mut mutants = Vec::new();
    for file in &sources.files {
        mutants.extend(mutate::enumerate_file(file)?);
        phase.tick();
    }
    phase.done();

    log::info!(
        "enumerated {} mutants across {} files{}",
        mutants.len(),
        sources.files.len(),
        sources.exclusion_note()
    );
    for pattern in sources.unused_patterns() {
        diagnostics.warn(format!("exclude pattern {pattern:?} matched nothing"));
    }

    if let Some((changed, range)) = scoped {
        let before = mutants.len();
        mutants = changed.filter(mutants);
        diagnostics.note(format!(
            "diff vs {range}: {} of {before} mutants sit on changed lines, {} dropped",
            mutants.len(),
            before - mutants.len()
        ));
        if mutants.is_empty() {
            log::info!("nothing changed. Delete {ANGELO_DIR}/ and re-run to widen the scope");
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
fn sample(database: &Database, keep: usize, diagnostics: &mut Diagnostics) -> Result<()> {
    if keep == 0 {
        return Ok(());
    }
    let total = database.mutant_count()?;
    let dropped = database.sample_down_to(keep)?;
    if dropped == 0 {
        return Ok(());
    }
    diagnostics.warn(format!(
        "sampled {keep} of {total} mutants, {dropped} dropped at random, so the score is an \
         ESTIMATE over a random sample, not a full census"
    ));
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
fn summarise(
    database: &Database,
    fail_under: f64,
    diagnostics: &Diagnostics,
    reports: &Reports,
) -> Result<ExitCode> {
    let counts = database.status_counts()?;
    if counts.is_empty() {
        // An empty pool is not a pass. A docs-only branch scoped by --diff-base
        // lands here, and a score over nothing would read as one. There is
        // nothing for a threshold to judge either, so it is not a failure.
        println!("no mutants in scope: nothing was measured, so there is no score");
        return Ok(ExitCode::SUCCESS);
    }
    let summary = report::print_summary(&counts, &database.survivors()?);
    reports.write(database, diagnostics, fail_under)?;
    let Some(failure) = summary.gate(fail_under).failure() else {
        return Ok(ExitCode::SUCCESS);
    };
    println!("{failure}");
    Ok(ExitCode::FAILURE)
}
