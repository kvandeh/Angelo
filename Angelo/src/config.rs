use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::mutate::{DEFAULT_ARID, DEFAULT_FAMILIES, Operators};

pub const CONFIG_FILE: &str = "angelo.conf";

pub const SKIP_DIRS: &[&str] = &[
    ".git",
    ".angelo",
    ".pytest_cache",
    ".venv",
    "venv",
    "__pycache__",
    "node_modules",
    "target",
];

/// `serde(default)` so a config written by an older angelo still loads: any
/// field added later falls back to its default instead of erroring out.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub paths: Vec<String>,
    pub test_command: String,
    /// 0 = one per CPU core.
    pub workers: usize,
    /// Mutants per pytest run; 1 = no batching.
    pub batch_size: usize,
    /// Run only the tests that cover a batch instead of the whole suite.
    pub test_selection: bool,
    /// Host runs in a long-lived pytest process (angelo's stand-in for fork).
    pub warm_workers: bool,
    /// Restart that process every N runs, bounding accumulated state.
    pub warm_recycle_after: usize,
    /// Compile every mutant of a file into the file at once and pick one with an
    /// environment variable, so a run re-imports nothing. Needs the fork worker
    /// to stay honest, so it does nothing where `fork()` does not exist.
    pub schemata: bool,
    /// Cap the mutant pool at this many, dropping the rest at random.
    /// 0 = keep every mutant.
    pub sample: usize,
    /// A run times out after: what its tests took * timeout_factor + 5s. A run
    /// of the whole suite is charged the whole suite's baseline duration.
    pub timeout_factor: f64,
    /// Glob patterns, relative to the project root, that `paths` cannot express:
    /// generated code, vendored code, one module that hangs. `**` matches any
    /// run of directories, `*` matches within one name.
    pub exclude: Vec<String>,
    /// Exit non-zero when the score comes in under this percentage, so CI can
    /// gate on it. 0 = no threshold.
    pub fail_under: f64,
    /// Write the run to this path in the mutation-testing-report schema, the
    /// format Stryker's viewers and dashboards read. Empty = off.
    pub report: String,
    /// Write one self-contained HTML file to this path. Empty = off.
    pub html_report: String,
    /// Write the survivors to this path in SonarQube's generic issue import
    /// format. Empty = off.
    pub sonar_report: String,
    /// Which operator families to plant mutants from, named the way a report
    /// names them. The default leaves out the three the evidence does not
    /// support; `mutate::FAMILIES` is the whole list.
    pub operators: Vec<String>,
    /// Call names not worth mutating whatever the operator: logging, printing,
    /// warning. Any dotted segment of a callee matching one of these turns the
    /// whole statement away. Empty = mutate everything.
    pub arid: Vec<String>,
    /// Keep at most this many mutants on any one source line, dropping the rest
    /// at random. 0 = keep every mutant.
    pub per_line_cap: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            paths: vec![".".to_string()],
            test_command: "python -m pytest".to_string(),
            workers: 0,
            batch_size: 8,
            test_selection: true,
            warm_workers: true,
            warm_recycle_after: 50,
            schemata: true,
            sample: 0,
            timeout_factor: 2.0,
            exclude: Vec::new(),
            fail_under: 0.0,
            report: String::new(),
            html_report: String::new(),
            sonar_report: String::new(),
            operators: DEFAULT_FAMILIES.iter().map(|f| f.to_string()).collect(),
            arid: DEFAULT_ARID.iter().map(|name| name.to_string()).collect(),
            per_line_cap: 0,
        }
    }
}

impl Config {
    fn detect() -> Self {
        let mut config = Config::default();
        if Path::new("src").is_dir() {
            config.paths = vec!["src".to_string()];
        }
        config
    }

    pub fn test_command_parts(&self) -> Result<Vec<String>> {
        let parts: Vec<String> = self
            .test_command
            .split_whitespace()
            .map(String::from)
            .collect();
        if parts.is_empty() {
            bail!("test_command in {CONFIG_FILE} is empty");
        }
        Ok(parts)
    }

    pub fn effective_workers(&self, cli_override: Option<usize>) -> usize {
        let configured = cli_override.unwrap_or(self.workers);
        if configured > 0 {
            return configured;
        }
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }

    /// What this run will plant and where, with the family names checked
    /// before a single file is parsed: an unknown one has to stop the run
    /// rather than quietly shrink the pool.
    pub fn operators(&self) -> Result<Operators> {
        Operators::new(&self.operators, &self.arid)
            .with_context(|| format!("reading operators from {CONFIG_FILE}"))
    }

