#![doc = include_str!("../README.md")]

use anyhow::Context;
use clap::Parser;
use core::num::NonZeroUsize;
use http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, StatusCode};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use valhalla_graphtile::GraphId;
use valhalla_graphtile::tile_provider::{
    CachingTileProvider, DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valhalla_microservice::{Interrupt, Stage, ValhallaMicroserviceBuilder, WorkerResult};
use valinor_loki::{ConnectivityMap, HttpOutcome, Loki, LokiConfig, handle_http_request};

/// `tyr`'s `Content-Disposition` for GPX responses.
const GPX_CONTENT_DISPOSITION: &str = "attachment; filename=route.gpx";

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// A mainline `valhalla.json` to read Loki's configuration from
    /// (its actions, defaults, and service limits). Without one, mainline's defaults apply.
    #[arg(env, long)]
    config: Option<PathBuf>,

    /// The tile set: a tile extract (`.tar`) or a tile directory.
    #[arg(env, long)]
    tiles: PathBuf,

    /// How many parsed tiles to keep in memory.
    #[arg(env, long, default_value = "1024")]
    tile_cache_size: NonZeroUsize,

    /// The upstream socket to listen on (`loki.service.proxy` + `_out`).
    #[arg(env, long, default_value = "ipc:///tmp/loki_out")]
    upstream_socket_endpoint: String,

    /// The downstream socket to send correlated requests to (`thor.service.proxy` + `_in`).
    #[arg(env, long, default_value = "ipc:///tmp/thor_in")]
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
            LokiConfig::from_valhalla_json(&json)
                .with_context(|| format!("parse {}", path.display()))?
        }
        None => LokiConfig::default(),
    };
    let modified = last_modified(&cli.tiles);

    if cli.tiles.is_dir() {
        let tiles = tile_ids_in(&cli.tiles)?;
        let provider = DirectoryGraphTileProvider::new(cli.tiles.clone(), cli.tile_cache_size);
        let loki = Loki::new(
            config,
            CachingTileProvider::new(provider, cli.tile_cache_size),
            Some(ConnectivityMap::new(tiles)),
        )
        .with_tileset_last_modified(modified);
        serve(&cli, loki).await
    } else {
        let provider = TarballTileProvider::<false, false>::new_readonly(&cli.tiles)
            .with_context(|| format!("open {}", cli.tiles.display()))?;
        let connectivity = ConnectivityMap::new(provider.tile_ids().copied());
        let loki = Loki::new(
            config,
            CachingTileProvider::new(provider, cli.tile_cache_size),
            Some(connectivity),
        )
        .with_tileset_last_modified(modified);
        serve(&cli, loki).await
    }
}

