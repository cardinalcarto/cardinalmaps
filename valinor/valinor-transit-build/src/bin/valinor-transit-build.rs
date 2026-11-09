//! Offline stages of the transit pipeline. See the crate docs and `docs/transit.md`.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use valinor_transit_build::compile::{self, CompileOptions, FeedOutcome};
use valinor_transit_build::transfers::{self, DEFAULT_ZSTD_LEVEL, TransferOptions};
use valinor_walk::DEFAULT_TRANSFER_SECONDS;
use valinor_walk::transfer_tile::TransferMode;

/// Offline stages of the transit pipeline.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    stage: Stage,
    /// Threads to use; all cores by default.
    #[arg(long, global = true)]
    threads: Option<NonZeroUsize>,
}

#[derive(Subcommand, Debug)]
enum Stage {
    /// Compiles every fetched feed to its own timetable.
    Compile {
        /// The fetch stage's output: `feeds.json` and `feeds/`.
        #[arg(long)]
        input: PathBuf,
        /// Where to write timetables and `compile.json`.
        #[arg(long)]
        output: PathBuf,
        /// Recompile feeds that failed last time, even with unchanged input.
        #[arg(long)]
        retry_failed: bool,
    },
    /// Computes tiled transfers between every stop of every compiled feed.
    Transfers {
        /// The compile stage's output.
        #[arg(long)]
        compiled: PathBuf,
        /// The street network: a `build_extract` tarball or a tile directory.
        #[arg(long)]
        street_tiles: PathBuf,
        /// Where to publish `transfers/`, holding tiles, reports, and `manifest.json`.
        #[arg(long)]
        output: PathBuf,
        /// The bound for walking transfers, in seconds.
        #[arg(long, default_value_t = DEFAULT_TRANSFER_SECONDS)]
        max_seconds_walking: u32,
        /// The bound for cycling transfers, in seconds.
        #[arg(long, default_value_t = DEFAULT_TRANSFER_SECONDS)]
        max_seconds_cycling: u32,
        /// The zstd level of the tiles.
        #[arg(long, default_value_t = DEFAULT_ZSTD_LEVEL)]
        zstd_level: i32,
        /// The modes to build; both by default.
        #[arg(long, value_enum, num_args = 1.., default_values_t = [Mode::Walking, Mode::Cycling])]
        modes: Vec<Mode>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Walking,
    Cycling,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn core::error::Error>> {
    let threads = cli
        .threads
        .or_else(|| std::thread::available_parallelism().ok())
        .unwrap_or(NonZeroUsize::MIN);
    match cli.stage {
        Stage::Compile {
            input,
            output,
            retry_failed,
        } => {
            let report = compile::run(&CompileOptions {
                input,
                output,
                threads,
                retry_failed,
            })?;
            let failed = report
                .feeds
                .iter()
                .filter(|feed| matches!(feed.outcome, FeedOutcome::Failed { .. }))
                .count();
            println!(
                "{} feeds: {} compiled, {failed} failed",
                report.feeds.len(),
                report.feeds.len() - failed
            );
        }
        Stage::Transfers {
            compiled,
            street_tiles,
            output,
            max_seconds_walking,
            max_seconds_cycling,
            zstd_level,
            modes,
        } => {
            let manifest = transfers::run(&TransferOptions {
                compiled,
                street_tiles,
                output,
                threads,
                max_seconds_walking,
                max_seconds_cycling,
                zstd_level,
                modes: modes
                    .into_iter()
                    .map(|mode| match mode {
                        Mode::Walking => TransferMode::Walking,
                        Mode::Cycling => TransferMode::Cycling,
                    })
                    .collect(),
            })?;
            println!("{}", serde_json::to_string_pretty(&manifest.totals)?);
        }
    }
    Ok(())
}
