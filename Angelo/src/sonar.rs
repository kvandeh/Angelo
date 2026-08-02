//! SonarQube's generic issue import format.
//!
//! `--report` already reaches SonarQube: Stryker's own jq filter converts the
//! mutation-testing-report schema into this format, and reusing a converter
//! somebody else maintains was the better trade right up until it was measured.
//! SonarQube 26.7 imports that filter's output and says this while doing it:
//!
//! ```text
//! WARN External issues were imported with a deprecated format which will be
//! removed in a later version of SonarQube. The "rules" field is missing.
//! ```
//!
//! So Angelo writes the current shape itself. A `rules` array declaring
//! `cleanCodeAttribute` and `impacts` is the whole difference, and it removes
//! the jq step along with the deprecation.
//!
//! **Nothing is installed on the server.** SonarQube registers the two rules
//! below as *external* rules from this file alone, which is also why they never
//! appear in a quality profile.
//!
//! Hand-rolled like `stryker.rs`, and for the same reason: one fixed shape is a
//! `format!` and an escape function, not a general serialiser.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::db::Settled;
use crate::mutate::{Lines, NO_COVERAGE_RULE, SURVIVED_RULE};
use crate::stryker::quoted;

/// Sonar's own id for us, and what its issues are grouped under.
const ENGINE: &str = "Angelo";

/// Ten minutes to add one assertion. The number is Stryker's, kept so that a
/// project switching off the jq route sees no change in its remediation effort.
const EFFORT_MINUTES: u32 = 10;

/// `TESTED` is one of Sonar's own clean-code attributes and means exactly what a
/// survivor reports, so no interpretation is invented here.
const ATTRIBUTE: &str = "TESTED";

/// Write the report, and change nothing about the run that produced it.
pub fn write(path: &Path, settled: &[Settled]) -> Result<()> {
    let document = render(settled);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {} for the SonarQube report", parent.display()))?;
    }
    fs::write(path, document)
        .with_context(|| format!("writing the SonarQube report to {}", path.display()))
}

fn render(settled: &[Settled]) -> String {
    let mut document = String::from("{\n  \"rules\": [\n");
    document.push_str(&rule(
        SURVIVED_RULE,
        "Mutant survived",
        "A fault was planted in this code and the test suite still passed. A test executes \
         this line but asserts nothing about its behaviour. Add an assertion.",
        "MEDIUM",
    ));
    document.push_str(",\n");
    document.push_str(&rule(
        NO_COVERAGE_RULE,
        "Mutant not covered by any test",
        "A fault was planted in this code and no test executed it at all. This is a gap in \
         the suite rather than a weak assertion. Write a test.",
        "HIGH",
    ));
    document.push_str("\n  ],\n  \"issues\": [\n");

    let mut first = true;
    for (path, mutants) in raisers(settled) {
        // One read and one cursor per file. `Lines` only ever moves forward, so
        // it is worth nothing unless a file's mutants share it, and reading per
        // mutant cost a whole file read per survivor.
        let source = fs::read_to_string(&path).unwrap_or_default();
        let mut lines = Lines::default();
        for entry in mutants {
            let Some(rendered) = issue(entry, &path, &source, &mut lines) else {
                continue;
            };
            if !first {
                document.push_str(",\n");
            }
            first = false;
            document.push_str(&rendered);
        }
    }
    document.push_str("\n  ]\n}\n");
    document
}

/// The mutants that raise an issue, grouped by file.
///
/// A `BTreeMap` so the same run always writes the same bytes and two reports
/// can be diffed, which is what `stryker.rs` does for the same reason. Filtering
/// here rather than in the loop keeps a file nothing survived in from being read
/// at all.
fn raisers(settled: &[Settled]) -> BTreeMap<String, Vec<&Settled>> {
    let mut files: BTreeMap<String, Vec<&Settled>> = BTreeMap::new();
    for entry in settled {
        if entry.raises().is_some() {
            files
                .entry(entry.mutant.report_path())
                .or_default()
                .push(entry);
        }
    }
    files
}

/// `type` and `severity` are deliberately absent. They are the pre-10.x pair,
/// optional once `impacts` is given, and writing both would reintroduce the
/// half of the format this module exists to leave behind.
fn rule(id: &str, name: &str, description: &str, severity: &str) -> String {
    format!(
        "    {{ \"id\": {}, \"name\": {}, \"description\": {}, \"engineId\": {}, \
         \"cleanCodeAttribute\": {}, \"impacts\": [{{ \"softwareQuality\": \"MAINTAINABILITY\", \
         \"severity\": \"{severity}\" }}] }}",
        quoted(id),
        quoted(name),
        quoted(description),
        quoted(ENGINE),
        quoted(ATTRIBUTE),
    )
}

