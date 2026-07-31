//! One self-contained HTML file: a score at the top, the detail underneath.
//!
//! Lighthouse is the model. No tabs, no search box, no framework, and no
//! network: a CI artifact and an aeroplane both have to render it, so the CSS
//! is inline and there is not a single external reference in the document.
//!
//! The template is a data file for the same reason `schema.sql` is one — an
//! editor can lint HTML, and it cannot lint a Rust string literal.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use log::Level;

use crate::db::Settled;
use crate::mutate::Status;
use crate::report::{Diagnostics, Summary};

const TEMPLATE: &str = include_str!("html/template.html");

/// Where the score stops being reassuring. Issue #1's bands, and the same ones
/// the report schema's viewers use.
const GOOD: f64 = 80.0;
const POOR: f64 = 50.0;

pub fn write(
    path: &Path,
    settled: &[Settled],
    counts: &[(String, i64)],
    diagnostics: &Diagnostics,
) -> Result<()> {
    let document = render(settled, counts, diagnostics);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {} for the report", parent.display()))?;
    }
    fs::write(path, document)
        .with_context(|| format!("writing the HTML report to {}", path.display()))
}

fn render(settled: &[Settled], counts: &[(String, i64)], diagnostics: &Diagnostics) -> String {
    // The score is asked for, never recomputed. One implementation, so the file
    // and stdout cannot disagree.
    let summary = Summary::of(counts);
    let (score, band) = match summary.score() {
        Some(score) => (format!("{score:.1}%"), band_of(score)),
        None => ("no score".to_string(), "none"),
    };

    TEMPLATE
        .replace("{{SCORE_BAND}}", band)
        .replace("{{SCORE}}", &score)
        .replace(
            "{{SCORE_DETAIL}}",
            &match summary.score() {
                Some(_) => format!(
                    "{} of {} mutants detected",
                    summary.detected(),
                    summary.scored()
                ),
                None => "nothing could be measured, which is also what a broken test command \
                         looks like"
                    .to_string(),
            },
        )
        .replace("{{PROBLEMS}}", &problems(diagnostics))
        .replace("{{COUNT_ROWS}}", &count_rows(counts))
        .replace("{{FILE_ROWS}}", &file_rows(settled))
        .replace("{{SURVIVORS}}", &survivors(settled))
        .replace("{{FACT_ROWS}}", &fact_rows(diagnostics))
        .replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
}

fn band_of(score: f64) -> &'static str {
    match score {
        score if score >= GOOD => "good",
        score if score >= POOR => "warn",
        _ => "bad",
    }
}

/// Everything the run found wrong, at the top where it cannot be scrolled past.
///
/// A problem that only ever reached a terminal is a problem nobody can attach
/// to a pull request, which is the whole reason this file exists.
fn problems(diagnostics: &Diagnostics) -> String {
    if diagnostics.problems().is_empty() {
        return String::new();
    }
    let items: String = diagnostics
        .problems()
        .iter()
        .map(|problem| {
            let (band, tag) = match problem.level {
                Level::Warn => ("warn", "CHECK"),
                _ => ("none", "NOTE"),
            };
            format!(
                "<li class=\"{band}\"><span class=\"tag\">{tag}</span><span>{}</span></li>",
                escape(&problem.message)
            )
        })
        .collect();
    format!("<h2>Worth checking</h2>\n<ul class=\"problems\">{items}</ul>")
}

fn count_rows(counts: &[(String, i64)]) -> String {
    counts
        .iter()
        .map(|(status, count)| {
            let scored = match Status::parse(status) {
                Some(Status::Error) => "no, it never ran cleanly",
                Some(Status::Untestable) => "no, its tests were already red",
                Some(_) => "yes",
                None => "not yet, it has not run",
            };
            format!(
                "<tr><td>{}</td><td class=\"n\">{count}</td><td>{scored}</td></tr>",
                escape(status)
            )
        })
        .collect()
}

