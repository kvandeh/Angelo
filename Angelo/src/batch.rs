use std::collections::HashSet;

use crate::coverage::{Coverage, TestCoverage};
use crate::mutate::Mutant;
use crate::schemata::{Host, Schemata};

/// Mutants that share no covering test, so one pytest run can judge them all.
pub struct Batch {
    pub mutants: Vec<Mutant>,
    covering_tests: HashSet<i64>,
    /// The functions this batch already switches. Empty on the splice path,
    /// where a file holds one mutant at a time and the question cannot arise.
    hosts: HashSet<Host>,
}

impl Batch {
    /// A closed batch: import-time or coverage-less mutants never get company.
    fn solo(mutant: Mutant) -> Batch {
        Batch {
            mutants: vec![mutant],
            covering_tests: HashSet::new(),
            hosts: HashSet::new(),
        }
    }

    fn open(mutant: Mutant, covering_tests: HashSet<i64>, host: Option<Host>) -> Batch {
        Batch {
            mutants: vec![mutant],
            covering_tests,
            hosts: host.into_iter().collect(),
        }
    }

    fn accepts(
        &self,
        mutant: &Mutant,
        covering: &HashSet<i64>,
        capacity: usize,
        host: Option<Host>,
    ) -> bool {
        !self.covering_tests.is_empty()
            && self.mutants.len() < capacity
            && self.covering_tests.is_disjoint(covering)
            && host.is_none_or(|host| !self.hosts.contains(&host))
            && !self.overlaps(mutant)
    }

    /// Whether some member already rewrites bytes this mutant rewrites.
    ///
    /// Two token mutants never overlap, because two tokens never do. Deletion
    /// and condition replacement rewrite whole nodes, so a batch can be offered
    /// the deletion of `return a + b` and the swap of its `+`. Splicing applies
    /// a batch back to front, and the outer edit would then be measured against
    /// offsets the inner one had already moved: at best a corrupt file, at
    /// worst a panic on a byte that is no longer a character boundary.
    fn overlaps(&self, mutant: &Mutant) -> bool {
        self.mutants.iter().any(|member| {
            member.file == mutant.file
                && member.byte_start < mutant.byte_end
                && mutant.byte_start < member.byte_end
        })
    }

    fn add(&mut self, mutant: Mutant, covering: HashSet<i64>, host: Option<Host>) {
        self.mutants.push(mutant);
        self.covering_tests.extend(covering);
        self.hosts.extend(host);
    }
}

