#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use env_logger::Env;
use log::info;
use tokio::net::TcpListener;
use wof_service::{AppState, Config, router};

#[derive(Debug, Parser)]
struct Args {
    /// The Who's On First SpatiaLite database, e.g.
    /// whosonfirst-data-admin-latest.spatial.db. Nothing may write to it
    /// while the service runs.
    #[arg(long, env = "WOF_DB")]
    db: PathBuf,

    /// The address to bind to.
    #[arg(long, env = "WOF_BIND", default_value = "127.0.0.1:3000")]
    bind: String,

    /// The largest bbox a request may ask for, in square degrees.
    #[arg(long, env = "WOF_MAX_AREA", default_value_t = 100.0)]
    max_area: f64,

    /// The most places a response may hold, ancestors included.
    #[arg(long, env = "WOF_MAX_PLACES", default_value_t = 50_000)]
    max_places: usize,

    /// How long a request may take, in seconds.
    #[arg(long, env = "WOF_TIMEOUT", default_value_t = 30)]
    timeout: u64,

    /// How many requests are answered at once; the rest wait. Defaults to the
    /// number of CPUs.
    #[arg(long, env = "WOF_CONCURRENCY")]
    concurrency: Option<usize>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let config = Config {
        max_area: args.max_area,
        max_places: args.max_places,
        timeout: Duration::from_secs(args.timeout),
        concurrency: args.concurrency.unwrap_or_else(num_cpus::get),
    };
    let state = AppState::open(args.db.clone(), config)?;
    info!("Serving {} on {} ({config:?})", args.db.display(), args.bind);
    let listener = TcpListener::bind(&args.bind).await?;
    axum::serve(listener, router(state)).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

/// SIGINT or SIGTERM. As a container's PID 1, the service gets no default
/// handling of either.
async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    info!("Shutting down");
}
