//! A worker that forwards every job unchanged.
//!
//! It decodes each `Api` and re-encodes it on the way out,
//! so splicing it into a mainline chain checks this crate's protocol handling
//! and that our message definitions carry everything mainline sends.
//! See `tools/oracle/README.md` for how to run it between mainline's Loki and Thor.

use clap::Parser;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;
use valhalla_microservice::{Interrupt, Stage, ValhallaMicroserviceBuilder, WorkerResult};
use valhalla_proto::Api;

#[derive(Parser)]
#[command(about = "Forwards every job from a mainline proxy to the next one, unchanged")]
struct Cli {
    /// The `_out` endpoint of the upstream proxy.
    #[arg(long)]
    upstream: String,
    /// The `_in` endpoint of the downstream proxy.
    #[arg(long)]
    downstream: String,
    /// The server's loopback endpoint (for error responses).
    #[arg(long)]
    loopback: String,
    /// The server's interrupt endpoint.
    #[arg(long)]
    interrupt: Option<String>,
}

fn forward(api: Api, _: &Interrupt) -> WorkerResult {
    WorkerResult::forward(api)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let mut builder = ValhallaMicroserviceBuilder::new(Stage::Thor, &cli.upstream, &cli.loopback)
        .with_downstream_endpoint(&cli.downstream);
    if let Some(interrupt) = &cli.interrupt {
        builder = builder.with_interrupt_endpoint(interrupt);
    }
    let mut service = builder.build(forward).await?;
    info!(
        "passthrough started (upstream = {}, downstream = {})",
        cli.upstream, cli.downstream
    );

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            outcome = service.tick() => match outcome {
                Ok(outcome) => debug!("{outcome:?}"),
                Err(e) => error!("{e}"),
            }
        }
    }
}
