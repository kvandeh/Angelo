use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

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
    /// Cap the mutant pool at this many, dropping the rest at random.
    /// 0 = keep every mutant.
    pub sample: usize,
    /// A run times out after: what its tests took * timeout_factor + 5s. A run
    /// of the whole suite is charged the whole suite's baseline duration.
    pub timeout_factor: f64,
    /// Exit non-zero when the score comes in under this percentage, so CI can
    /// gate on it. 0 = no threshold.
    pub fail_under: f64,
    /// Draw one redrawn progress bar instead of a line per mutant. Runs of more
    /// than a thousand mutants do this anyway; this forces it at any size.
    pub show_loading: bool,
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
            sample: 0,
            timeout_factor: 2.0,
            fail_under: 0.0,
            show_loading: false,
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

    pub fn python_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for path in &self.paths {
            collect_python_files(Path::new(path), &mut files)
                .with_context(|| format!("searching {path}"))?;
        }
        files.sort();
        Ok(files)
    }
}

fn collect_python_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry
            .with_context(|| format!("reading an entry of {}", dir.display()))?
            .path();
        if path.is_dir() {
            if !is_skipped(&path) {
                collect_python_files(&path, files)?;
            }
            continue;
        }
        if is_mutation_target(&path) {
            let clean = path
                .strip_prefix(".")
                .map(Path::to_path_buf)
                .unwrap_or(path);
            files.push(clean);
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

pub fn init() -> Result<()> {
    if Path::new(CONFIG_FILE).exists() {
        bail!("{CONFIG_FILE} already exists. Delete it first for a fresh one");
    }
    let config = Config::detect();
    write(&config)?;
    println!("wrote {CONFIG_FILE} (paths = {:?})", config.paths);
    Ok(())
}

pub fn load_or_init() -> Result<Config> {
    if !Path::new(CONFIG_FILE).exists() {
        println!("no {CONFIG_FILE} found, generating one");
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
        assert!(!old.show_loading);
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