/// First-fit: each mutant joins the first batch with room and no conflict.
///
/// `schemata` is the tree the run will switch mutants on rather than splice
/// them into, when there is one. Two mutants of the same function conflict
/// there however disjoint their tests are, because the wrapper can only call
/// one copy: the second would never take effect and would be scored a survivor.
///
/// `placed` is called once per mutant. Composing is not free — every mutant is
/// tried against every open batch, and the disjointness check is over sets of
/// test ids — so a large pool spends visible time here and something has to
/// move on screen while it does.
pub fn compose(
    mutants: Vec<Mutant>,
    coverage: Option<&Coverage>,
    batch_size: usize,
    schemata: Option<&Schemata>,
    placed: impl Fn(),
) -> Vec<Batch> {
    let mut batches: Vec<Batch> = Vec::new();
    for mutant in mutants {
        placed();
        let host = schemata.and_then(|schemata| schemata.host(&mutant));
        let covering = coverage.map(|coverage| coverage.classify(&mutant));
        let Some(TestCoverage::Tested(covering)) = covering else {
            batches.push(Batch::solo(mutant));
            continue;
        };
        let fits = batches
            .iter_mut()
            .find(|batch| batch.accepts(&mutant, &covering, batch_size, host));
        match fits {
            Some(batch) => batch.add(mutant, covering, host),
            None => batches.push(Batch::open(mutant, covering, host)),
        }
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CoverageRow;
    use std::path::PathBuf;

    /// One byte each, and no two on the same one: these mutants are about
    /// coverage, and two that overlapped would be refused for a reason the
    /// test is not making.
    fn mutant(id: i64, line: u32) -> Mutant {
        Mutant {
            id,
            file: PathBuf::from("calc.py"),
            line,
            byte_start: id as usize,
            byte_end: id as usize + 1,
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
            None,
            || {},
        );
        assert_eq!(sizes(&batches), [2]);
    }

    /// Deletion and condition replacement rewrite whole nodes, so a batch can
    /// be offered two mutants of the same bytes. Splicing applies a batch back
    /// to front, and the outer edit would land on offsets the inner one had
    /// already moved.
    #[test]
    fn two_mutants_of_the_same_bytes_never_share_a_batch() {
        let deletion = Mutant {
            byte_start: 1,
            byte_end: 20,
            original: "return a + b".to_string(),
            replacement: "pass".to_string(),
            ..mutant(1, 1)
        };
        let inside = Mutant {
            byte_start: 9,
            byte_end: 10,
            ..mutant(2, 2)
        };
        let batches = compose(vec![deletion, inside], Some(&coverage()), 8, None, || {});
        assert_eq!(sizes(&batches), [1, 1]);
    }

    /// The guard is about bytes, not about files: two files' mutants may hold
    /// the same offsets and still splice cleanly. Asked of the predicate
    /// directly, since coverage is what decides whether two mutants are offered
    /// to each other at all and this is not a question about coverage.
    #[test]
    fn the_same_offsets_in_two_files_do_not_overlap() {
        let batch = Batch::solo(mutant(1, 1));
        let same_file = Mutant {
            byte_start: 1,
            byte_end: 2,
            ..mutant(2, 2)
        };
        assert!(batch.overlaps(&same_file));

        let other_file = Mutant {
            file: PathBuf::from("other.py"),
            byte_start: 1,
            byte_end: 2,
            ..mutant(3, 3)
        };
        assert!(!batch.overlaps(&other_file));
    }

    #[test]
    fn a_shared_test_is_a_conflict() {
        let batches = compose(
            vec![mutant(1, 1), mutant(2, 3)],
            Some(&coverage()),
            8,
            None,
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
            None,
            || {},
        );
        assert_eq!(sizes(&batches), [1, 1]);
    }

    #[test]
    fn no_coverage_means_no_batching() {
        let batches = compose(vec![mutant(1, 1), mutant(2, 2)], None, 8, None, || {});
        assert_eq!(sizes(&batches), [1, 1]);
    }

    /// Disjoint tests are not enough on the schemata path. The wrapper calls
    /// one copy of the function, so a second mutant of the same function never
    /// takes effect and comes back a survivor. Splicing has no such limit,
    /// which is why the same two mutants batch together without schemata.
    #[test]
    fn two_mutants_of_one_function_do_not_share_a_run() {
        //         0            13       23      27          40
        let source = "def f(a, b):\n    x = a + b\n    return x * a\n";
        let file = std::env::temp_dir().join("angelo-batch-host-test.py");
        std::fs::write(&file, source).expect("writing the fixture");
        let name = file.display().to_string().replace('\\', "/");

        let in_f = || {
            vec![
                Mutant {
                    id: 1,
                    file: file.clone(),
                    line: 2,
                    byte_start: 23,
                    byte_end: 24,
                    original: "+".to_string(),
                    replacement: "-".to_string(),
                },
                Mutant {
                    id: 2,
                    file: file.clone(),
                    line: 3,
                    byte_start: 40,
                    byte_end: 41,
                    original: "*".to_string(),
                    replacement: "/".to_string(),
                },
            ]
        };
        let schemata = Schemata::build("".as_ref(), &in_f()).expect("building schemata");
        let _ = std::fs::remove_file(&file);
        assert_eq!(schemata.hosted_count(), 2, "both live in the same function");

        // Line 2 is test_a's, line 3 is test_b's: no test covers both.
        let coverage = Coverage::build(vec![
            CoverageRow {
                file: name.clone(),
                context_id: 1,
                context: "test_calc.test_a".to_string(),
                numbits: vec![0b0000_0100],
            },
            CoverageRow {
                file: name,
                context_id: 2,
                context: "test_calc.test_b".to_string(),
                numbits: vec![0b0000_1000],
            },
        ]);

        let spliced = compose(in_f(), Some(&coverage), 8, None, || {});
        assert_eq!(sizes(&spliced), [2], "splicing can hold both at once");

        let hosted = compose(in_f(), Some(&coverage), 8, Some(&schemata), || {});
        assert_eq!(sizes(&hosted), [1, 1], "switching can only hold one");
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
        let batches = compose(mutants, Some(&coverage), 2, None, || {});
        assert_eq!(sizes(&batches), [2, 2, 1]);
    }
}
