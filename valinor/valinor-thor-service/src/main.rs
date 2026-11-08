#![doc = include_str!("../README.md")]

use anyhow::Context;
use clap::Parser;
use core::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use valhalla_graphtile::tile_provider::{
    CachingTileProvider, DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valhalla_microservice::{Interrupt, Stage, ValhallaMicroserviceBuilder, WorkerResult};
use valhalla_proto::Api;
use valhalla_proto::options::Action;
use valhalla_response::{ErrorCode, ValhallaError};
use valinor_thor::{Paths, Thor, ThorConfig, Workspace};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// A mainline `valhalla.json` to read Thor's configuration from
    /// (its search parameters and service limits). Without one, mainline's defaults apply.
    #[arg(env, long)]
    config: Option<PathBuf>,

    /// The tile set: a tile extract (`.tar`) or a tile directory.
    #[arg(env, long)]
    tiles: PathBuf,

    /// How many parsed tiles to keep in memory.
    #[arg(env, long, default_value = "1024")]
    tile_cache_size: NonZeroUsize,

    /// The upstream socket to listen on (`thor.service.proxy` + `_out`).
    #[arg(env, long, default_value = "ipc:///tmp/thor_out")]
    upstream_socket_endpoint: String,

    /// The downstream socket to send trips to (`odin.service.proxy` + `_in`).
    #[arg(env, long, default_value = "ipc:///tmp/odin_in")]
    downstream_socket_endpoint: String,

    /// The Valhalla loopback socket endpoint (`httpd.service.loopback`).
    #[arg(env, long, default_value = "ipc:///tmp/loopback")]
    loopback_socket_endpoint: String,

    /// The Valhalla interrupt socket endpoint (`httpd.service.interrupt`).
    #[arg(env, long, default_value = "ipc:///tmp/interrupt")]
    interrupt_socket_endpoint: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env()))
        .init();

    let config = match &cli.config {
        Some(path) => {
            let json = std::fs::read_to_string(path)
                .with_context(|| format!("read {}", path.display()))?;
            ThorConfig::from_valhalla_json(&json)
                .with_context(|| format!("parse {}", path.display()))?
        }
        None => ThorConfig::default(),
    };

    if cli.tiles.is_dir() {
        let provider = DirectoryGraphTileProvider::new(cli.tiles.clone(), cli.tile_cache_size);
        let thor = Thor::new(
            config,
            CachingTileProvider::new(provider, cli.tile_cache_size),
        );
        serve(&cli, thor).await
    } else {
        let provider = TarballTileProvider::<false, false>::new_readonly(&cli.tiles)
            .with_context(|| format!("open {}", cli.tiles.display()))?;
        let thor = Thor::new(
            config,
            CachingTileProvider::new(provider, cli.tile_cache_size),
        );
        serve(&cli, thor).await
    }
}

/// Runs the worker loop until Ctrl-C.
async fn serve<P>(cli: &Cli, thor: Thor<P>) -> anyhow::Result<()>
where
    P: GraphTileProvider + Send + Sync + 'static,
{
    let worker = Arc::new(Worker {
        thor,
        workspace: Mutex::new(Workspace::default()),
    });
    let builder = ValhallaMicroserviceBuilder::new(
        Stage::Thor,
        &cli.upstream_socket_endpoint,
        &cli.loopback_socket_endpoint,
    )
    .with_downstream_endpoint(&cli.downstream_socket_endpoint)
    .with_interrupt_endpoint(&cli.interrupt_socket_endpoint);
    let mut service = builder
        .build(move |api: Api, interrupt: &Interrupt| worker.handle(api, interrupt))
        .await?;
    info!(
        "Thor service started (upstream = {}, downstream = {}, loopback = {}, interrupt = {})",
        cli.upstream_socket_endpoint,
        cli.downstream_socket_endpoint,
        cli.loopback_socket_endpoint,
        cli.interrupt_socket_endpoint
    );
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received; shutting down...");
                return Ok(());
            }
            outcome = service.tick() => match outcome {
                Ok(outcome) => debug!("{outcome:?}"),
                // None of the errors leave the service unusable, so log them and carry on.
                Err(e) => error!("{e}"),
            }
        }
    }
}

/// Thor and its search storage.
///
/// The service runs one job at a time, so the workspace's lock is never contended;
/// it only makes the worker function shareable.
struct Worker<P> {
    thor: Thor<P>,
    workspace: Mutex<Workspace>,
}

