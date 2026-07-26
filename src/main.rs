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
        /// Keep at most N mutants, dropping the rest at random. The score
        /// becomes an estimate over that sample.
        #[arg(long, value_name = "N")]
        sample: Option<usize>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Init => config::init(),
        Command::Exec {
            workers,
            init_only,
            diff,
            sample,
        } => exec::run(exec::Options {
            workers,
            init_only,
            diff,
            sample,
        }),
    }
}
