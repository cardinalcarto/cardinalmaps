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
use valinor_router::{DEFAULT_TILE_CACHE_SIZE, Router, RouterConfig};

mod app;
#[cfg(test)]
mod tests;

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

fn open(cli: &Cli) -> anyhow::Result<(Router, usize)> {
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
    Ok((router, workers))
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

    let (router, workers) = open(&cli)?;
    let state = Arc::new(app::AppState::new(
        router,
        workers,
        Duration::from_secs(cli.timeout_seconds),
    ));
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
