#![doc = include_str!("../README.md")]

use clap::Parser;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use valhalla_microservice::{Interrupt, Stage, ValhallaMicroserviceBuilder, WorkerResult};
use valhalla_proto::Api;
use valhalla_proto::options::Action;
use valhalla_response::{ErrorCode, ValhallaError};

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
        // Mainline narrates everything else (including actions it does not know),
        // but we do not have a narrative builder yet.
        // 107 is mainline's code for an action it means to support but has not written.
        _ => WorkerResult::error(&ValhallaError::new(ErrorCode::NotImplemented), &mut request),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;
    use serde_json::{Value, json};
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

    #[test]
    fn route_is_not_implemented_yet() {
        let result = handle_message(
            request(Action::Route.into(), options::Format::Json),
            &Interrupt::never(),
        );
        assert_eq!(
            response(result),
            (
                StatusCode::NOT_IMPLEMENTED,
                json!({
                    "error_code": 107,
                    "error": "Not Implemented",
                    "status_code": 501,
                    "status": "Not Implemented",
                })
            )
        );
    }

    #[test]
    fn route_is_not_implemented_yet_in_osrm_format() {
        let result = handle_message(
            request(Action::Route.into(), options::Format::Osrm),
            &Interrupt::never(),
        );
        assert_eq!(
            response(result),
            (
                StatusCode::NOT_IMPLEMENTED,
                json!({"code": "InvalidService", "message": "Service name is invalid."})
            )
        );
    }

    #[test]
    fn unknown_actions_and_missing_options_do_not_panic() {
        for request in [request(99, options::Format::Json), Api::default()] {
            let (status, body) = response(handle_message(request, &Interrupt::never()));
            assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
            assert_eq!(body["error_code"], 107);
        }
    }
}