impl<P: GraphTileProvider> Worker<P> {
    /// Handles a job from Loki, as mainline's `thor_worker_t::work` does.
    fn handle(&self, mut api: Api, interrupt: &Interrupt) -> WorkerResult {
        if let Err(interrupted) = interrupt.check() {
            return interrupted.into();
        }
        let action = api.options.as_ref().map_or(0, |o| o.action);
        match Action::try_from(action) {
            Ok(Action::Route) => {}
            // Mainline's Thor passes status requests on to Odin
            // (or fails with 402 while shutting down, which this worker doesn't do).
            Ok(Action::Status) => return WorkerResult::forward(api),
            // Mainline's Thor also serves these; 107 is its code for an action it means to
            // support but hasn't written.
            Ok(
                Action::OptimizedRoute
                | Action::SourcesToTargets
                | Action::Isochrone
                | Action::TraceRoute
                | Action::TraceAttributes
                | Action::Expansion
                | Action::Centroid,
            ) => return WorkerResult::error(&ErrorCode::NotImplemented.into(), &mut api),
            // `thor_worker_t::work`: "this should never happen".
            _ => return WorkerResult::error(&ErrorCode::ThorUnknownAction.into(), &mut api),
        }
        let mut workspace = self
            .workspace
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let result = self
            .thor
            .route_interruptible(&mut workspace, &mut api, jiff_now(), &|| {
                interrupt.is_interrupted()
            });
        match result {
            Ok(paths) => match build_trip(&paths, &mut api) {
                Ok(()) => WorkerResult::forward(api),
                Err(e) => WorkerResult::error(&e, &mut api),
            },
            // An interrupted search sends nothing.
            Err(_) if interrupt.is_interrupted() => match interrupt.check() {
                Err(interrupted) => interrupted.into(),
                Ok(()) => WorkerResult::error(&ErrorCode::ThorUnknown.into(), &mut api),
            },
            Err(e) => WorkerResult::error(&e, &mut api),
        }
    }
}

/// The current time, for `current` date and times.
fn jiff_now() -> jiff::Timestamp {
    jiff::Timestamp::now()
}

/// Turns the paths into the request's `trip` (mainline's `TripLegBuilder`).
///
/// Trip building isn't written yet, so a route that path search finds is error 107
/// (mainline's code for an action it means to support but hasn't written).
fn build_trip(_paths: &Paths, _api: &mut Api) -> Result<(), ValhallaError> {
    Err(ValhallaError::with_detail(
        ErrorCode::NotImplemented,
        "trip building",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use http::StatusCode;
    use serde_json::{Value, json};
    use valhalla_proto::{Options, options};

    fn tiles() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../valhalla-graphtile/fixtures/andorra-tiles")
    }

    fn worker() -> Worker<DirectoryGraphTileProvider> {
        let provider = DirectoryGraphTileProvider::new(tiles(), NonZeroUsize::new(16).expect("16"));
        Worker {
            thor: Thor::new(ThorConfig::default(), provider),
            workspace: Mutex::new(Workspace::default()),
        }
    }

    fn response(result: WorkerResult) -> (StatusCode, Value) {
        let WorkerResult::HttpResponse {
            status_code, body, ..
        } = result
        else {
            panic!("expected an HTTP response, got {result:?}");
        };
        (
            status_code,
            serde_json::from_slice(&body).expect("JSON body"),
        )
    }

    fn request(action: Action) -> Api {
        Api {
            options: Some(Options {
                action: action.into(),
                format: options::Format::Json.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn answers_other_actions_with_mainline_codes() {
        let worker = worker();
        let (status, body) =
            response(worker.handle(request(Action::SourcesToTargets), &Interrupt::never()));
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["error_code"], json!(107));
        let (status, body) = response(worker.handle(request(Action::Locate), &Interrupt::never()));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error_code"], json!(400));
        assert!(matches!(
            worker.handle(request(Action::Status), &Interrupt::never()),
            WorkerResult::Forward(_)
        ));
    }

    #[test]
    fn routes_until_trip_building() {
        // Loki's output for a short Andorra route, from the Thor oracle fixtures.
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../valinor-thor/fixtures/oracle/andorra-loki.tsv");
        let text = std::fs::read_to_string(fixture).expect("fixture");
        let worker = worker();
        let mut routed = 0;
        let mut unroutable = 0;
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            let bytes = STANDARD.decode(fields[2]).expect("base64");
            let api =
                <Api as valhalla_proto::prost::Message>::decode(bytes.as_slice()).expect("an Api");
            if api
                .options
                .as_ref()
                .is_none_or(|o| o.action != Action::Route as i32)
            {
                continue;
            }
            let (status, body) = response(worker.handle(api, &Interrupt::never()));
            match body["error_code"].as_u64() {
                Some(107) => {
                    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
                    routed += 1;
                }
                Some(442) => {
                    assert_eq!(status, StatusCode::BAD_REQUEST);
                    assert_eq!(body["error"], json!("No path could be found for input"));
                    unroutable += 1;
                }
                other => panic!("unexpected {other:?}: {body}"),
            }
            if routed >= 3 && unroutable >= 3 {
                break;
            }
        }
        assert!(routed >= 3 && unroutable >= 3);
    }
}