    pub fn python_files(&self) -> Result<Sources> {
        let mut sources = Sources {
            files: Vec::new(),
            excludes: Excludes::new(&self.exclude),
        };
        for path in &self.paths {
            collect_python_files(Path::new(path), &mut sources)
                .with_context(|| format!("searching {path}"))?;
        }
        sources.files.sort();
        Ok(sources)
    }
}

/// What the search found, and what the `exclude` patterns turned away. The two
/// travel together because a silent exclusion raises the score, so the caller
/// has to be able to report the second alongside the first.
pub struct Sources {
    pub files: Vec<PathBuf>,
    excludes: Excludes,
}

impl Sources {
    /// Rendered as a trailing clause, empty when nothing was excluded, so the
    /// usual run's message is the one it has always been.
    pub fn exclusion_note(&self) -> String {
        match self.excludes.excluded {
            0 => String::new(),
            1 => format!(" (1 path excluded by {CONFIG_FILE})"),
            excluded => format!(" ({excluded} paths excluded by {CONFIG_FILE})"),
        }
    }

    /// Whether `exclude` is why nothing was found, so an empty result blames
    /// the right key rather than sending the reader to `paths`.
    pub fn excluded_everything(&self) -> bool {
        self.files.is_empty() && self.excludes.excluded > 0
    }

    /// A pattern that matches nothing is not an error, because a project may
    /// legitimately not have a `migrations/` yet. It is almost always a typo.
    pub fn unused_patterns(&self) -> Vec<&str> {
        self.excludes
            .patterns
            .iter()
            .filter(|pattern| pattern.hits == 0)
            .map(|pattern| pattern.source.as_str())
            .collect()
    }
}

