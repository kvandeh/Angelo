//! Docs rot silently. Every CLI flag and config key the documentation
//! mentions must actually exist, and every config key must be documented.
//!
//! This is the cheap half of "mutation testing the docs": change a flag name
//! and the docs stop matching, so the build fails instead of the reader.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root, which is the crate's *parent*: the crate lives in
/// `Angelo/`, while `README.md`, `docs/` and `zensical.toml` sit beside it.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate directory has a parent")
        .to_path_buf()
}

/// The crate directory, for the sources this test reads directly.
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn prose() -> String {
    let mut text = fs::read_to_string(repo_root().join("README.md")).expect("README.md");
    let docs = repo_root().join("docs");
    for entry in fs::read_dir(&docs).expect("docs/") {
        let path = entry.expect("a docs entry").path();
        if path.extension().is_some_and(|extension| extension == "md") {
            text.push_str(&fs::read_to_string(&path).expect("a docs page"));
        }
    }
    text
}

fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_angelo"))
        .args(args)
        .arg("--help")
        .output()
        .expect("running angelo --help");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Long flags written as `--name` anywhere in the prose. A `--` preceded by a
/// letter is not a flag; it is something like a CSS class (`md-button--primary`)
/// or a table rule.
fn documented_flags(text: &str) -> BTreeSet<String> {
    let mut flags = BTreeSet::new();
    let characters: Vec<char> = text.chars().collect();
    for (at, window) in characters.windows(2).enumerate() {
        if window != ['-', '-'] {
            continue;
        }
        let preceded_by_word = at
            .checked_sub(1)
            .and_then(|before| characters.get(before))
            .is_some_and(|c| c.is_alphanumeric() || *c == '-');
        if preceded_by_word {
            continue;
        }
        let name: String = characters[at + 2..]
            .iter()
            .take_while(|c| c.is_ascii_lowercase() || **c == '-')
            .collect();
        let name = name.trim_end_matches('-');
        if name.len() > 2 {
            flags.insert(name.to_string());
        }
    }
    flags
}

/// Config keys are the `pub` fields of Config, read straight from the source
/// so the list cannot drift from the struct.
fn config_keys() -> BTreeSet<String> {
    let source = fs::read_to_string(crate_root().join("src/config.rs")).expect("src/config.rs");
    let body = source
        .split("pub struct Config {")
        .nth(1)
        .expect("the Config struct")
        .split("\n}")
        .next()
        .expect("the end of Config");
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("pub "))
        .filter_map(|field| field.split(':').next())
        .map(str::to_string)
        .collect()
}

/// The docs quote other tools' command lines too, so a line naming one of them
/// is skipped rather than mined for angelo flags.
const OTHER_TOOLS: &[&str] = &[
    "cargo",
    "pip",
    "git ",
    "mutmut",
    "coverage",
    "pytest",
    "zensical",
    "bash",
    "uses:",
    "benchmark.py",
    "docker",
    "mvn",
    "npx",
    "sonar-scanner",
];

#[test]
fn every_documented_flag_exists() {
    let exec_help = help(&["exec"]);
    let root_help = help(&[]);
    let text = prose();

    let ours: String = text
        .lines()
        .filter(|line| !OTHER_TOOLS.iter().any(|tool| line.contains(tool)))
        .collect::<Vec<_>>()
        .join("\n");

    let missing: Vec<String> = documented_flags(&ours)
        .into_iter()
        .filter(|flag| {
            !exec_help.contains(&format!("--{flag}")) && !root_help.contains(&format!("--{flag}"))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "documented flags that angelo does not have: {missing:?}"
    );
}

#[test]
fn every_config_key_is_documented() {
    let text = prose();
    let undocumented: Vec<String> = config_keys()
        .into_iter()
        .filter(|key| !text.contains(key))
        .collect();
    assert!(
        undocumented.is_empty(),
        "config keys missing from README/docs: {undocumented:?}"
    );
}

#[test]
fn the_documented_commands_exist() {
    let root_help = help(&[]);
    for command in ["init", "exec"] {
        assert!(root_help.contains(command), "angelo has no `{command}`");
    }
}

/// The docs site cannot build a page that is not in the nav, and a nav entry
/// pointing at a missing page is a broken link.
#[test]
fn the_nav_matches_the_docs_folder() {
    let config = fs::read_to_string(repo_root().join("zensical.toml")).expect("zensical.toml");
    let pages: BTreeSet<String> = fs::read_dir(repo_root().join("docs"))
        .expect("docs/")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "md").then(|| file_name(&path))
        })
        .collect();

    for page in &pages {
        assert!(
            config.contains(page),
            "docs/{page} is not in the zensical.toml nav"
        );
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned()
}
