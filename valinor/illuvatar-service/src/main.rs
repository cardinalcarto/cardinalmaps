#![doc = include_str!("../README.md")]

use clap::Parser;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use valhalla_microservice::{Interrupt, Stage, ValhallaMicroserviceBuilder, WorkerResult};
use valhalla_proto::Api;
use valhalla_proto::options::Action;

mod handlers;

#[derive(Parser)]
#[command(version, about, long_about = None)]
#[expect(
    clippy::struct_field_names,
    reason = "the field names are the command line flags and environment variables"
)]
struct Cli {
    /// The upstream socket to listen on.
    #[arg(env, long, default_value = "ipc:///tmp/odin_out")]
    upstream_socket_endpoint: String,

    /// The Valhalla loopback socket endpoint.
    #[arg(env, long, default_value = "ipc:///tmp/loopback")]
    loopback_socket_endpoint: String,

    /// The Valhalla interrupt socket endpoint (`httpd.service.interrupt`).
    #[arg(env, long, default_value = "ipc:///tmp/interrupt")]
    interrupt_socket_endpoint: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let upstream_socket_endpoint = &cli.upstream_socket_endpoint;
    let loopback_socket_endpoint = &cli.loopback_socket_endpoint;
    let interrupt_socket_endpoint = &cli.interrupt_socket_endpoint;

    tracing_subscriber::registry()
        // Standard logger, configured via the RUST_LOG env variable
        .with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env()))
        // TODO: We should probably optionally add Sentry here (behind a feature flag).
        .init();

    let service_builder = ValhallaMicroserviceBuilder::new(
        Stage::Odin,
        upstream_socket_endpoint,
        loopback_socket_endpoint,
    )
    .with_interrupt_endpoint(interrupt_socket_endpoint);
    let mut service = service_builder.build(handle_message).await?;

    info!(
        "Ilúvatar service started (upstream = {upstream_socket_endpoint}, loopback = {loopback_socket_endpoint}, interrupt = {interrupt_socket_endpoint})"
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

/// Handles a job from Thor, as mainline's `odin_worker_t::work` does.
///
/// A missing `options` message is read as the default (proto3 semantics),
/// which is what mainline sees too.
fn handle_message(mut request: Api, _interrupt: &Interrupt) -> WorkerResult {
    let action = request.options.as_ref().map_or(0, |options| options.action);
    match Action::try_from(action) {
        Ok(Action::Status) => handlers::status::status(&mut request),
        // Mainline narrates everything else, including actions it does not know;
        // those have no trip, so narration fails with mainline's error.
        _ => handlers::directions::directions(&mut request),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;
    use http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
    use serde_json::Value;
    use valhalla_proto::prost::Message;
    use valhalla_proto::{Options, options};

    fn response(result: WorkerResult) -> (StatusCode, Value) {
        let WorkerResult::HttpResponse {
            status_code, body, ..
        } = result
        else {
            panic!("expected an HTTP response");
        };
        (
            status_code,
            serde_json::from_slice(&body).expect("JSON body"),
        )
    }

    fn request(action: i32, format: options::Format) -> Api {
        Api {
            options: Some(Options {
                action,
                format: format.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// A captured mainline case's Thor→Odin message, with its trip
    /// (see `tools/oracle/copy-tyr-fixtures.sh` for the layout).
    fn captured(case: &str, format: &str) -> Api {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../valinor-tyr/fixtures/captures/andorra")
            .join(case);
        let read = |path: std::path::PathBuf| {
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            Api::decode(bytes.as_slice()).expect("an Api message")
        };
        let mut api = read(dir.join(format).join("thor-odin.bin"));
        api.trip = read(dir.join("trip.bin")).trip;
        api
    }

    fn http(result: WorkerResult) -> (StatusCode, http::HeaderMap, Vec<u8>) {
        let WorkerResult::HttpResponse {
            status_code,
            headers,
            body,
        } = result
        else {
            panic!("expected an HTTP response");
        };
        (status_code, headers, body)
    }

    #[test]
    fn routes_are_narrated_and_serialized() {
        for (format, content_type) in [
            ("json", "application/json;charset=utf-8"),
            ("osrm", "application/json;charset=utf-8"),
            ("pbf", "application/x-protobuf"),
        ] {
            let (status, headers, body) = http(handle_message(
                captured("auto_town", format),
                &Interrupt::never(),
            ));
            assert_eq!(status, StatusCode::OK, "{format}");
            assert_eq!(headers[CONTENT_TYPE], content_type, "{format}");
            assert!(!headers.contains_key(CONTENT_DISPOSITION), "{format}");
            match format {
                "json" => {
                    let body: Value = serde_json::from_slice(&body).expect("JSON");
                    assert!(
                        !body["trip"]["legs"][0]["maneuvers"]
                            .as_array()
                            .expect("maneuvers")
                            .is_empty()
                    );
                }
                "osrm" => {
                    let body: Value = serde_json::from_slice(&body).expect("JSON");
                    assert_eq!(body["code"], "Ok");
                    let step = &body["routes"][0]["legs"][0]["steps"][0];
                    assert!(
                        !step["bannerInstructions"]
                            .as_array()
                            .expect("banners")
                            .is_empty()
                    );
                    assert!(
                        !step["voiceInstructions"]
                            .as_array()
                            .expect("voice")
                            .is_empty()
                    );
                }
                _ => {
                    let api = Api::decode(body.as_slice()).expect("a PBF response");
                    assert!(api.directions.is_some());
                }
            }
        }
    }

    #[test]
    fn gpx_is_sent_as_an_attachment() {
        let mut api = captured("auto_town", "json");
        if let Some(options) = api.options.as_mut() {
            options.format = options::Format::Gpx.into();
        }
        let (status, headers, body) = http(handle_message(api, &Interrupt::never()));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[CONTENT_TYPE], "application/gpx+xml;charset=utf-8");
        assert_eq!(
            headers[CONTENT_DISPOSITION],
            "attachment; filename=route.gpx"
        );
        assert!(String::from_utf8(body).expect("UTF-8").contains("<gpx"));
    }

    #[test]
    fn narration_errors_are_serialized_in_the_request_format() {
        let mut api = captured("auto_town", "osrm");
        api.trip = None;
        let (status, _, body) = http(handle_message(api, &Interrupt::never()));
        let body: Value = serde_json::from_slice(&body).expect("JSON");
        assert_ne!(status, StatusCode::OK);
        assert!(body.get("code").is_some(), "{body}");
        assert!(body.get("error_code").is_none(), "{body}");
    }

    #[test]
    fn unknown_actions_and_missing_options_do_not_panic() {
        for request in [request(99, options::Format::Json), Api::default()] {
            // Neither has a trip, so narration fails, as it does in mainline.
            let (status, body) = response(handle_message(request, &Interrupt::never()));
            assert_ne!(status, StatusCode::OK);
            assert!(body["error_code"].is_u64(), "{body}");
        }
    }
}
