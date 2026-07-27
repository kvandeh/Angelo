use std::collections::HashMap;
use std::env;
use std::io::{self, IsTerminal, Write};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::mutate::{Mutant, Status};
use crate::runner::BatchOutcome;

/// Past this many mutants a line each stops being a report and starts being a
/// wall, so the lines collapse into one redrawn bar.
const BAR_ABOVE: usize = 1000;

/// Characters of bar, chosen to leave room for the counts on an 80-column
/// terminal.
const BAR_WIDTH: usize = 36;

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

/// Owns the live progress: one line per settled mutant, or one redrawn bar.
pub struct Progress {
    /// Empty in bar mode, which never prints a label and so never builds one.
    labels: HashMap<i64, String>,
    total: usize,
    done: usize,
    detected: usize,
    survived: usize,
    /// None means a line per mutant. Some is bar mode, holding the instant the
    /// run started, which is the only thing an estimate can be built from.
    bar_since: Option<Instant>,
    /// Characters the last bar left behind, so they can be wiped.
    drawn: usize,
}

impl Progress {
    pub fn new(mutants: &[Mutant], show_loading: bool) -> Progress {
        let bar = show_loading || mutants.len() > BAR_ABOVE;
        Progress {
            labels: match bar {
                true => HashMap::new(),
                false => mutants.iter().map(|m| (m.id, m.to_string())).collect(),
            },
            total: mutants.len(),
            done: 0,
            detected: 0,
            survived: 0,
            bar_since: bar.then(Instant::now),
            drawn: 0,
        }
    }

    pub fn print(&mut self, outcome: &BatchOutcome) {
        let seconds = outcome.duration_ms as f64 / 1000.0;
        let batch_note = if outcome.verdicts.len() > 1 {
            format!("  [batch of {}]", outcome.verdicts.len())
        } else {
            String::new()
        };
        for (mutant_id, status) in &outcome.verdicts {
            self.done += 1;
            if status.is_detected() {
                self.detected += 1;
            }
            if *status == Status::Survived {
                self.survived += 1;
            }
            if self.bar_since.is_some() {
                continue;
            }
            let fallback = format!("mutant #{mutant_id}");
            let label = self.labels.get(mutant_id).unwrap_or(&fallback);
            println!(
                "[{}/{}] {label}  {}{batch_note}  ({seconds:.1}s)",
                self.done,
                self.total,
                Paint::of(*status).on(status.as_str())
            );
        }
        if let Some(error) = &outcome.error {
            // "A run where every mutant dies almost instantly means a broken
            // test command" is the loudest thing angelo has to say, and this is
            // how it says it. The bar never swallows one.
            self.erase();
            println!("          {error}");
        }
        self.redraw();
    }

    /// Leave the finished bar on screen and move off its line.
    pub fn finish(&mut self) {
        if self.drawn > 0 {
            println!();
            self.drawn = 0;
        }
    }

    fn redraw(&mut self) {
        let Some(started) = self.bar_since else {
            return;
        };
        let line = bar_line(
            self.done,
            self.total,
            self.detected,
            self.survived,
            started.elapsed(),
        );
        // Measured before it is painted: `erase` wipes character by character,
        // and escape codes take up none of the width they would be counted for.
        self.drawn = line.chars().count();
        print!("\r{}", painted(&line));
        let _ = io::stdout().flush();
    }

    fn erase(&mut self) {
        if self.drawn == 0 {
            return;
        }
        print!("\r{:width$}\r", "", width = self.drawn);
        self.drawn = 0;
    }
}

/// The redrawn line. Pure, so the arithmetic gets a unit test rather than a
/// screenshot.
fn bar_line(
    done: usize,
    total: usize,
    detected: usize,
    survived: usize,
    elapsed: Duration,
) -> String {
    let total = total.max(1);
    let filled = BAR_WIDTH * done.min(total) / total;
    format!(
        "  [{}{}] {:>3}%  {done}/{total}  detected {detected}  survived {survived}  ~{} left",
        "#".repeat(filled),
        "-".repeat(BAR_WIDTH - filled),
        100 * done.min(total) / total,
        remaining(done, total, elapsed)
    )
}

