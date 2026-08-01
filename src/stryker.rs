//! The mutation-testing-report schema, version 2.
//!
//! <http://stryker-mutator.io/report.schema.json>, the format StrykerJS,
//! Stryker.NET, Stryker4s and muttest all write. Emitting it rather than
//! inventing an Angelo shape buys the existing HTML viewer and dashboard, and
//! is the documented route into SonarQube.
//!
//! The schema also already agrees with us on arithmetic: it scores
//! `detected / valid`, where detected is killed plus timeout and valid excludes
//! errors and ignored mutants. That is `Summary::score` exactly, so this module
//! reports numbers and never computes one.
//!
//! Hand-rolled rather than reached for `serde_json`: this is one fixed shape,
//! not a general serialiser, and the escaping is the only hard part.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::db::Settled;
use crate::mutate::{Lines, Position};

/// The version of the schema this writes. Not the crate's version.
const SCHEMA_VERSION: &str = "2.0";

/// The bands a viewer tints its score badge with. The schema requires them and
/// nothing else reads them, so `low` follows the project's own `fail_under`
/// when it has one: that is the number this project actually cares about.
fn thresholds(fail_under: f64) -> (u32, u32) {
    let low = match fail_under > 0.0 {
        true => fail_under.round() as u32,
        false => 60,
    };
    (low.max(80), low)
}

/// Write the report, and change nothing about the run that produced it.
pub fn write(path: &Path, settled: &[Settled], fail_under: f64) -> Result<()> {
    let document = render(settled, fail_under)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {} for the report", parent.display()))?;
    }
    fs::write(path, document).with_context(|| format!("writing the report to {}", path.display()))
}

fn render(settled: &[Settled], fail_under: f64) -> Result<String> {
    let (high, low) = thresholds(fail_under);
    let mut document = String::from("{\n");
    document.push_str(&format!("  \"schemaVersion\": \"{SCHEMA_VERSION}\",\n"));
    document.push_str(&format!(
        "  \"thresholds\": {{ \"high\": {high}, \"low\": {low} }},\n"
    ));
    // `framework.name` is what a SonarQube import uses as its engine id.
    document.push_str(&format!(
        "  \"framework\": {{ \"name\": \"Angelo\", \"version\": \"{}\" }},\n",
        env!("CARGO_PKG_VERSION")
    ));
    // No `projectRoot`. The conversion into other formats strips it off the
    // front of every key with a literal string match, so a Windows root against
    // forward-slashed keys silently strips nothing and the paths resolve to no
    // file at all. Relative keys need no root.
    document.push_str("  \"files\": {\n");

    let files = group_by_file(settled);
    let mut first = true;
    for (path, mutants) in &files {
        if !first {
            document.push_str(",\n");
        }
        first = false;
        document.push_str(&render_file(path, mutants)?);
    }
    document.push_str("\n  }\n}\n");
    Ok(document)
}

/// A BTreeMap, so the same run always writes the same bytes and two reports can
/// be diffed.
fn group_by_file(settled: &[Settled]) -> BTreeMap<String, Vec<&Settled>> {
    let mut files: BTreeMap<String, Vec<&Settled>> = BTreeMap::new();
    for entry in settled {
        files
            .entry(entry.mutant.report_path())
            .or_default()
            .push(entry);
    }
    files
}

fn render_file(path: &str, mutants: &[&Settled]) -> Result<String> {
    // The schema requires the whole original text of every mutated file. No
    // consumer of the Sonar route reads it, but the HTML viewer cannot show a
    // mutant in context without it and a strict validator demands it.
    //
    // A file that has since been deleted or renamed still has rows in the
    // database, and losing the whole report over one of them would be worse
    // than reporting it with no source to show.
    let source = fs::read_to_string(path).unwrap_or_default();
    let mut lines = Lines::default();

    let mut rendered = format!(
        "    {}: {{\n      \"language\": \"python\",\n      \"source\": {},\n      \"mutants\": [\n",
        quoted(path),
        quoted(&source)
    );
    for (at, entry) in mutants.iter().enumerate() {
        if at > 0 {
            rendered.push_str(",\n");
        }
        rendered.push_str(&render_mutant(entry, &source, &mut lines));
    }
    rendered.push_str("\n      ]\n    }");
    Ok(rendered)
}

