use std::collections::HashMap;

use crate::mutate::{Mutant, Status};
use crate::runner::BatchOutcome;

/// Owns the live progress lines: one line per settled mutant.
pub struct Progress {
    labels: HashMap<i64, String>,
    total: usize,
    done: usize,
}

impl Progress {
    pub fn new(mutants: &[Mutant]) -> Progress {
        Progress {
            labels: mutants.iter().map(|m| (m.id, m.to_string())).collect(),
            total: mutants.len(),
            done: 0,
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
            let fallback = format!("mutant #{mutant_id}");
            let label = self.labels.get(mutant_id).unwrap_or(&fallback);
            println!(
                "[{}/{}] {label}  {}{batch_note}  ({seconds:.1}s)",
                self.done,
                self.total,
                status.as_str()
            );
        }
        if let Some(error) = &outcome.error {
            println!("          {error}");
        }
    }
}

#[derive(Default)]
pub struct Summary {
    detected: i64,
    survived: i64,
    error: i64,
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
                _ => summary.pending += count,
            }
        }
        summary
    }

    fn detected(&self) -> i64 {
        self.detected
    }

    /// Error mutants are excluded, they never got a fair trial.
    fn scored(&self) -> i64 {
        self.detected + self.survived
    }

    fn score(&self) -> Option<f64> {
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
                "no score to check against --fail-under: every mutant errored, which is what a \
                 broken test command looks like"
                    .to_string(),
            ),
            Gate::Partial(pending) => Some(format!(
                "{pending} mutants still pending, so the score is partial and cannot clear \
                 --fail-under"
            )),
        }
    }
}

pub fn print_summary(counts: &[(String, i64)], survivors: &[Mutant]) -> Summary {
    let summary = Summary::of(counts);

    println!();
    println!("=== mutation report ===");
    for (status, count) in counts {
        println!("{status:>9}: {count}");
    }
    if let Some(score) = summary.score() {
        println!(
            "    score: {score:.1}% ({}/{} detected)",
            summary.detected(),
            summary.scored()
        );
    }
    if summary.error > 0 {
        println!(
            "note: {} error mutants sit outside the score, a broken test command also looks like this, so check one before trusting the numbers",
            summary.error
        );
    }
    if summary.pending > 0 {
        println!(
            "note: {} mutants still pending, re-run `angelo exec` to resume",
            summary.pending
        );
    }

    if survivors.is_empty() {
        return summary;
    }
    println!();
    println!("survivors (changes your tests never noticed):");
    for mutant in survivors {
        println!("  {mutant}");
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
