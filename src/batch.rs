use std::collections::HashSet;

use crate::coverage::{Coverage, TestCoverage};
use crate::mutate::Mutant;

/// Mutants that share no covering test, so one pytest run can judge them all.
pub struct Batch {
    pub mutants: Vec<Mutant>,
    covering_tests: HashSet<i64>,
}

impl Batch {
    /// A closed batch: import-time or coverage-less mutants never get company.
    fn solo(mutant: Mutant) -> Batch {
        Batch {
            mutants: vec![mutant],
            covering_tests: HashSet::new(),
        }
    }

    fn open(mutant: Mutant, covering_tests: HashSet<i64>) -> Batch {
        Batch {
            mutants: vec![mutant],
            covering_tests,
        }
    }

    fn accepts(&self, covering: &HashSet<i64>, capacity: usize) -> bool {
        !self.covering_tests.is_empty()
            && self.mutants.len() < capacity
            && self.covering_tests.is_disjoint(covering)
    }

    fn add(&mut self, mutant: Mutant, covering: HashSet<i64>) {
        self.mutants.push(mutant);
        self.covering_tests.extend(covering);
    }
}

/// First-fit: each mutant joins the first batch with room and no conflict.
///
/// `placed` is called once per mutant. Composing is not free — every mutant is
/// tried against every open batch, and the disjointness check is over sets of
/// test ids — so a large pool spends visible time here and something has to
/// move on screen while it does.
pub fn compose(
    mutants: Vec<Mutant>,
    coverage: Option<&Coverage>,
    batch_size: usize,
    placed: impl Fn(),
) -> Vec<Batch> {
    let mut batches: Vec<Batch> = Vec::new();
    for mutant in mutants {
        placed();
        let covering = coverage.map(|coverage| coverage.classify(&mutant));
        let Some(TestCoverage::Tested(covering)) = covering else {
            batches.push(Batch::solo(mutant));
            continue;
        };
        let fits = batches
            .iter_mut()
            .find(|batch| batch.accepts(&covering, batch_size));
        match fits {
            Some(batch) => batch.add(mutant, covering),
            None => batches.push(Batch::open(mutant, covering)),
        }
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CoverageRow;
    use std::path::PathBuf;

    fn mutant(id: i64, line: u32) -> Mutant {
        Mutant {
            id,
            file: PathBuf::from("calc.py"),
            line,
            byte_start: 0,
            byte_end: 1,
            original: "+".to_string(),
            replacement: "-".to_string(),
        }
    }

    /// line 1: test_a; line 2: test_b; line 3: test_a and test_b; line 5: import only.
    fn coverage() -> Coverage {
        Coverage::build(vec![
            CoverageRow {
                file: "calc.py".to_string(),
                context_id: 1,
                context: "test_calc.test_a".to_string(),
                numbits: vec![0b0000_1010],
            },
            CoverageRow {
                file: "calc.py".to_string(),
                context_id: 2,
                context: "test_calc.test_b".to_string(),
                numbits: vec![0b0000_1100],
            },
            CoverageRow {
                file: "calc.py".to_string(),
                context_id: 0,
                context: String::new(),
                numbits: vec![0b0010_0000],
            },
        ])
    }

    fn sizes(batches: &[Batch]) -> Vec<usize> {
        batches.iter().map(|batch| batch.mutants.len()).collect()
    }

    #[test]
    fn disjoint_tests_batch_together() {
        let batches = compose(
            vec![mutant(1, 1), mutant(2, 2)],
            Some(&coverage()),
            8,
            || {},
        );
        assert_eq!(sizes(&batches), [2]);
    }

    #[test]
    fn a_shared_test_is_a_conflict() {
        let batches = compose(
            vec![mutant(1, 1), mutant(2, 3)],
            Some(&coverage()),
            8,
            || {},
        );
        assert_eq!(sizes(&batches), [1, 1]);
    }

    #[test]
    fn import_time_mutants_run_alone() {
        let batches = compose(
            vec![mutant(1, 5), mutant(2, 1)],
            Some(&coverage()),
            8,
            || {},
        );
        assert_eq!(sizes(&batches), [1, 1]);
    }

    #[test]
    fn no_coverage_means_no_batching() {
        let batches = compose(vec![mutant(1, 1), mutant(2, 2)], None, 8, || {});
        assert_eq!(sizes(&batches), [1, 1]);
    }

    #[test]
    fn batch_size_caps_a_batch() {
        let coverage = Coverage::build(
            (1..=5)
                .map(|i| CoverageRow {
                    file: "calc.py".to_string(),
                    context_id: i,
                    context: format!("test_calc.test_{i}"),
                    numbits: vec![1 << i],
                })
                .collect(),
        );
        let mutants = (1..=5).map(|i| mutant(i, i as u32)).collect();
        let batches = compose(mutants, Some(&coverage), 2, || {});
        assert_eq!(sizes(&batches), [2, 2, 1]);
    }
}