fn render_mutant(entry: &Settled, source: &str, lines: &mut Lines) -> String {
    let mutant = &entry.mutant;
    // `end` is exclusive, so the two offsets go in as they are. Both are
    // 1-based; the schema sets `minimum: 1` on each, and a 0 is not off by one,
    // it is invalid.
    let start = lines.position(source, mutant.byte_start);
    let end = lines.position(source, mutant.byte_end);
    let status = match entry.status {
        Some(status) => status.schema_name(entry.executed),
        // Enumerated, never run. The schema has a name for that too, so a
        // resumable run reports honestly instead of looking finished.
        None => "Pending",
    };

    let mut fields = vec![
        format!("\"id\": {}", quoted(&mutant.id.to_string())),
        format!("\"mutatorName\": {}", quoted(mutant.mutator())),
        format!("\"replacement\": {}", quoted(&mutant.replacement)),
        format!("\"location\": {}", location(start, end)),
        format!("\"status\": {}", quoted(status)),
    ];
    if let Some(duration) = entry.duration_ms {
        fields.push(format!("\"duration\": {duration}"));
    }
    if !entry.failed_tests.is_empty() {
        fields.push(format!("\"killedBy\": {}", strings(&entry.failed_tests)));
    }
    format!("        {{ {} }}", fields.join(", "))
}

fn location(start: Position, end: Position) -> String {
    format!(
        "{{ \"start\": {{ \"line\": {}, \"column\": {} }}, \"end\": {{ \"line\": {}, \"column\": {} }} }}",
        start.line, start.column, end.line, end.column
    )
}

fn strings(values: &[String]) -> String {
    let quoted: Vec<String> = values.iter().map(|value| quoted(value)).collect();
    format!("[{}]", quoted.join(", "))
}

