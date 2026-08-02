use std::collections::{HashMap, HashSet};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::mutate::Mutant;

/// Which lines a run is allowed to mutate.
///
/// The two variants answer different questions and only one of them suits a
/// pull request. `Since` compares a revision against the working tree, so on a
/// pushed branch, where nothing is uncommitted, it finds nothing at all.
/// `Branch` compares against the merge base, which is what the branch adds
/// however many commits it took and however far the base moved since.
pub enum Scope {
    /// `--diff [REV]`: everything that differs from REV right now.
    Since(String),
    /// `--diff-base [REV]`: what this branch adds on top of REV. `None` means
    /// work the base branch out.
    Branch(Option<String>),
}

impl Scope {
    /// clap refuses both flags at once, so at most one of them is set.
    pub fn from_flags(diff: Option<String>, base: Option<Option<String>>) -> Option<Scope> {
        match (diff, base) {
            (Some(revision), _) => Some(Scope::Since(revision)),
            (_, Some(base)) => Some(Scope::Branch(base)),
            (None, None) => None,
        }
    }

    /// The revision range to diff, resolved and fit to print.
    pub fn range(&self) -> Result<String> {
        match self {
            Scope::Since(revision) => Ok(revision.clone()),
            Scope::Branch(base) => {
                refuse_a_shallow_clone()?;
                let base = match base {
                    Some(base) => base.clone(),
                    None => default_base()?,
                };
                Ok(merge_base_range(&base))
            }
        }
    }
}

/// Three dots, not two: `A...B` diffs merge-base(A, B) against B. The base
/// branch's own new commits never show up, and the branch's commits collapse
/// into one net change, so a line added in the first commit and deleted in
/// the third counts for nothing.
fn merge_base_range(base: &str) -> String {
    format!("{base}...HEAD")
}

/// Where to diff from when `--diff-base` is given no revision.
///
/// GitHub Actions knows the answer exactly, so ask it first: `GITHUB_BASE_REF`
/// names the branch the pull request targets, and the checkout has it as a
/// remote branch rather than a local one. Otherwise origin's own HEAD is the
/// next best thing, and the usual names are the last resort.
fn default_base() -> Result<String> {
    let mut candidates = Vec::new();
    if let Ok(branch) = std::env::var("GITHUB_BASE_REF")
        && !branch.is_empty()
    {
        candidates.push(format!("origin/{branch}"));
        candidates.push(branch);
    }
    if let Ok(head) = git(&["symbolic-ref", "refs/remotes/origin/HEAD"])
        && let Some(name) = head.trim().strip_prefix("refs/remotes/")
    {
        candidates.push(name.to_string());
    }
    candidates.extend(["origin/main", "main", "master"].map(str::to_string));

    candidates
        .into_iter()
        .find(|candidate| git(&["rev-parse", "--verify", "--quiet", candidate]).is_ok())
        .context("no base branch found, name one: `angelo exec --diff-base main`")
}

/// A shallow clone has no merge base to find. Falling back to a two-dot diff
/// would mutate whatever the two histories happen to disagree about and then
/// report a confident score for it, so stop and say what to change instead.
/// `actions/checkout` defaults to `fetch-depth: 1`.
fn refuse_a_shallow_clone() -> Result<()> {
    if git(&["rev-parse", "--is-shallow-repository"])?.trim() == "true" {
        bail!(
            "--diff-base needs the history back to the merge base, but this is a shallow \
             clone. In GitHub Actions, set `fetch-depth: 0` on actions/checkout"
        );
    }
    Ok(())
}

/// Every git call goes through here, so a failure names the command it came
/// from rather than leaving the user to guess.
fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("running git {} (is git on PATH?)", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Lines touched over a revision range, per file. Mutating only these is the
/// biggest lever there is on a big repo: the pool shrinks with the change,
/// not with the codebase.
pub struct ChangedLines {
    by_file: HashMap<String, HashSet<u32>>,
}

impl ChangedLines {
    /// `git diff --unified=0` over `range`, restricted to Python files.
    /// `--relative` makes the paths match the ones mutants carry when angelo
    /// runs in a subdirectory of the repo.
    pub fn over(range: &str) -> Result<ChangedLines> {
        let diff = git(&["diff", "--unified=0", "--relative", range, "--", "*.py"])?;
        Ok(ChangedLines::parse(&diff))
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

    /// A file renamed inside the branch arrives under its new path, because
    /// that is what `+++ b/` carries. Mutants know only the new path too.
    #[test]
    fn a_renamed_file_is_read_under_its_new_path() {
        let changed = ChangedLines::parse(
            "\
diff --git a/old.py b/new.py
similarity index 88%
rename from old.py
rename to new.py
index 1234567..89abcde 100644
--- a/old.py
+++ b/new.py
@@ -3 +3 @@ def f():
-    return 1
+    return 2
",
        );
        assert!(changed.contains(&mutant("new.py", 3)));
        assert!(!changed.contains(&mutant("old.py", 3)));
    }

    #[test]
    fn no_flag_means_the_whole_codebase() {
        assert!(Scope::from_flags(None, None).is_none());
    }

    #[test]
    fn a_plain_revision_is_diffed_as_given() {
        let scope = Scope::from_flags(Some("HEAD".to_string()), None).expect("a scope");
        assert_eq!(scope.range().unwrap(), "HEAD");
    }

    /// The third dot is the whole fix: two dots would drag in whatever the
    /// base branch gained since this branch left it.
    #[test]
    fn a_base_is_diffed_from_the_merge_base() {
        let scope = Scope::from_flags(None, Some(Some("main".to_string()))).expect("a scope");
        assert!(matches!(scope, Scope::Branch(Some(base)) if base == "main"));
        assert_eq!(merge_base_range("main"), "main...HEAD");
    }

    /// `--diff-base` with no revision defers to [`default_base`], which needs
    /// a repository to ask, so the flag alone must survive the trip.
    #[test]
    fn a_bare_diff_base_carries_no_revision() {
        let scope = Scope::from_flags(None, Some(None)).expect("a scope");
        assert!(matches!(scope, Scope::Branch(None)));
    }
}
