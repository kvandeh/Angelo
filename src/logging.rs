//! Levels, timestamps and where a line goes.
//!
//! The split this module exists to enforce: **results go to stdout, commentary
//! goes to stderr.** `scripts/verdict-matrix.sh` greps Angelo's output for its
//! verdict counts, so the report is the program's output and must print at any
//! verbosity. Everything else is commentary, and a level can silence it.
//!
//! Logging is also routed through the progress bars rather than around them, so
//! a warning printed mid-run does not land on top of a half-drawn bar.

use std::env;
use std::io::Write;

use anyhow::{Context, Result};
use clap::ValueEnum;
use indicatif::MultiProgress;
use log::{LevelFilter, Metadata, Record};

/// How much commentary a run prints. `error` still prints the whole report,
/// because the report is not commentary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Verbosity {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Verbosity {
    fn filter(self) -> LevelFilter {
        match self {
            Verbosity::Error => LevelFilter::Error,
            Verbosity::Warn => LevelFilter::Warn,
            Verbosity::Info => LevelFilter::Info,
            Verbosity::Debug => LevelFilter::Debug,
            Verbosity::Trace => LevelFilter::Trace,
        }
    }
}

/// What settled the level. Separated from the environment so the precedence
/// rule is a unit test rather than a thing you find out in CI.
#[derive(Debug, PartialEq, Eq)]
enum Chosen {
    /// `--verbosity`, which beats everything.
    Flag(LevelFilter),
    /// `RUST_LOG`, the convention `env_logger` already implements. Kept as the
    /// raw spec because it can name per-module levels a `LevelFilter` cannot.
    RustLog(String),
    /// Nobody asked, so `CI` decides.
    Default(LevelFilter),
}

/// Highest wins: the flag, then `RUST_LOG`, then whether this looks like CI.
///
/// `CI` replaces guessing from the platform. GitHub Actions sets it on Windows
/// and macOS runners too, so it is the signal that actually means nobody is
/// watching this scroll past.
fn choose(flag: Option<Verbosity>, rust_log: Option<String>, ci: bool) -> Chosen {
    if let Some(verbosity) = flag {
        return Chosen::Flag(verbosity.filter());
    }
    if let Some(spec) = rust_log.filter(|spec| !spec.trim().is_empty()) {
        return Chosen::RustLog(spec);
    }
    Chosen::Default(match ci {
        true => LevelFilter::Warn,
        false => LevelFilter::Info,
    })
}

/// `CI` counts when it is set to anything meaningful. `false` and `0` are how a
/// person turns it back off, and an empty value is not a claim either way.
fn looks_like_ci(value: Option<String>) -> bool {
    match value {
        Some(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "false" | "0"
        ),
        None => false,
    }
}

/// Start logging, writing through the bars rather than over them.
///
/// Both go to stderr: `env_logger` writes there by default, and `indicatif`
/// hides a bar entirely when stderr is not a terminal, so a redirected run
/// emits no control characters at all.
pub fn init(flag: Option<Verbosity>, bars: &MultiProgress) -> Result<()> {
    let mut builder = env_logger::Builder::new();
    match choose(
        flag,
        env::var("RUST_LOG").ok(),
        looks_like_ci(env::var("CI").ok()),
    ) {
        Chosen::Flag(level) | Chosen::Default(level) => builder.filter_level(level),
        Chosen::RustLog(spec) => builder.parse_filters(&spec),
    };
    builder.format(format_line);

    let logger = BarAware {
        inner: builder.build(),
        bars: bars.clone(),
    };
    log::set_max_level(logger.inner.filter());
    log::set_boxed_logger(Box::new(logger)).context("installing the logger")
}

/// `12:34:56 INFO   enumerated 812 mutants`, and the module too once the reader
/// is debugging Angelo itself rather than their own suite.
fn format_line(buffer: &mut env_logger::fmt::Formatter, record: &Record) -> std::io::Result<()> {
    let style = buffer.default_level_style(record.level());
    let module = match record.level() >= log::Level::Debug {
        true => format!("{}  ", record.target()),
        false => String::new(),
    };
    writeln!(
        buffer,
        "{} {style}{:<6}{style:#} {module}{}",
        clock(buffer),
        record.level(),
        record.args()
    )
}

/// The wall clock, without the date the reader already knows. `timestamp` is
/// RFC 3339, so the time is what sits between the `T` and the `Z`.
fn clock(buffer: &env_logger::fmt::Formatter) -> String {
    let stamp = buffer.timestamp_seconds().to_string();
    stamp
        .split_once('T')
        .and_then(|(_, rest)| rest.split_once('Z'))
        .map(|(time, _)| time.to_string())
        .unwrap_or(stamp)
}

/// `env_logger`'s sink, wrapped so its writes pass through the bars.
struct BarAware {
    inner: env_logger::Logger,
    bars: MultiProgress,
}

impl log::Log for BarAware {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        // Filtered records never reach `suspend`, which takes a lock and
        // repaints every bar. On a `trace!` in a hot loop that is the whole
        // cost of logging, so it has to sit behind the level check.
        if !self.inner.matches(record) {
            return;
        }
        self.bars.suspend(|| self.inner.log(record));
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_beats_everything() {
        assert_eq!(
            choose(Some(Verbosity::Trace), Some("error".to_string()), true),
            Chosen::Flag(LevelFilter::Trace)
        );
    }

    #[test]
    fn rust_log_beats_the_default_but_not_the_flag() {
        assert_eq!(
            choose(None, Some("angelo::exec=debug".to_string()), false),
            Chosen::RustLog("angelo::exec=debug".to_string())
        );
        assert_eq!(
            choose(Some(Verbosity::Warn), Some("debug".to_string()), false),
            Chosen::Flag(LevelFilter::Warn)
        );
    }

    /// An exported-but-empty `RUST_LOG` is not a request for anything, and
    /// `env_logger` would read it as "off".
    #[test]
    fn an_empty_rust_log_is_not_a_choice() {
        assert_eq!(
            choose(None, Some("  ".to_string()), false),
            Chosen::Default(LevelFilter::Info)
        );
    }

    #[test]
    fn ci_is_quiet_and_a_terminal_is_not() {
        assert_eq!(choose(None, None, true), Chosen::Default(LevelFilter::Warn));
        assert_eq!(
            choose(None, None, false),
            Chosen::Default(LevelFilter::Info)
        );
    }

    /// Every CI provider sets `CI` to something different, and `CI=false` is how
    /// a person says they are not in one.
    #[test]
    fn ci_counts_when_it_says_anything_real() {
        assert!(looks_like_ci(Some("true".to_string())));
        assert!(looks_like_ci(Some("1".to_string())));
        assert!(looks_like_ci(Some("woodpecker".to_string())));
        assert!(!looks_like_ci(Some("false".to_string())));
        assert!(!looks_like_ci(Some("FALSE".to_string())));
        assert!(!looks_like_ci(Some("0".to_string())));
        assert!(!looks_like_ci(Some(String::new())));
        assert!(!looks_like_ci(None));
    }
}
