mod batch;
mod config;
mod coverage;
mod db;
mod diff;
mod exec;
mod html;
mod logging;
mod mutate;
mod pytest;
mod report;
mod runner;
mod schemata;
mod sonar;
mod stryker;
mod warm;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::logging::Verbosity;

#[derive(Parser)]
#[command(name = "angelo", about = "Fast mutation testing for Python", version)]
struct Cli {
    /// How much a run says about itself. The report always prints; this is the
    /// commentary around it (default: info, or warn when CI is set)
    #[arg(long, value_enum, global = true, value_name = "LEVEL")]
    verbosity: Option<Verbosity>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Detect the project layout and write angelo.conf
    Init {
        /// Overwrite an existing angelo.conf instead of refusing
        #[arg(long)]
        force: bool,
    },
    /// Enumerate mutants into .angelo/angelo.db, then run them
    Exec {
        /// Parallel pytest workers (default: one per CPU core)
        #[arg(long)]
        workers: Option<usize>,
        /// Stop after enumerating, so the planned mutants can be inspected
        #[arg(long)]
        init_only: bool,
        /// Mutate only lines changed since this git revision (default: HEAD)
        #[arg(long, num_args = 0..=1, default_missing_value = "HEAD")]
        diff: Option<String>,
        /// Mutate only the lines this branch adds on top of this revision,
        /// which is what a pull request changes (default: the base branch)
        #[arg(long, value_name = "REV", num_args = 0..=1, conflicts_with = "diff")]
        diff_base: Option<Option<String>>,
        /// Keep at most N mutants, dropping the rest at random. The score
        /// becomes an estimate over that sample.
        #[arg(long, value_name = "N")]
        sample: Option<usize>,
        /// Exit 1 when the score comes in under this percentage, so CI can
        /// gate on it (default: 0, no threshold)
        #[arg(long, value_name = "PERCENT")]
        fail_under: Option<f64>,
        /// Write the run here in the mutation-testing-report schema, the
        /// format Stryker's viewers and dashboards read
        #[arg(long, value_name = "PATH")]
        report: Option<PathBuf>,
        /// Write one self-contained HTML file here
        #[arg(long, value_name = "PATH")]
        html_report: Option<PathBuf>,
        /// Write the survivors here in SonarQube's generic issue import
        /// format, ready for sonar.externalIssuesReportPaths
        #[arg(long, value_name = "PATH")]
        sonar_report: Option<PathBuf>,
    },
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    // Before anything that might log, and before the first bar is added.
    let bars = report::bars();
    logging::init(cli.verbosity, &bars)?;

    match cli.command {
        Command::Init { force } => {
            config::init(force)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Exec {
            workers,
            init_only,
            diff,
            diff_base,
            sample,
            fail_under,
            report,
            html_report,
            sonar_report,
        } => exec::run(
            exec::Options {
                workers,
                init_only,
                scope: diff::Scope::from_flags(diff, diff_base),
                sample,
                fail_under,
                report,
                html_report,
                sonar_report,
            },
            &bars,
        ),
    }
}
