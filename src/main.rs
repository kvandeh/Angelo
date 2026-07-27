mod batch;
mod config;
mod coverage;
mod db;
mod diff;
mod exec;
mod mutate;
mod pytest;
mod report;
mod runner;
mod warm;

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "angelo", about = "Fast mutation testing for Python", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Detect the project layout and write angelo.conf
    Init,
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
    },
}

fn main() -> Result<ExitCode> {
    match Cli::parse().command {
        Command::Init => {
            config::init()?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Exec {
            workers,
            init_only,
            diff,
            diff_base,
            sample,
            fail_under,
        } => exec::run(exec::Options {
            workers,
            init_only,
            scope: diff::Scope::from_flags(diff, diff_base),
            sample,
            fail_under,
        }),
    }
}