/// A JSON string, quotes included.
///
/// Mutants carry raw Python, so every one of these cases turns up in a real
/// run: a replacement holding a quote, a source file holding a backslash, and a
/// `source` field holding the whole file's newlines.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            // Anything else below a space has no short escape and has to go out
            // as six characters or the document is not JSON.
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate::{Mutant, Status};
    use std::path::PathBuf;

    fn settled(
        original: &str,
        replacement: &str,
        status: Option<Status>,
        executed: bool,
    ) -> Settled {
        Settled {
            mutant: Mutant {
                id: 7,
                file: PathBuf::from("calc.py"),
                line: 1,
                byte_start: 4,
                byte_end: 4 + original.len(),
                original: original.to_string(),
                replacement: replacement.to_string(),
            },
            status,
            executed,
            duration_ms: Some(120),
            failed_tests: vec!["test_add".to_string()],
        }
    }

    #[test]
    fn escaping_survives_what_python_actually_contains() {
        assert_eq!(quoted(r#"a "b" c"#), r#""a \"b\" c""#);
        assert_eq!(quoted(r"a\b"), r#""a\\b""#);
        assert_eq!(quoted("one\ntwo"), r#""one\ntwo""#);
        assert_eq!(quoted("a\tb"), r#""a\tb""#);
        assert_eq!(quoted("a\rb"), r#""a\rb""#);
    }

    /// A control character with no short escape has to become six characters.
    /// An escape sequence in a Python string literal is a real source of these.
    #[test]
    fn a_bare_control_character_becomes_a_unicode_escape() {
        assert_eq!(quoted("a\u{1}b"), r#""a\u0001b""#);
        assert_eq!(quoted("\u{8}\u{c}"), r#""\b\f""#);
    }

    /// Non-ASCII is valid JSON as it stands, and escaping it would only make
    /// the file bigger and the source unreadable in a viewer.
    #[test]
    fn text_above_ascii_goes_through_untouched() {
        assert_eq!(quoted("héllo → ok"), "\"héllo → ok\"");
    }

    #[test]
    fn a_mutant_carries_every_field_the_schema_requires() {
        let source = "a = 1 + 2\n";
        let rendered = render_mutant(
            &settled("+", "-", Some(Status::Survived), true),
            source,
            &mut Lines::default(),
        );
        assert!(rendered.contains(r#""id": "7""#), "{rendered}");
        assert!(rendered.contains(r#""mutatorName": "ArithmeticOperator""#));
        assert!(rendered.contains(r#""replacement": "-""#));
        assert!(rendered.contains(r#""status": "Survived""#));
        assert!(rendered.contains(r#""killedBy": ["test_add"]"#));
        assert!(rendered.contains(r#""duration": 120"#));
    }

    /// The offsets say bytes 4 to 5 of `a = 1 + 2`, which is line 1, column 5,
    /// ending exclusively at column 6.
    #[test]
    fn a_location_is_one_based_and_ends_exclusively() {
        let rendered = render_mutant(
            &settled("+", "-", Some(Status::Killed), true),
            "a = 1 + 2\n",
            &mut Lines::default(),
        );
        assert!(
            rendered.contains(
                r#""location": { "start": { "line": 1, "column": 5 }, "end": { "line": 1, "column": 6 } }"#
            ),
            "{rendered}"
        );
    }

    /// A mutant nothing covers is `NoCoverage`, not `Survived`. They are
    /// different findings: one wants a test, the other wants an assertion.
    #[test]
    fn an_uncovered_survivor_is_reported_as_such() {
        let uncovered = render_mutant(
            &settled("+", "-", Some(Status::Survived), false),
            "a = 1 + 2\n",
            &mut Lines::default(),
        );
        assert!(uncovered.contains(r#""status": "NoCoverage""#));
    }

    /// A pending mutant is enumerated and unjudged, and saying so is the
    /// difference between a partial report and a dishonest one.
    #[test]
    fn a_pending_mutant_says_it_is_pending() {
        let pending = render_mutant(
            &settled("+", "-", None, false),
            "a = 1 + 2\n",
            &mut Lines::default(),
        );
        assert!(pending.contains(r#""status": "Pending""#));
    }

    #[test]
    fn the_document_carries_its_version_and_thresholds() {
        let document = render(&[settled("+", "-", Some(Status::Killed), true)], 0.0).unwrap();
        assert!(document.contains(r#""schemaVersion": "2.0""#));
        assert!(document.contains(r#""thresholds": { "high": 80, "low": 60 }"#));
        assert!(document.contains(r#""framework": { "name": "Angelo""#));
        assert!(document.contains(r#""language": "python""#));
    }

    /// The conversion into other formats strips `projectRoot` off every key
    /// with a literal string match, and a root that does not match strips
    /// nothing, so the paths resolve to no file and the issues vanish silently.
    #[test]
    fn no_project_root_is_written() {
        let document = render(&[settled("+", "-", Some(Status::Killed), true)], 0.0).unwrap();
        assert!(!document.contains("projectRoot"), "{document}");
    }

    /// A project that set a threshold cares about that number, not about 60.
    #[test]
    fn a_threshold_becomes_the_lower_band() {
        assert_eq!(thresholds(75.0), (80, 75));
        assert_eq!(thresholds(0.0), (80, 60));
        // A band cannot sit below the one under it.
        assert_eq!(thresholds(95.0), (95, 95));
    }

    /// Keys are what a consumer resolves back to real files, so a Windows run
    /// must not write Windows paths.
    #[test]
    fn file_keys_are_forward_slashed() {
        let mut entry = settled("+", "-", Some(Status::Killed), true);
        entry.mutant.file = PathBuf::from(".\\src\\calc.py");
        let document = render(&[entry], 0.0).unwrap();
        assert!(document.contains(r#""src/calc.py""#), "{document}");
    }
}
