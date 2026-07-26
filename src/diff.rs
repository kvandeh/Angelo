use std::collections::{HashMap, HashSet};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::mutate::Mutant;

/// Lines touched since a git revision, per file. Mutating only these is the
/// biggest lever there is on a big repo: the pool shrinks with the change,
/// not with the codebase.
pub struct ChangedLines {
    by_file: HashMap<String, HashSet<u32>>,
}

impl ChangedLines {
    /// `git diff --unified=0` against `revision`, restricted to Python files.
    /// `--relative` makes the paths match the ones mutants carry when angelo
    /// runs in a subdirectory of the repo.
    pub fn since(revision: &str) -> Result<ChangedLines> {
        let output = Command::new("git")
            .args(["diff", "--unified=0", "--relative", revision, "--", "*.py"])
            .output()
            .context("running git diff (is git on PATH?)")?;
        if !output.status.success() {
            bail!(
                "git diff against '{revision}' failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(ChangedLines::parse(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    fn parse(diff: &str) -> ChangedLines {
        let mut by_file: HashMap<String, HashSet<u32>> = HashMap::new();
        let mut current = String::new();
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                current = path.to_string();
                by_file.entry(current.clone()).or_default();
                continue;
            }
            let Some(hunk) = line.strip_prefix("@@ ") else {
                continue;
            };
            let Some((start, count)) = added_range(hunk) else {
                continue;
            };
            let lines = by_file.entry(current.clone()).or_default();
            lines.extend(start..start + count);
        }
        ChangedLines { by_file }
    }

    fn contains(&self, mutant: &Mutant) -> bool {
        self.by_file
            .get(&mutant.coverage_file())
            .is_some_and(|lines| lines.contains(&mutant.line))
    }

    pub fn filter(&self, mutants: Vec<Mutant>) -> Vec<Mutant> {
        mutants
            .into_iter()
            .filter(|mutant| self.contains(mutant))
            .collect()
    }
}

/// "-12,0 +13,2 @@ def f():" -> (13, 2). A missing count means one line.
fn added_range(hunk: &str) -> Option<(u32, u32)> {
    let added = hunk.split_whitespace().find(|part| part.starts_with('+'))?;
    let (start, count) = match added[1..].split_once(',') {
        Some((start, count)) => (start, count.parse().ok()?),
        None => (&added[1..], 1),
    };
    Some((start.parse().ok()?, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const DIFF: &str = "\
diff --git a/calc.py b/calc.py
index 1234567..89abcde 100644
--- a/calc.py
+++ b/calc.py
@@ -4,0 +5,2 @@ def add(a, b):
+def sub(a, b):
+    return a - b
@@ -20 +21 @@ def other():
-    return 1
+    return 2
diff --git a/untouched.py b/untouched.py
--- a/untouched.py
+++ b/untouched.py
@@ -1,3 +1,0 @@
-gone
";

    fn mutant(file: &str, line: u32) -> Mutant {
        Mutant {
            id: 0,
            file: PathBuf::from(file),
            line,
            byte_start: 0,
            byte_end: 1,
            original: "+".to_string(),
            replacement: "-".to_string(),
        }
    }

    #[test]
    fn reads_added_line_numbers_from_hunks() {
        let changed = ChangedLines::parse(DIFF);
        assert!(changed.contains(&mutant("calc.py", 5)));
        assert!(changed.contains(&mutant("calc.py", 6)));
        assert!(changed.contains(&mutant("calc.py", 21)));
        assert!(!changed.contains(&mutant("calc.py", 7)));
        assert!(!changed.contains(&mutant("other.py", 5)));
    }

    #[test]
    fn a_pure_deletion_contributes_no_lines() {
        let changed = ChangedLines::parse(DIFF);
        assert!(!changed.contains(&mutant("untouched.py", 1)));
    }

    #[test]
    fn single_line_hunks_have_no_comma() {
        assert_eq!(added_range("-20 +21 @@ def other():"), Some((21, 1)));
        assert_eq!(added_range("-4,0 +5,2 @@"), Some((5, 2)));
    }

    #[test]
    fn filter_keeps_only_touched_mutants() {
        let changed = ChangedLines::parse(DIFF);
        let kept = changed.filter(vec![
            mutant("calc.py", 5),
            mutant("calc.py", 99),
            mutant("elsewhere.py", 5),
        ]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].line, 5);
    }

    #[test]
    fn an_empty_diff_keeps_nothing() {
        let changed = ChangedLines::parse("");
        assert!(changed.filter(vec![mutant("calc.py", 5)]).is_empty());
    }
}