/// One issue, given the file's text and the cursor walking it.
///
/// The source is passed in rather than read here: a file deleted since the run
/// still has rows in the database, and reporting it with an empty source beats
/// losing the whole document over it. Sonar drops an issue whose path matches
/// no file anyway.
fn issue(entry: &Settled, path: &str, source: &str, lines: &mut Lines) -> Option<String> {
    let rule_id = entry.raises()?;
    let mutant = &entry.mutant;
    let start = lines.position(source, mutant.byte_start);
    let end = lines.position(source, mutant.byte_end);

    let mutation = format!(
        "The {} was mutated to {} without any tests failing.",
        mutant.mutator(),
        mutant.replacement
    );
    let message = match rule_id {
        NO_COVERAGE_RULE => format!("A mutant was not covered by any of the tests. {mutation}"),
        _ => format!("A mutant survived after running the tests. {mutation}"),
    };

    // Sonar counts columns from 0 where the mutation-testing schema counts from
    // 1, so every column loses one. Lines are 1-based in both and do not.
    Some(format!(
        "    {{ \"ruleId\": {}, \"effortMinutes\": {EFFORT_MINUTES}, \"primaryLocation\": \
         {{ \"message\": {}, \"filePath\": {}, \"textRange\": {{ \"startLine\": {}, \
         \"startColumn\": {}, \"endLine\": {}, \"endColumn\": {} }} }} }}",
        quoted(rule_id),
        quoted(&message),
        quoted(path),
        start.line,
        start.column - 1,
        end.line,
        end.column - 1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate::{Mutant, Status};
    use std::path::PathBuf;

    /// `a = 1 + 2`, with the mutant on the `+` at byte 6.
    const SOURCE: &str = "a = 1 + 2\n";

    /// The pure half of rendering: no file is read, so a test states its own
    /// source rather than depending on one existing on disk.
    fn rendered(entry: &Settled) -> Option<String> {
        issue(entry, "calc.py", SOURCE, &mut Lines::default())
    }

    fn settled(status: Option<Status>, executed: bool) -> Settled {
        Settled {
            mutant: Mutant {
                id: 7,
                file: PathBuf::from("calc.py"),
                line: 1,
                byte_start: 6,
                byte_end: 7,
                original: "+".to_string(),
                replacement: "-".to_string(),
            },
            status,
            executed,
            duration_ms: Some(120),
            failed_tests: vec![],
        }
    }

    /// The `rules` array is the entire reason this module exists: without it
    /// SonarQube imports the issues and warns that the format is going away.
    #[test]
    fn both_rules_are_declared_in_the_new_shape() {
        let document = render(&[settled(Some(Status::Survived), true)]);
        assert!(document.contains(r#""id": "MutantSurvived""#), "{document}");
        assert!(document.contains(r#""id": "MutantNoCoverage""#));
        assert!(document.contains(r#""cleanCodeAttribute": "TESTED""#));
        assert!(document.contains(r#""softwareQuality": "MAINTAINABILITY""#));
        assert!(document.contains(r#""engineId": "Angelo""#));
    }

    /// The deprecated pair, which is optional once `impacts` is present.
    /// Writing them would bring back the half of the format being left behind.
    #[test]
    fn the_pre_ten_severity_pair_is_absent() {
        let document = render(&[settled(Some(Status::Survived), true)]);
        assert!(!document.contains(r#""type""#), "{document}");
        assert!(!document.contains(r#""severity": "MAJOR""#));
    }

    #[test]
    fn a_covered_survivor_and_an_uncovered_one_get_different_rules() {
        let covered = rendered(&settled(Some(Status::Survived), true)).expect("an issue");
        assert!(
            covered.contains(r#""ruleId": "MutantSurvived""#),
            "{covered}"
        );
        assert!(covered.contains("A mutant survived after running the tests."));

        let uncovered = rendered(&settled(Some(Status::Survived), false)).expect("an issue");
        assert!(uncovered.contains(r#""ruleId": "MutantNoCoverage""#));
        assert!(uncovered.contains("A mutant was not covered by any of the tests."));
    }

    /// Everything a developer cannot act on in their own file stays out. An
    /// all-`error` run therefore raises nothing, which is exactly why the docs
    /// insist on `--fail-under` alongside this.
    #[test]
    fn only_survivors_become_issues() {
        for status in [
            Status::Killed,
            Status::Timeout,
            Status::Error,
            Status::Untestable,
        ] {
            assert!(
                rendered(&settled(Some(status), true)).is_none(),
                "{status:?} should raise no issue"
            );
        }
        assert!(
            rendered(&settled(None, false)).is_none(),
            "pending raises none"
        );
    }

    /// Sonar counts columns from zero where the mutation-testing schema counts
    /// from one. The `+` is byte 6 of `a = 1 + 2`, which the schema calls
    /// column 7, so Sonar calls it column 6. An off-by-one here lands the
    /// squiggle on the wrong token.
    #[test]
    fn columns_are_zero_based_for_sonar() {
        let issue = rendered(&settled(Some(Status::Survived), true)).expect("an issue");
        assert!(
            issue.contains(
                r#""textRange": { "startLine": 1, "startColumn": 6, "endLine": 1, "endColumn": 7 }"#
            ),
            "{issue}"
        );
    }

    /// Sonar resolves `filePath` against the scanner's base directory by
    /// literal match, so a Windows separator resolves to no file and the issue
    /// is dropped without an error.
    #[test]
    fn file_paths_are_forward_slashed() {
        let mut entry = settled(Some(Status::Survived), true);
        entry.mutant.file = PathBuf::from(".\\src\\calc.py");
        let issue = issue(
            &entry,
            &entry.mutant.report_path(),
            SOURCE,
            &mut Lines::default(),
        )
        .expect("an issue");
        assert!(issue.contains(r#""filePath": "src/calc.py""#), "{issue}");
    }

    /// A run with nothing to report is still a valid document, and the rules
    /// still have to be declared or the file is malformed.
    #[test]
    fn a_run_with_no_survivors_still_writes_a_valid_document() {
        let document = render(&[settled(Some(Status::Killed), true)]);
        assert!(document.contains(r#""issues": ["#), "{document}");
        assert!(document.trim_end().ends_with('}'));
    }
}