/// The part stdout cannot do. Worst score first, because that is the file to
/// open next; a file with no score at all sorts last, since it asks a different
/// question.
fn file_rows(settled: &[Settled]) -> String {
    let mut rows: Vec<(String, usize, usize, Option<f64>)> = by_file(settled)
        .into_iter()
        .map(|(path, entries)| {
            let summary = Summary::of(&counts_of(&entries));
            let survived = entries
                .iter()
                .filter(|entry| entry.status == Some(Status::Survived))
                .count();
            (path, entries.len(), survived, summary.score())
        })
        .collect();
    rows.sort_by(|left, right| {
        let key = |score: &Option<f64>| score.unwrap_or(f64::INFINITY);
        key(&left.3)
            .partial_cmp(&key(&right.3))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    rows.iter()
        .map(|(path, mutants, survived, score)| {
            let (band, shown) = match score {
                Some(score) => (band_of(*score), format!("{score:.1}%")),
                None => ("none", "&mdash;".to_string()),
            };
            format!(
                "<tr><td class=\"file\">{}</td><td class=\"n\">{mutants}</td>\
                 <td class=\"n\">{survived}</td><td class=\"n {band}\">{shown}</td></tr>",
                escape(path)
            )
        })
        .collect()
}

/// Grouped by file, because a survivor is only actionable next to its
/// neighbours: five in one function is one missing test, not five.
fn survivors(settled: &[Settled]) -> String {
    let mut sections = String::new();
    for (path, entries) in by_file(settled) {
        let mut alive: Vec<&&Settled> = entries
            .iter()
            .filter(|entry| entry.status == Some(Status::Survived))
            .collect();
        if alive.is_empty() {
            continue;
        }
        alive.sort_by_key(|entry| entry.mutant.line);

        let items: String = alive
            .iter()
            .map(|entry| {
                let mutant = &entry.mutant;
                let uncovered = match entry.executed {
                    true => String::new(),
                    // A different finding and a different fix: this line wants a
                    // test at all, not merely a better assertion.
                    false => " <span class=\"line\">no test covers this</span>".to_string(),
                };
                format!(
                    "<li><span class=\"line\">{}</span> <del>{}</del> &rarr; <ins>{}</ins>{uncovered}</li>",
                    mutant.line,
                    escape(&mutant.original),
                    shown(&mutant.replacement),
                )
            })
            .collect();
        sections.push_str(&format!("<h3>{}</h3><ul>{items}</ul>", escape(&path)));
    }
    match sections.is_empty() {
        true => "<p class=\"empty\">No survivors. Every mutant your tests could judge, they \
                 caught.</p>"
            .to_string(),
        false => sections,
    }
}

/// A removal has no replacement text, and an empty cell reads as a rendering
/// bug rather than as the deletion it is.
fn shown(replacement: &str) -> String {
    match replacement.is_empty() {
        true => "<em>removed</em>".to_string(),
        false => escape(replacement),
    }
}

fn fact_rows(diagnostics: &Diagnostics) -> String {
    diagnostics
        .facts()
        .iter()
        .map(|(name, value)| {
            format!(
                "<tr><td>{}</td><td class=\"mono\">{}</td></tr>",
                escape(name),
                escape(value)
            )
        })
        .collect()
}

fn by_file(settled: &[Settled]) -> BTreeMap<String, Vec<&Settled>> {
    let mut files: BTreeMap<String, Vec<&Settled>> = BTreeMap::new();
    for entry in settled {
        files
            .entry(entry.mutant.report_path())
            .or_default()
            .push(entry);
    }
    files
}

/// The shape `Summary::of` reads, so a per-file score is the same arithmetic as
/// the whole run's rather than a second version of it.
fn counts_of(entries: &[&Settled]) -> Vec<(String, i64)> {
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for entry in entries {
        let name = match entry.status {
            Some(status) => status.as_str(),
            None => "pending",
        };
        *counts.entry(name.to_string()).or_default() += 1;
    }
    counts.into_iter().collect()
}

/// Mutants carry raw Python, and `a < b` is a real replacement. Without this a
/// single survivor eats the rest of the page.
///
/// Both quote forms go too: these values land inside attributes as well as
/// between tags, and one escape function that is always right beats two that
/// are each right somewhere.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            character => out.push(character),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate::Mutant;
    use std::path::PathBuf;

    fn settled(file: &str, line: u32, original: &str, status: Option<Status>) -> Settled {
        Settled {
            mutant: Mutant {
                id: 1,
                file: PathBuf::from(file),
                line,
                byte_start: 0,
                byte_end: original.len(),
                original: original.to_string(),
                replacement: "-".to_string(),
            },
            status,
            executed: true,
            duration_ms: None,
            failed_tests: Vec::new(),
        }
    }

    fn counts(pairs: &[(&str, i64)]) -> Vec<(String, i64)> {
        pairs.iter().map(|(s, n)| (s.to_string(), *n)).collect()
    }

    #[test]
    fn everything_from_source_is_escaped() {
        assert_eq!(escape("a < b && c"), "a &lt; b &amp;&amp; c");
        assert_eq!(escape(r#""x""#), "&quot;x&quot;");
        assert_eq!(escape("it's"), "it&#39;s");
    }

    /// The one that matters: a replacement is attacker-controlled in the sense
    /// that it comes from a file, and it must render as text.
    #[test]
    fn a_script_tag_in_a_mutant_renders_as_text() {
        let mut entry = settled("x.py", 3, "1", Some(Status::Survived));
        entry.mutant.replacement = "<script>alert(1)</script>".to_string();
        let document = render(
            &[entry],
            &counts(&[("survived", 1)]),
            &Diagnostics::default(),
        );
        assert!(!document.contains("<script>alert"), "the tag survived");
        assert!(document.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }

    #[test]
    fn the_score_matches_the_summary_exactly() {
        let document = render(
            &[settled("x.py", 1, "+", Some(Status::Killed))],
            &counts(&[("killed", 46), ("survived", 28)]),
            &Diagnostics::default(),
        );
        assert!(document.contains("62.2%"), "the score is missing");
        assert!(document.contains("46 of 74 mutants detected"));
    }

    #[test]
    fn the_bands_follow_the_score() {
        assert_eq!(band_of(91.0), "good");
        assert_eq!(band_of(80.0), "good");
        assert_eq!(band_of(79.9), "warn");
        assert_eq!(band_of(50.0), "warn");
        assert_eq!(band_of(49.9), "bad");
    }

    /// An all-error run has no score, and the file must not imply one.
    #[test]
    fn a_run_with_no_score_says_so() {
        let document = render(&[], &counts(&[("error", 5)]), &Diagnostics::default());
        assert!(document.contains("no score"));
        assert!(document.contains("broken test command"));
        assert!(!document.contains("{{"), "a placeholder went unfilled");
    }

    /// Worst first, because that is the file to open next.
    #[test]
    fn the_worst_scoring_file_sorts_first() {
        let entries = vec![
            settled("good.py", 1, "+", Some(Status::Killed)),
            settled("good.py", 2, "+", Some(Status::Killed)),
            settled("bad.py", 1, "+", Some(Status::Survived)),
        ];
        let rows = file_rows(&entries);
        let bad = rows.find("bad.py").expect("bad.py");
        let good = rows.find("good.py").expect("good.py");
        assert!(bad < good, "{rows}");
    }

    /// A file whose mutants all errored has no score to rank, and guessing one
    /// would put it at the top of a list it does not belong in.
    #[test]
    fn a_file_with_no_score_sorts_last() {
        let entries = vec![
            settled("unscored.py", 1, "+", Some(Status::Error)),
            settled("scored.py", 1, "+", Some(Status::Survived)),
        ];
        let rows = file_rows(&entries);
        assert!(rows.find("scored.py") < rows.find("unscored.py"), "{rows}");
    }

    #[test]
    fn survivors_group_under_their_file() {
        let entries = vec![
            settled("a.py", 9, "+", Some(Status::Survived)),
            settled("a.py", 2, "*", Some(Status::Survived)),
            settled("b.py", 1, "+", Some(Status::Killed)),
        ];
        let rendered = survivors(&entries);
        assert!(rendered.contains("<h3>a.py</h3>"));
        assert!(
            !rendered.contains("b.py"),
            "a killed mutant is not a survivor"
        );
        // Sorted by line, so the list reads down the file.
        assert!(rendered.find(">2<") < rendered.find(">9<"), "{rendered}");
    }

    /// A survivor nothing covers wants a test, not a better assertion, and the
    /// report has to say which of the two it is looking at.
    #[test]
    fn an_uncovered_survivor_is_marked() {
        let mut entry = settled("a.py", 4, "+", Some(Status::Survived));
        entry.executed = false;
        assert!(survivors(&[entry]).contains("no test covers this"));
    }

    #[test]
    fn a_clean_run_says_there_are_no_survivors() {
        let rendered = survivors(&[settled("a.py", 1, "+", Some(Status::Killed))]);
        assert!(rendered.contains("No survivors"));
    }

    /// A removal has no replacement text, and a blank cell reads as a bug.
    #[test]
    fn a_removal_reads_as_a_removal() {
        let mut entry = settled("a.py", 1, "not", Some(Status::Survived));
        entry.mutant.replacement = String::new();
        assert!(survivors(&[entry]).contains("<em>removed</em>"));
    }

    #[test]
    fn problems_and_facts_reach_the_page() {
        let mut diagnostics = Diagnostics::default();
        diagnostics.warn("the baseline is red");
        diagnostics.fact("workers", "8");
        let document = render(&[], &counts(&[("killed", 1)]), &diagnostics);
        assert!(document.contains("Worth checking"));
        assert!(document.contains("the baseline is red"));
        assert!(document.contains("workers"));
    }

    /// No panel at all when there is nothing wrong, rather than an empty box.
    #[test]
    fn a_clean_run_grows_no_problems_panel() {
        let document = render(&[], &counts(&[("killed", 1)]), &Diagnostics::default());
        assert!(!document.contains("Worth checking"));
    }

    /// The file has to open on a machine with no network at all.
    #[test]
    fn the_document_reaches_out_to_nothing() {
        let document = render(&[], &counts(&[("killed", 1)]), &Diagnostics::default());
        for remote in ["src=", "<script", "@import", "cdn", "http://"] {
            assert!(!document.contains(remote), "{remote} is in the document");
        }
        // One link out is fine; it is a hyperlink, not a fetch.
        assert_eq!(document.matches("https://").count(), 1);
    }
}
