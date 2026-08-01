use std::collections::HashMap;
use std::env;
use std::io::{self, IsTerminal};
use std::sync::OnceLock;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use log::Level;

use crate::mutate::{Mutant, Status};
use crate::runner::BatchOutcome;

/// How often any bar repaints. The cost of drawing then scales with the wall
/// clock rather than with the pool size, which is exactly what a line per
/// mutant got wrong: output must not get more expensive the more work there is.
const REDRAW_HZ: u8 = 5;

/// Characters of bar, chosen to leave room for the counts on an 80-column
/// terminal.
const BAR_WIDTH: usize = 36;

/// The container every bar draws into, throttled and pointed at stderr.
///
/// stderr because stdout carries the report, and `indicatif` hides a bar
/// outright when stderr is not a terminal, so a redirected or piped run emits
/// no control characters at all.
pub fn bars() -> MultiProgress {
    MultiProgress::with_draw_target(ProgressDrawTarget::stderr_with_hz(REDRAW_HZ))
}

/// How a verdict reads on screen. Hand-rolled ANSI: four escape codes are less
/// than a dependency costs, and `IsTerminal` has been in the standard library
/// since 1.70.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Paint {
    Green,
    Yellow,
    Red,
    Dim,
    Bold,
}

impl Paint {
    /// The colour a settled mutant reads in. Detected is the good outcome, a
    /// survivor is the finding, an error is the one that invalidates a score,
    /// and an untestable mutant is a note rather than a result.
    fn of(status: Status) -> Paint {
        match status {
            Status::Killed | Status::Timeout => Paint::Green,
            Status::Survived => Paint::Yellow,
            Status::Error => Paint::Red,
            Status::Untestable => Paint::Dim,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Paint::Green => "\x1b[32m",
            Paint::Yellow => "\x1b[33m",
            Paint::Red => "\x1b[31m",
            Paint::Dim => "\x1b[2m",
            Paint::Bold => "\x1b[1m",
        }
    }

    /// The text in this colour, or exactly the text when colour is off.
    fn on(self, text: &str) -> String {
        match colour_is_on() {
            true => format!("{}{text}\x1b[0m", self.code()),
            false => text.to_string(),
        }
    }
}

/// Colour goes to a terminal and nowhere else. `verdict-matrix.sh` greps these
/// very lines, and so does anyone piping a run into `grep`, so a redirected run
/// has to stay byte-for-byte what it was before colour existed.
fn colour_wanted(is_terminal: bool, no_color_set: bool) -> bool {
    is_terminal && !no_color_set
}

fn colour_is_on() -> bool {
    // Asked once: `is_terminal` is a syscall and a survivor list can be long.
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        // NO_COLOR counts when it is set to anything at all, empty included.
        colour_wanted(
            io::stdout().is_terminal(),
            env::var_os("NO_COLOR").is_some(),
        )
    })
}

/// One step of a run, drawn while it happens.
///
/// A phase whose size is known counts; one whose length is the whole question
/// spins. The baseline is the longest single wait in a run and nobody can know
/// how long a test suite takes, so it gets a spinner rather than a fake bar.
pub struct Phase(ProgressBar);

impl Phase {
    pub fn counted(bars: &MultiProgress, what: &str, total: usize) -> Phase {
        let bar = bars.add(ProgressBar::new(total as u64));
        bar.set_style(
            ProgressStyle::with_template(&format!(
                "  {{prefix:<9}} [{{bar:{BAR_WIDTH}}}] {{percent:>3}}%  {{pos}}/{{len}}"
            ))
            .expect("a valid template")
            .progress_chars("#>-"),
        );
        bar.set_prefix(what.to_string());
        Phase(bar)
    }

    pub fn spinner(bars: &MultiProgress, what: &str) -> Phase {
        let bar = bars.add(ProgressBar::new_spinner());
        bar.set_style(
            ProgressStyle::with_template("  {prefix:<9} {spinner} {msg} ({elapsed})")
                .expect("a valid template"),
        );
        bar.set_prefix(what.to_string());
        // A spinner has no work to count, so something else has to move it.
        bar.enable_steady_tick(Duration::from_millis(1000 / REDRAW_HZ as u64));
        Phase(bar)
    }

    pub fn say(&self, message: impl Into<String>) {
        self.0.set_message(message.into());
    }

    pub fn tick(&self) {
        self.0.inc(1);
    }

