#![doc = include_str!("../README.md")]

use anyhow::{Context, bail};
use clap::Parser;
use std::path::{Path, PathBuf};
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use zeromq::{DealerSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

/// The size of `prime_server`'s `http_request_info_t`, which leads every message
/// (two `u32` fields and two `u16` fields).
const REQUEST_INFO_SIZE: usize = 12;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// The `_out` endpoint of the proxy upstream of the tap.
    #[arg(env, long)]
    upstream: String,

    /// The `_in` endpoint of the proxy downstream of the tap.
    #[arg(env, long)]
    downstream: String,

    /// The directory to write captured frames to (created if missing).
    #[arg(env, long)]
    out_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env()))
        .init();

    std::fs::create_dir_all(&cli.out_dir)
        .with_context(|| format!("failed to create {}", cli.out_dir.display()))?;

    let mut upstream = DealerSocket::new();
    upstream
        .connect(&cli.upstream)
        .await
        .with_context(|| format!("failed to connect upstream to {}", cli.upstream))?;
    let mut downstream = DealerSocket::new();
    downstream
        .connect(&cli.downstream)
        .await
        .with_context(|| format!("failed to connect downstream to {}", cli.downstream))?;

    info!(
        "tap started (upstream = {}, downstream = {}, out = {})",
        cli.upstream,
        cli.downstream,
        cli.out_dir.display()
    );

    let mut sequence: u64 = 0;
    loop {
        // Advertise readiness with an empty heartbeat, as `prime_server` workers do.
        upstream
            .send(ZmqMessage::from(""))
            .await
            .context("failed to advertise to upstream")?;

        let message = tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received; shutting down");
                return Ok(());
            }
            message = upstream.recv() => message.context("failed to receive from upstream")?,
        };

        // A failure to record one message should not wedge the chain;
        // log it and still forward, so the request completes.
        if let Err(e) = record(&cli.out_dir, sequence, &message) {
            error!("failed to record message {sequence}: {e:#}");
        }
        sequence += 1;

        if let Err(e) = downstream.send(message).await {
            error!("failed to forward message downstream: {e}");
        }
    }
}

/// Writes each frame of a message to its own file.
fn record(out_dir: &Path, sequence: u64, message: &ZmqMessage) -> anyhow::Result<()> {
    let frames = message.iter().collect::<Vec<_>>();
    let Some(request_info) = frames.first() else {
        bail!("received an empty message");
    };
    if request_info.len() != REQUEST_INFO_SIZE {
        bail!(
            "expected a {REQUEST_INFO_SIZE}-byte request info frame; got {} bytes",
            request_info.len()
        );
    }
    let mut id_bytes = [0u8; 4];
    id_bytes.copy_from_slice(&request_info[0..4]);
    let request_id = u32::from_le_bytes(id_bytes);

    for (index, frame) in frames.iter().enumerate() {
        let path = out_dir.join(format!("{sequence:06}-{request_id}.{index}.bin"));
        std::fs::write(&path, frame)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    info!(
        "recorded message {sequence} (request {request_id}, {} frames)",
        frames.len()
    );
    Ok(())
}