/// The bar's filled run in green. The line is built plain and painted after,
/// so `bar_line` stays a pure string its unit test can measure.
fn painted(line: &str) -> String {
    let (Some(start), Some(end)) = (line.find('#'), line.rfind('#')) else {
        return line.to_string();
    };
    format!(
        "{}{}{}",
        &line[..start],
        Paint::Green.on(&line[start..=end]),
        &line[end + 1..]
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
fn compact(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match (seconds / 3600, (seconds % 3600) / 60, seconds % 60) {
        (0, 0, seconds) => format!("{seconds}s"),
        (0, minutes, seconds) => format!("{minutes}m{seconds:02}s"),
        (hours, minutes, _) => format!("{hours}h{minutes:02}m"),
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

    fn detected(&self) -> i64 {
        self.detected
    }

    /// Error and untestable mutants are excluded, neither got a fair trial:
    /// one broke before pytest could judge it, the other was only ever going
    /// to be judged by a test that was already red.
    fn scored(&self) -> i64 {
        self.detected + self.survived
    }

    fn score(&self) -> Option<f64> {
        match self.scored() {
            0 => None,
            scored => Some(self.detected as f64 / scored as f64 * 100.0),
        }
    }
}

pub fn print_summary(counts: &[(String, i64)], survivors: &[Mutant]) {
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

    #[test]
    fn the_bar_fills_with_progress() {
        let quarter = bar_line(250, 1000, 200, 50, Duration::from_secs(30));
        assert!(quarter.contains(" 25%"), "{quarter}");
        assert!(quarter.contains("250/1000"));
        assert!(quarter.contains("detected 200  survived 50"));
        assert_eq!(quarter.matches('#').count(), BAR_WIDTH / 4);
        assert_eq!(quarter.matches('-').count(), BAR_WIDTH * 3 / 4);

        let full = bar_line(1000, 1000, 900, 100, Duration::from_secs(120));
        assert!(full.contains("100%"));
        assert_eq!(full.matches('#').count(), BAR_WIDTH);
    }

    /// A run settles its mutants before the first redraw, and a scope that
    /// filtered everything out leaves nothing to divide by.
    #[test]
    fn an_empty_or_finished_bar_does_not_divide_by_zero() {
        assert!(bar_line(0, 0, 0, 0, Duration::ZERO).contains("  0%"));
        assert!(bar_line(0, 0, 0, 0, Duration::ZERO).contains("~-- left"));
        assert!(bar_line(9, 9, 9, 0, Duration::from_secs(1)).contains("~-- left"));
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

    /// Painting the bar must not change what `erase` has to wipe, so with
    /// colour off the line comes back identical rather than merely similar.
    #[test]
    fn painting_an_uncoloured_bar_changes_nothing() {
        let line = bar_line(250, 1000, 200, 50, Duration::from_secs(30));
        assert_eq!(painted(&line).len(), line.len() + escape_width(&line));
    }

    /// Under `cargo test` stdout is captured, so colour is normally off; run
    /// with `--nocapture` from a terminal and it is on. The test has to hold
    /// either way, so it asks how wide the escapes are rather than assuming.
    fn escape_width(line: &str) -> usize {
        match colour_is_on() && line.contains('#') {
            true => Paint::Green.code().len() + "\x1b[0m".len(),
            false => 0,
        }
    }

    #[test]
    fn durations_read_compactly() {
        assert_eq!(compact(Duration::from_secs(43)), "43s");
        assert_eq!(compact(Duration::from_secs(258)), "4m18s");
        assert_eq!(compact(Duration::from_secs(7523)), "2h05m");
    }
}
