//! `valinor-server`: Valhalla's HTTP API, served by `valinor-router` behind axum.
//!
//! ```text
//! valinor-server --config <valhalla.json> [--tiles <tarball-or-dir>...] [--listen <addr:port>]
//! ```
//!
//! It serves Valhalla's paths at the root (`/route`, `/locate`, `/status`, and so on),
//! with `GET ?json=` and POST, as mainline's httpd does; see [`app`] for the details.
//! `GET /status` is the health check. Logging follows `RUST_LOG`.
//! It writes no files.
//! With `--transit-timetable`, it also plans public transit at `/transit/...`; see [`transit`].

use anyhow::{Context, bail};
use clap::Parser;
use core::num::NonZeroUsize;
use core::time::Duration;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use valinor_gtfs::LoadedTimetable;
use valinor_router::{DEFAULT_TILE_CACHE_SIZE, Router, RouterConfig};
use valinor_transit_plan::{ServiceLimits, StreetConfig, StreetPlanner};

mod app;
#[cfg(test)]
mod tests;
mod transit;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// A mainline `valhalla.json`, for the stages' configuration
    /// (actions, service limits, defaults, and search parameters).
    #[arg(env, long)]
    config: PathBuf,

    /// The tile sets: mainline tile extracts (`.tar`) or tile directories, from one build.
    /// Without any, the config's `mjolnir.tile_extract`,
    /// relative to the config file's directory.
    #[arg(long, num_args = 1..)]
    tiles: Vec<PathBuf>,

    /// The address to listen on.
    #[arg(env, long, default_value = "0.0.0.0:8002")]
    listen: SocketAddr,

    /// How many requests to route at once (default: one per core).
    /// Each has its own path search storage; more requests wait.
    #[arg(env, long)]
    workers: Option<NonZeroUsize>,

    /// How many tiles each tile directory keeps in memory (tarballs need no cache).
    #[arg(env, long, default_value_t = DEFAULT_TILE_CACHE_SIZE)]
    tile_cache_size: NonZeroUsize,

    /// How long a request may take, waiting for a worker included, before it is a 504.
    #[arg(env, long, default_value = "60")]
    timeout_seconds: u64,

    /// The largest request body, in bytes (larger ones are a 413).
    #[arg(env, long, default_value = "16777216")]
    max_body_bytes: usize,

    /// A compiled transit timetable (`valinor-gtfs`), to serve `/transit/...`.
    /// Without one, those paths are unknown, as any others are.
    #[arg(env, long)]
    transit_timetable: Option<PathBuf>,

    /// The shared secret for feeding GTFS-RT to `PUT /transit/feeds/...`.
    /// Without one, real-time data can't be ingested and transit serves the schedule.
    #[arg(env, long)]
    transit_ingest_token: Option<String>,

    /// The longest access or egress leg a transit query may ask for, in seconds
    /// (longer ones are a 400), as Valhalla's service limits bound its other work.
    #[arg(env, long, default_value = "3600")]
    transit_max_walk_seconds: u32,
}

/// The tile sets to open: `--tiles`, or else the config's `mjolnir.tile_extract`
/// resolved against the config's directory.
fn tile_sets(cli: &Cli, config: &RouterConfig) -> anyhow::Result<Vec<PathBuf>> {
    if !cli.tiles.is_empty() {
        return Ok(cli.tiles.clone());
    }
    let directory = cli.config.parent().unwrap_or(Path::new("."));
    match config.tile_extract_in(directory) {
        Some(extract) => Ok(vec![extract]),
        None => bail!(
            "no tiles: pass --tiles, or name mjolnir.tile_extract in {}",
            cli.config.display()
        ),
    }
}

/// Opens transit, if a timetable was given, over the same tile sets as the router.
fn open_transit(cli: &Cli, tiles: &[PathBuf]) -> anyhow::Result<Option<transit::Transit>> {
    let Some(path) = cli.transit_timetable.as_deref() else {
        return Ok(None);
    };
    // SAFETY: the file is the operator's published timetable, replaced by renaming over it,
    // as `LoadedTimetable::map` asks; nothing modifies it in place while the server runs.
    let timetable = unsafe { LoadedTimetable::map(path) }
        .with_context(|| format!("load the timetable {}", path.display()))?;
    let config = StreetConfig {
        limits: ServiceLimits {
            max_walk_seconds: cli.transit_max_walk_seconds,
            ..ServiceLimits::default()
        },
        ..StreetConfig::default()
    };
    let planner =
        StreetPlanner::open(timetable, tiles, config).context("open transit over the tiles")?;
    info!(
        "transit: {} feeds, from {}",
        planner.state().feeds().len(),
        path.display()
    );
    Ok(Some(transit::Transit::new(
        planner,
        cli.transit_ingest_token.clone(),
    )))
}

fn open(cli: &Cli) -> anyhow::Result<(Router, usize, Vec<PathBuf>)> {
    let json = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("read {}", cli.config.display()))?;
    let mut config = RouterConfig::from_valhalla_json(&json)
        .with_context(|| format!("parse {}", cli.config.display()))?;
    config.tile_cache_size = cli.tile_cache_size;
    if let Some(workers) = cli.workers {
        config.workspaces = workers;
    }
    let workers = config.workspaces.get();
    let tiles = tile_sets(cli, &config)?;
    let router = Router::open(config, &tiles).context("open the tiles")?;
    info!(
        "opened {} tiles of build {} from {}",
        router.tiles().tile_ids().count(),
        router.tiles().build_id().value(),
        tiles
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok((router, workers, tiles))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                // Colors only for a terminal, not in a container's log.
                .with_ansi(std::io::stderr().is_terminal())
                .with_writer(std::io::stderr)
                .with_filter(EnvFilter::from_default_env()),
        )
        .init();

    let (router, workers, tiles) = open(&cli)?;
    let transit = open_transit(&cli, &tiles)?;
    let state = Arc::new(
        app::AppState::new(router, workers, Duration::from_secs(cli.timeout_seconds))
            .with_transit(transit),
    );
    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("listen on {}", cli.listen))?;
    info!("listening on {} with {workers} workers", cli.listen);
    axum::serve(listener, app::app(state, cli.max_body_bytes))
        .with_graceful_shutdown(shutdown())
        .await
        .context("serve")
}

/// Resolves on Ctrl-C or SIGTERM (what Kubernetes sends), for a graceful shutdown.
async fn shutdown() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            core::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => core::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = core::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => info!("Ctrl-C received; shutting down"),
        () = terminate => info!("SIGTERM received; shutting down"),
    }
}
