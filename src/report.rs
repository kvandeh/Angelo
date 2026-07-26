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

    /// Error mutants are excluded — they never got a fair trial.
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
            "note: {} error mutants sit outside the score — a broken test command also looks like this, so check one before trusting the numbers",
            summary.error
        );
    }
    if summary.pending > 0 {
        println!(
            "note: {} mutants still pending — re-run `angelo exec` to resume",
            summary.pending
        );
    }

    if survivors.is_empty() {
        return;
    }
    println!();
    println!("survivors (changes your tests never noticed):");
    for mutant in survivors {
        println!("  {mutant}");
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
}