    /// Take the bar off the screen. The phase's one-line result is a `log`
    /// record, so it obeys the verbosity rather than being drawn twice.
    pub fn done(self) {
        self.0.finish_and_clear();
    }
}

/// The bar that watches mutants settle, and the counts it displays.
pub struct Progress {
    bar: ProgressBar,
    /// Empty unless `debug` is on, because building one costs a `to_string`
    /// per mutant and nothing reads them at the default verbosity.
    labels: HashMap<i64, String>,
    total: usize,
    done: usize,
    detected: usize,
    survived: usize,
}

impl Progress {
    pub fn new(bars: &MultiProgress, mutants: &[Mutant]) -> Progress {
        let bar = bars.add(ProgressBar::new(mutants.len() as u64));
        bar.set_style(
            ProgressStyle::with_template(&format!(
                "  {{prefix:<9}} [{{bar:{BAR_WIDTH}}}] {{percent:>3}}%  {{pos}}/{{len}}  {{msg}}"
            ))
            .expect("a valid template")
            .progress_chars("#>-"),
        );
        bar.set_prefix("mutants");
        Progress {
            labels: match log::log_enabled!(Level::Debug) {
                true => mutants.iter().map(|m| (m.id, m.to_string())).collect(),
                false => HashMap::new(),
            },
            total: mutants.len(),
            done: 0,
            detected: 0,
            survived: 0,
            bar,
        }
    }

    pub fn print(&mut self, outcome: &BatchOutcome) {
        let seconds = outcome.duration_ms as f64 / 1000.0;
        let batch_note = match outcome.verdicts.len() {
            1 => String::new(),
            size => format!("  [batch of {size}]"),
        };
        for (mutant_id, status) in &outcome.verdicts {
            self.done += 1;
            if status.is_detected() {
                self.detected += 1;
            }
            if *status == Status::Survived {
                self.survived += 1;
            }
            if let Some(label) = self.labels.get(mutant_id) {
                log::debug!(
                    "[{}/{}] {label}  {}{batch_note}  ({seconds:.1}s)",
                    self.done,
                    self.total,
                    status.as_str()
                );
            }
        }
        if let Some(error) = &outcome.error {
            // "A run where every mutant dies almost instantly means a broken
            // test command" is the loudest thing angelo has to say. It goes
            // through `log`, so it suspends the bar rather than smearing it.
            log::warn!("{error}");
        }
        self.bar.set_message(counts_message(
            self.done,
            self.total,
            self.detected,
            self.survived,
            self.bar.elapsed(),
        ));
        self.bar.set_position(self.done as u64);
    }

    pub fn finish(&mut self) {
        self.bar.finish_and_clear();
    }
}

/// The counts that ride alongside the bar. Pure, so the arithmetic gets a unit
/// test rather than a screenshot.
fn counts_message(
    done: usize,
    total: usize,
    detected: usize,
    survived: usize,
    elapsed: Duration,
) -> String {
    format!(
        "detected {detected}  survived {survived}  ~{} left",
        remaining(done, total, elapsed)
    )
}

/// A linear extrapolation, which is all it can honestly be: batching settles
/// mutants in clumps and a red batch bisects into four more runs, so the rate
/// is lumpy by design. Hence the `~`.
fn remaining(done: usize, total: usize, elapsed: Duration) -> String {
    if done == 0 || done >= total {
        return "--".to_string();
    }
    let per_mutant = elapsed.as_secs_f64() / done as f64;
    let left = per_mutant * (total - done) as f64;
    compact(Duration::try_from_secs_f64(left).unwrap_or_default())
}

/// `2h05m`, `4m18s`, `43s`.
pub fn compact(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match (seconds / 3600, (seconds % 3600) / 60, seconds % 60) {
        (0, 0, seconds) => format!("{seconds}s"),
        (0, minutes, seconds) => format!("{minutes}m{seconds:02}s"),
        (hours, minutes, _) => format!("{hours}h{minutes:02}m"),
    }
}

/// What the run has to tell the reader, gathered as it happens.
///
/// Collected once and rendered twice: on stderr while the run goes, and in the
/// HTML report afterwards. A problem that only ever reached a terminal is a
/// problem nobody can attach to a pull request.
#[derive(Default)]
pub struct Diagnostics {
    problems: Vec<Problem>,
    facts: Vec<(String, String)>,
}

pub struct Problem {
    pub level: Level,
    pub message: String,
}