/// Runs the worker loop until Ctrl-C.
async fn serve<P>(cli: &Cli, loki: Loki<P>) -> anyhow::Result<()>
where
    P: GraphTileProvider + Send + Sync + 'static,
{
    let loki = Arc::new(loki);
    let builder = ValhallaMicroserviceBuilder::new(
        Stage::Loki,
        &cli.upstream_socket_endpoint,
        &cli.loopback_socket_endpoint,
    )
    .with_downstream_endpoint(&cli.downstream_socket_endpoint)
    .with_interrupt_endpoint(&cli.interrupt_socket_endpoint);
    let mut service = builder
        .build_http(move |payload: &[u8], _interrupt: &Interrupt| {
            worker_result(handle_http_request(&loki, payload))
        })
        .await?;
    info!(
        "Loki service started (upstream = {}, downstream = {}, loopback = {}, interrupt = {})",
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

/// What the worker sends where, for Loki's outcome.
fn worker_result(outcome: HttpOutcome) -> WorkerResult {
    match outcome {
        HttpOutcome::Forward(api) => WorkerResult::Forward(api),
        HttpOutcome::Respond {
            content_type,
            attachment,
            body,
            ..
        } => {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
            if attachment {
                headers.insert(
                    CONTENT_DISPOSITION,
                    HeaderValue::from_static(GPX_CONTENT_DISPOSITION),
                );
            }
            WorkerResult::HttpResponse {
                status_code: StatusCode::OK,
                headers,
                body,
            }
        }
        HttpOutcome::Error { error, mut api } => WorkerResult::error(&error, &mut api),
    }
}

/// When the tile set was last modified, in seconds since the epoch (0 if unknown),
/// as mainline reports it in `/status`.
fn last_modified(path: &Path) -> u32 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| u32::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// The tile IDs in a tile directory (`<level>/<digits>/.../<digits>.gph`).
fn tile_ids_in(dir: &Path) -> anyhow::Result<Vec<GraphId>> {
    let mut ids = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).with_context(|| format!("read {}", d.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "gph") {
                continue;
            }
            let Ok(relative) = path
                .with_extension("")
                .strip_prefix(dir)
                .map(Path::to_path_buf)
            else {
                continue;
            };
            let parts: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            if let Some((level, rest)) = parts.split_first()
                && let (Ok(level), Ok(tile)) = (level.parse::<u8>(), rest.concat().parse::<u64>())
                && let Ok(id) = GraphId::try_from_components(level, tile, 0)
            {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use valhalla_proto::Api;
    use valhalla_response::{ErrorCode, ValhallaError};

    fn tiles() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../valhalla-graphtile/fixtures/andorra-tiles")
    }

    #[test]
    fn finds_the_tiles_of_a_directory() {
        let mut ids = tile_ids_in(&tiles()).unwrap();
        ids.sort_by_key(GraphId::value);
        assert_eq!(ids.len(), 7);
        assert!(ids.contains(&GraphId::try_from_components(2, 763_926, 0).unwrap()));
        assert!(last_modified(&tiles()) > 0);
    }

    #[test]
    fn answers_locate_and_errors_over_http() {
        let provider = DirectoryGraphTileProvider::new(tiles(), NonZeroUsize::new(16).unwrap());
        let loki = Loki::new(LokiConfig::default(), provider, None);

        let body = r#"{"locations":[{"lat":42.5078,"lon":1.5211}],"costing":"auto"}"#;
        let raw = format!(
            "POST /locate HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let WorkerResult::HttpResponse {
            status_code,
            headers,
            body,
        } = worker_result(handle_http_request(&loki, raw.as_bytes()))
        else {
            panic!("expected a response");
        };
        assert_eq!(status_code, StatusCode::OK);
        assert_eq!(headers[CONTENT_TYPE], "application/json;charset=utf-8");
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json[0]["edges"].as_array().is_some_and(|e| !e.is_empty()));

        let raw = "POST /route HTTP/1.1\r\n\r\n{\"locations\":[{\"lat\":42.5,\"lon\":1.52}],\"costing\":\"auto\"}";
        let WorkerResult::HttpResponse { status_code, .. } =
            worker_result(handle_http_request(&loki, raw.as_bytes()))
        else {
            panic!("expected an error response");
        };
        assert_eq!(status_code, StatusCode::BAD_REQUEST);

        let raw = "GET /route?json={\"locations\":[{\"lat\":42.5078,\"lon\":1.5211},{\"lat\":42.5355,\"lon\":1.583}],\"costing\":\"auto\"} HTTP/1.1\r\n\r\n";
        assert!(matches!(
            worker_result(handle_http_request(&loki, raw.as_bytes())),
            WorkerResult::Forward(_)
        ));

        // Mainline's error for a job that isn't HTTP.
        let mut api = Api::default();
        let expected = WorkerResult::error(
            &ValhallaError::with_detail(
                ErrorCode::LokiUnknown,
                "read the request: the headers do not end",
            ),
            &mut api,
        );
        let actual = worker_result(handle_http_request(&loki, b"nonsense"));
        assert_eq!(format!("{expected:?}"), format!("{actual:?}"));
    }
}