/// `paths` may name one module as readily as a directory.
fn collect_python_files(dir: &Path, sources: &mut Sources) -> Result<()> {
    if dir.is_file() {
        if is_mutation_target(dir) && !sources.excludes.excludes(dir) {
            sources.files.push(dir.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry
            .with_context(|| format!("reading an entry of {}", dir.display()))?
            .path();
        if path.is_dir() {
            // An excluded directory is never descended into, so an exclusion
            // costs less than the scan it replaces rather than more.
            if !is_skipped(&path) && !sources.excludes.excludes(&path) {
                collect_python_files(&path, sources)?;
            }
            continue;
        }
        if is_mutation_target(&path) && !sources.excludes.excludes(&path) {
            let clean = path
                .strip_prefix(".")
                .map(Path::to_path_buf)
                .unwrap_or(path);
            sources.files.push(clean);
        }
    }
    Ok(())
}

fn is_skipped(dir: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    SKIP_DIRS.contains(&name) || name == "tests"
}

fn is_mutation_target(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let is_python = name.ends_with(".py");
    let is_test = name.starts_with("test_") || name.ends_with("_test.py") || name == "conftest.py";
    is_python && !is_test
}

/// The `exclude` patterns, and how often each one fired.
struct Excludes {
    patterns: Vec<Pattern>,
    excluded: usize,
}

struct Pattern {
    /// Kept verbatim so a warning can quote what the user actually wrote.
    source: String,
    segments: Vec<String>,
    hits: usize,
}

impl Excludes {
    fn new(patterns: &[String]) -> Excludes {
        let patterns = patterns
            .iter()
            .map(|source| Pattern {
                source: source.clone(),
                segments: forward_slashed(source)
                    .split('/')
                    .map(String::from)
                    .collect(),
                hits: 0,
            })
            .collect();
        Excludes {
            patterns,
            excluded: 0,
        }
    }

    fn excludes(&mut self, path: &Path) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        // Match one path form only: forward slashes, relative to the project
        // root, the same shape `Mutant::coverage_file` produces. A `./` prefix
        // and a Windows backslash must not change the answer.
        //
        // Both are stripped from the normalised string rather than the `Path`,
        // because a backslash is not a separator off Windows: `strip_prefix(".")`
        // leaves `.\src\x.py` whole there, and the leading `.` then survives as
        // its own segment and matches nothing.
        let forward = forward_slashed(&path.to_string_lossy());
        let relative = forward.strip_prefix("./").unwrap_or(&forward);
        let segments: Vec<&str> = relative.split('/').collect();
        let Some(pattern) = self
            .patterns
            .iter_mut()
            .find(|pattern| matches(&pattern.segments, &segments))
        else {
            return false;
        };
        pattern.hits += 1;
        self.excluded += 1;
        true
    }
}

fn forward_slashed(path: &str) -> String {
    path.replace('\\', "/")
}

/// `**` matches any run of segments, `*` matches within one segment, and
/// anything else is literal.
fn matches(pattern: &[String], path: &[&str]) -> bool {
    let Some((head, rest)) = pattern.split_first() else {
        return path.is_empty();
    };
    if head == "**" {
        // Zero segments counts, so `**/migrations/**` turns away the directory
        // itself and not only the files under it. Pruning is the whole point.
        return (0..=path.len()).any(|taken| matches(rest, &path[taken..]));
    }
    let Some((segment, tail)) = path.split_first() else {
        return false;
    };
    segment_matches(head, segment) && matches(rest, tail)
}

fn segment_matches(pattern: &str, segment: &str) -> bool {
    let Some((before, after)) = pattern.split_once('*') else {
        return pattern == segment;
    };
    let Some(rest) = segment.strip_prefix(before) else {
        return false;
    };
    // Every split point, so a second `*` in the same segment still works.
    rest.char_indices()
        .map(|(at, _)| at)
        .chain([rest.len()])
        .any(|at| segment_matches(after, &rest[at..]))
}

pub fn init(force: bool) -> Result<()> {
    if Path::new(CONFIG_FILE).exists() && !force {
        bail!("{CONFIG_FILE} already exists. Delete it, or pass --force, for a fresh one");
    }
    let config = Config::detect();
    write(&config)?;
    log::info!("wrote {CONFIG_FILE} (paths = {:?})", config.paths);
    Ok(())
}

pub fn load_or_init() -> Result<Config> {
    if !Path::new(CONFIG_FILE).exists() {
        log::info!("no {CONFIG_FILE} found, generating one");
        let config = Config::detect();
        write(&config)?;
        return Ok(config);
    }
    let text = fs::read_to_string(CONFIG_FILE).with_context(|| format!("reading {CONFIG_FILE}"))?;
    toml::from_str(&text).with_context(|| format!("parsing {CONFIG_FILE}"))
}

fn write(config: &Config) -> Result<()> {
    let text = toml::to_string_pretty(config).context("serialising config")?;
    fs::write(CONFIG_FILE, text).with_context(|| format!("writing {CONFIG_FILE}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_files_are_targets_tests_are_not() {
        assert!(is_mutation_target(Path::new("src/calculator.py")));
        assert!(!is_mutation_target(Path::new("src/test_calculator.py")));
        assert!(!is_mutation_target(Path::new("src/calculator_test.py")));
        assert!(!is_mutation_target(Path::new("src/conftest.py")));
        assert!(!is_mutation_target(Path::new("src/notes.txt")));
    }

    /// Naming one module used to fail with "Not a directory (os error 20)".
    #[test]
    fn paths_may_name_a_single_file() {
        let root = std::env::temp_dir().join(format!("angelo-cfg-{}", std::process::id()));
        let package = root.join("pkg");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("mod.py"), "x = 1\n").unwrap();
        fs::write(package.join("test_mod.py"), "def test(): pass\n").unwrap();

        let mut sources = Sources {
            files: Vec::new(),
            excludes: Excludes::new(&[]),
        };
        collect_python_files(&package.join("mod.py"), &mut sources).unwrap();
        assert_eq!(sources.files, [package.join("mod.py")]);

        // A test file named directly is still not a mutation target.
        let mut sources = Sources {
            files: Vec::new(),
            excludes: Excludes::new(&[]),
        };
        collect_python_files(&package.join("test_mod.py"), &mut sources).unwrap();
        assert!(sources.files.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn junk_and_test_dirs_are_skipped() {
        assert!(is_skipped(Path::new("project/__pycache__")));
        assert!(is_skipped(Path::new("project/tests")));
        assert!(!is_skipped(Path::new("project/app")));
    }

    #[test]
    fn a_config_missing_new_fields_still_loads() {
        let old: Config = toml::from_str(
            "paths = [\".\"]\ntest_command = \"python -m pytest\"\nworkers = 0\ntimeout_factor = 2.0\n",
        )
        .expect("an older config must still parse");
        assert!(old.test_selection);
        assert_eq!(old.batch_size, 8);
        assert!(old.report.is_empty());
        assert!(old.html_report.is_empty());
    }

    /// The other direction, and the one that bites: `show_loading` was removed
    /// when every phase got a bar, and a config still naming it has to keep
    /// working rather than failing to parse.
    #[test]
    fn a_config_naming_a_removed_field_still_loads() {
        let stale: Config = toml::from_str("paths = [\".\"]\nshow_loading = true\n")
            .expect("a config naming a dropped key must still parse");
        assert_eq!(stale.paths, ["."]);
    }

    fn excluded(patterns: &[&str], path: &str) -> bool {
        let owned: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        Excludes::new(&owned).excludes(Path::new(path))
    }

    #[test]
    fn a_star_matches_within_one_name_only() {
        assert!(excluded(&["src/generated/*.py"], "src/generated/client.py"));
        assert!(!excluded(
            &["src/generated/*.py"],
            "src/generated/v1/client.py"
        ));
        assert!(!excluded(&["src/generated/*.py"], "src/app/client.py"));
        assert!(excluded(&["src/*_pb2.py"], "src/order_pb2.py"));
        assert!(excluded(&["*.py"], "client.py"));
    }

    #[test]
    fn a_double_star_matches_any_run_of_directories() {
        assert!(excluded(
            &["**/migrations/**"],
            "src/app/migrations/0001.py"
        ));
        assert!(excluded(&["**/migrations/**"], "migrations/0001.py"));
        // The directory itself, so the walk prunes it instead of descending.
        assert!(excluded(&["**/migrations/**"], "src/app/migrations"));
        assert!(!excluded(&["**/migrations/**"], "src/migrations.py"));
        assert!(!excluded(&["**/migrations/**"], "src/app/models.py"));
    }

    #[test]
    fn a_literal_path_excludes_exactly_itself() {
        assert!(excluded(&["src/legacy_parser.py"], "src/legacy_parser.py"));
        assert!(!excluded(
            &["src/legacy_parser.py"],
            "src/legacy_parser2.py"
        ));
        assert!(!excluded(
            &["src/legacy_parser.py"],
            "app/src/legacy_parser.py"
        ));
    }

    #[test]
    fn backslashes_do_not_change_the_answer() {
        assert!(excluded(
            &["src/generated/*.py"],
            "src\\generated\\client.py"
        ));
        assert!(excluded(
            &["src\\generated\\*.py"],
            "src/generated/client.py"
        ));
        // The walk hands out `./src/...` when paths is ["."].
        assert!(excluded(
            &["src/generated/*.py"],
            "./src/generated/client.py"
        ));
        assert!(excluded(
            &["src/generated/*.py"],
            ".\\src\\generated\\client.py"
        ));
    }

    #[test]
    fn no_patterns_excludes_nothing() {
        assert!(!excluded(&[], "src/app/models.py"));
    }

    #[test]
    fn a_pattern_that_never_fires_is_reported() {
        let patterns = ["**/migrations/**".to_string(), "src/typo/*.py".to_string()];
        let mut sources = Sources {
            files: Vec::new(),
            excludes: Excludes::new(&patterns),
        };
        assert!(sources.excludes.excludes(Path::new("src/migrations")));
        assert_eq!(sources.unused_patterns(), ["src/typo/*.py"]);
        assert_eq!(
            sources.exclusion_note(),
            " (1 path excluded by angelo.conf)"
        );
        assert!(sources.excludes.excludes(Path::new("app/migrations")));
        assert_eq!(
            sources.exclusion_note(),
            " (2 paths excluded by angelo.conf)"
        );
        // Nothing survived the patterns, so `paths` is the wrong thing to blame.
        assert!(sources.excluded_everything());
    }

    #[test]
    fn nothing_excluded_says_nothing() {
        let sources = Sources {
            files: Vec::new(),
            excludes: Excludes::new(&[]),
        };
        assert_eq!(sources.exclusion_note(), "");
        assert!(sources.unused_patterns().is_empty());
        // Empty because there are no sources, not because a pattern ate them.
        assert!(!sources.excluded_everything());
    }

    #[test]
    fn test_command_splits_on_whitespace() {
        let mut config = Config::detect();
        config.test_command = "python -m pytest -q".to_string();
        assert_eq!(
            config.test_command_parts().unwrap(),
            ["python", "-m", "pytest", "-q"]
        );
        config.test_command = "  ".to_string();
        assert!(config.test_command_parts().is_err());
    }
}