impl Diagnostics {
    /// Something that casts doubt on the score. Said now, and kept.
    pub fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::warn!("{message}");
        self.problems.push(Problem {
            level: Level::Warn,
            message,
        });
    }

    /// Something the reader needs in order to read the score correctly, which
    /// is not the same as something being wrong.
    pub fn note(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::info!("{message}");
        self.problems.push(Problem {
            level: Level::Info,
            message,
        });
    }

    /// A run fact. Already said by the phase that produced it, so this only
    /// records; a report you cannot reproduce is decoration.
    pub fn fact(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.facts.push((name.into(), value.into()));
    }

    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    pub fn facts(&self) -> &[(String, String)] {
        &self.facts
    }
}

#[derive(Default)]
pub struct Summary {
    detected: i64,
    survived: i64,
    error: i64,
    untestable: i64,
    pending: i64,
}

impl Summary {
    pub fn of(counts: &[(String, i64)]) -> Summary {
        let mut summary = Summary::default();
        for (status, count) in counts {
            match Status::parse(status) {
                Some(status) if status.is_detected() => summary.detected += count,
                Some(Status::Survived) => summary.survived += count,
                Some(Status::Error) => summary.error += count,
                Some(Status::Untestable) => summary.untestable += count,
                _ => summary.pending += count,
            }
        }
        summary
    }

    pub fn detected(&self) -> i64 {
        self.detected
    }

    /// Error and untestable mutants are excluded, neither got a fair trial:
    /// one broke before pytest could judge it, the other was only ever going
    /// to be judged by a test that was already red.
    ///
    /// This is also what the mutation-testing-report schema calls `valid`, so
    /// the JSON report and stdout divide by the same number.
    pub fn scored(&self) -> i64 {
        self.detected + self.survived
    }

    pub fn score(&self) -> Option<f64> {
        match self.scored() {
            0 => None,
            scored => Some(self.detected as f64 / scored as f64 * 100.0),
        }
    }

    /// Judge the run against a `--fail-under` percentage. A threshold of 0 is
    /// off; any other has to be *earned*, so a run that could not be measured
    /// or is not finished fails it rather than passing by default.
    pub fn gate(&self, threshold: f64) -> Gate {
        if threshold <= 0.0 {
            return Gate::Passed;
        }
        if self.pending > 0 {
            return Gate::Partial(self.pending);
        }
        let Some(score) = self.score() else {
            return Gate::Unmeasured;
        };
        // Compare the raw ratio, not the printed percentage, so 4 of 5 clears
        // --fail-under 80 on the number rather than on how it rounds.
        if self.detected as f64 * 100.0 >= threshold * self.scored() as f64 {
            Gate::Passed
        } else {
            Gate::Below { score, threshold }
        }
    }
}

/// What a `--fail-under` threshold makes of a finished run.
pub enum Gate {
    /// No threshold, or the score cleared it.
    Passed,
    Below {
        score: f64,
        threshold: f64,
    },
    /// Nothing was scoreable, which is also what a broken test command looks
    /// like. A tool that could not measure must never report success.
    Unmeasured,
    /// Mutants are still pending, so the score can still move.
    Partial(i64),
}

impl Gate {
    /// The one line CI reads, or `None` when the run passed.
    pub fn failure(&self) -> Option<String> {
        match self {
            Gate::Passed => None,
            Gate::Below { score, threshold } => {
                // The comparison is on the raw ratio, so a threshold set to the
                // score as the report printed it can still fail. One decimal
                // would render that as "62.2% is below 62.2%", which reads as a
                // bug rather than a verdict, so widen both until they differ.
                let places = if format!("{score:.1}") == format!("{threshold:.1}") {
                    4
                } else {
                    1
                };
                Some(format!(
                    "score {score:.places$}% is below --fail-under {threshold:.places$}%"
                ))
            }
            Gate::Unmeasured => Some(
                "no score to check against --fail-under: every mutant errored or came back \
                 untestable, and a run that could not measure anything is what a broken test \
                 command looks like"
                    .to_string(),
            ),
            Gate::Partial(pending) => Some(format!(
                "{pending} mutants still pending, so the score is partial and cannot clear \
                 --fail-under"
            )),
        }
    }
}

/// The report, on stdout, at every verbosity.
///
/// This is the program's output rather than commentary about it: two scripts in
/// `scripts/` grep these very lines for the verdict counts, so `--verbosity
/// error` must still print all of it.
pub fn print_summary(counts: &[(String, i64)], survivors: &[Mutant]) -> Summary {
    let summary = Summary::of(counts);

    // Survivors first, report last. A real codebase produces hundreds of
    // survivors, and the score is the number the reader ran the command for.
    // Printing it above the list means scrolling back for it.
    if !survivors.is_empty() {
        println!();
        println!("survivors (changes your tests never noticed):");
        for mutant in survivors {
            println!("  {mutant}");
        }
    }

    println!();
    println!("=== mutation report ===");
    for (status, count) in counts {
        // Padded before it is painted, so the column does not count escapes.
        let label = format!("{status:>10}");
        match Status::parse(status) {
            Some(status) => println!("{}: {count}", Paint::of(status).on(&label)),
            None => println!("{label}: {count}"),
        }
    }
    if let Some(score) = summary.score() {
        println!(
            "{}",
            Paint::Bold.on(&format!(
                "     score: {score:.1}% ({}/{} detected)",
                summary.detected(),
                summary.scored()
            ))
        );
    }
    if summary.error > 0 {
        println!(
            "{}",
            Paint::Red.on(&format!(
                "note: {} error mutants sit outside the score, a broken test command also looks like this, so check one before trusting the numbers",
                summary.error
            ))
        );
    }
    if summary.untestable > 0 {
        println!(
            "note: {} untestable mutants sit outside the score, the only tests covering them were already failing before any mutant was planted",
            summary.untestable
        );
    }
    if summary.pending > 0 {
        println!(
            "note: {} mutants still pending, re-run `angelo exec` to resume",
            summary.pending
        );
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, i64)]) -> Vec<(String, i64)> {
        pairs.iter().map(|(s, n)| (s.to_string(), *n)).collect()
    }

    #[test]
    fn timeouts_count_as_detected() {
        let summary = Summary::of(&counts(&[("killed", 3), ("timeout", 1), ("survived", 4)]));
        assert_eq!(summary.detected(), 4);
        assert_eq!(summary.scored(), 8);
        assert_eq!(summary.score(), Some(50.0));
    }

    #[test]
    fn errors_stay_out_of_the_score() {
        let summary = Summary::of(&counts(&[("killed", 1), ("survived", 1), ("error", 98)]));
        assert_eq!(summary.scored(), 2);
        assert_eq!(summary.score(), Some(50.0));
        assert_eq!(summary.error, 98);
    }

    #[test]
    fn a_run_of_only_errors_has_no_score() {
        let summary = Summary::of(&counts(&[("error", 5)]));
        assert_eq!(summary.score(), None);
    }

    #[test]
    fn unknown_statuses_count_as_pending() {
        let summary = Summary::of(&counts(&[("pending", 2), ("something_new", 1)]));
        assert_eq!(summary.pending, 3);
    }

    /// A mutant whose only tests were already red never got a fair trial, so
    /// scoring it either way would be an invented number.
    #[test]
    fn untestable_mutants_stay_out_of_the_score() {
        let summary = Summary::of(&counts(&[
            ("killed", 3),
            ("survived", 1),
            ("untestable", 96),
        ]));
        assert_eq!(summary.scored(), 4);
        assert_eq!(summary.score(), Some(75.0));
        assert_eq!(summary.untestable, 96);
        assert_eq!(summary.pending, 0, "untestable is settled, not pending");
    }

    /// `indicatif` draws the bar now, but the counts beside it are still ours.
    #[test]
    fn the_counts_beside_the_bar_read_correctly() {
        let quarter = counts_message(250, 1000, 200, 50, Duration::from_secs(30));
        assert_eq!(quarter, "detected 200  survived 50  ~1m30s left");
    }

    /// A run settles its mutants before the first redraw, and a scope that
    /// filtered everything out leaves nothing to divide by.
    #[test]
    fn an_empty_or_finished_run_does_not_divide_by_zero() {
        assert!(counts_message(0, 0, 0, 0, Duration::ZERO).contains("~-- left"));
        assert!(counts_message(9, 9, 9, 0, Duration::from_secs(1)).contains("~-- left"));
    }

    /// 30 seconds bought 250 of 1000, so 750 want 90 more.
    #[test]
    fn the_estimate_extrapolates_from_what_is_done() {
        assert_eq!(remaining(250, 1000, Duration::from_secs(30)), "1m30s");
        assert_eq!(remaining(0, 1000, Duration::from_secs(30)), "--");
    }

    /// The decision, not the escape codes. A redirected run is what CI reads
    /// and what `verdict-matrix.sh` greps, so it has to come out plain.
    #[test]
    fn colour_only_goes_to_a_terminal() {
        assert!(colour_wanted(true, false));
        assert!(!colour_wanted(false, false), "a redirected run stays plain");
        assert!(!colour_wanted(true, true), "NO_COLOR beats a terminal");
        assert!(!colour_wanted(false, true));
    }

    #[test]
    fn every_status_reads_in_its_own_colour() {
        assert_eq!(Paint::of(Status::Killed), Paint::Green);
        assert_eq!(Paint::of(Status::Timeout), Paint::Green);
        assert_eq!(Paint::of(Status::Survived), Paint::Yellow);
        assert_eq!(Paint::of(Status::Error), Paint::Red);
        assert_eq!(Paint::of(Status::Untestable), Paint::Dim);
    }

    #[test]
    fn durations_read_compactly() {
        assert_eq!(compact(Duration::from_secs(43)), "43s");
        assert_eq!(compact(Duration::from_secs(258)), "4m18s");
        assert_eq!(compact(Duration::from_secs(7523)), "2h05m");
    }

    /// A problem has to reach the report, not only the terminal it scrolled off.
    #[test]
    fn diagnostics_keep_what_they_say() {
        let mut diagnostics = Diagnostics::default();
        diagnostics.warn("the baseline is red");
        diagnostics.note("41 mutants were sampled away");
        diagnostics.fact("workers", "8");

        assert_eq!(diagnostics.problems().len(), 2);
        assert_eq!(diagnostics.problems()[0].level, Level::Warn);
        assert_eq!(diagnostics.problems()[1].level, Level::Info);
        assert_eq!(
            diagnostics.facts(),
            [("workers".to_string(), "8".to_string())]
        );
    }

    /// The whole point of the flag: below fails, above passes.
    #[test]
    fn a_threshold_judges_the_score() {
        let summary = Summary::of(&counts(&[("killed", 1), ("survived", 4)]));
        assert!(summary.gate(80.0).failure().is_some());
        assert!(summary.gate(10.0).failure().is_none());
    }

    /// 4 of 5 is exactly 80%, and it must not fail on a rounding artefact.
    #[test]
    fn an_exact_match_clears_the_threshold() {
        let summary = Summary::of(&counts(&[("killed", 4), ("survived", 1)]));
        assert!(summary.gate(80.0).failure().is_none());
        assert!(summary.gate(80.1).failure().is_some());
    }

    #[test]
    fn zero_means_no_threshold() {
        let summary = Summary::of(&counts(&[("survived", 100)]));
        assert!(summary.gate(0.0).failure().is_none());
    }

    /// An all-error run has no score, and no score is not a pass.
    #[test]
    fn an_unmeasurable_run_fails_a_threshold() {
        let summary = Summary::of(&counts(&[("error", 5)]));
        assert_eq!(summary.score(), None);
        assert!(summary.gate(1.0).failure().is_some());
        assert!(summary.gate(0.0).failure().is_none());
    }

    /// The pending mutants could all survive, so a partial score proves nothing.
    #[test]
    fn a_partial_run_fails_a_threshold_it_would_otherwise_clear() {
        let summary = Summary::of(&counts(&[("killed", 10), ("pending", 1)]));
        assert_eq!(summary.score(), Some(100.0));
        assert!(summary.gate(50.0).failure().is_some());
    }

    #[test]
    fn the_failure_line_names_both_numbers() {
        let summary = Summary::of(&counts(&[("killed", 46), ("survived", 28)]));
        assert_eq!(
            summary.gate(80.0).failure().unwrap(),
            "score 62.2% is below --fail-under 80.0%"
        );
    }

    /// 46 of 74 prints as 62.2% but is really 62.16%, so a threshold typed
    /// straight off the report does fail. It must not say "62.2% is below
    /// 62.2%" while doing so.
    #[test]
    fn a_near_miss_widens_until_the_numbers_differ() {
        let summary = Summary::of(&counts(&[("killed", 46), ("survived", 28)]));
        assert_eq!(
            summary.gate(62.2).failure().unwrap(),
            "score 62.1622% is below --fail-under 62.2000%"
        );
    }
}
