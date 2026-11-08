//! The worker loop, mirroring `prime_server`'s `worker_t::work`.

use crate::error::{Error, InvalidMessage, SocketRole};
use crate::http_protocol::HttpRequestInfo;
use crate::interrupt::{Interrupt, InterruptListener};
use crate::result::{WorkerResult, error_response, serialize_http};
use bytes::Bytes;
use core::any::Any;
use core::time::Duration;
use http::StatusCode;
use std::sync::Arc;
use tracing::{trace, warn};
use valhalla_proto::prost::Message;
use valhalla_proto::{Api, Options};
use valhalla_response::{ErrorCode, ValhallaError};
use zerocopy::IntoBytes;
use zeromq::{DealerSocket, PushSocket, Socket, SocketRecv, SocketSend, ZmqError, ZmqMessage};

/// How long a worker waits for a job before advertising again.
///
/// `prime_server`'s workers poll with this timeout (`POLL_TIMEOUT`)
/// and advertise after every poll, busy or not.
/// The upstream proxy only hands work to workers that have advertised,
/// so re-advertising is what lets a worker recover if the proxy restarts.
const ADVERTISE_INTERVAL: Duration = Duration::from_secs(1);

/// The mainline service whose internal error behavior a worker mirrors.
///
/// When the payload from upstream is not a valid `Api`, or the worker function panics,
/// the service still answers the client, as mainline's services do,
/// with the error that mainline's service of this kind would send.
///
/// Loki's jobs are raw HTTP requests rather than `Api` messages
/// (see [`ValhallaMicroserviceBuilder::build_http`]), so it has no decode error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Loki: anything unexpected is error 199.
    Loki,
    /// Thor: undecodable input is error 401, and anything unexpected is error 499.
    Thor,
    /// Odin: every failure is error 299 (Odin wraps even its own coded errors as unknown).
    Odin,
}

impl Stage {
    fn decode_error(self) -> ValhallaError {
        match self {
            // Loki doesn't decode its jobs; its parse failures come from the worker function.
            Stage::Loki => ValhallaError::new(ErrorCode::LokiUnknown),
            Stage::Thor => ValhallaError::with_detail(
                ErrorCode::ThorIntermediateParseFailed,
                "Failed parsing pbf in Thor::Worker",
            ),
            Stage::Odin => ValhallaError::with_detail(
                ErrorCode::OdinUnknown,
                ValhallaError::with_detail(
                    ErrorCode::OdinIntermediateParseFailed,
                    "Failed parsing pbf in Odin::Worker",
                )
                .message(),
            ),
        }
    }

    fn unknown_error(self, detail: &str) -> ValhallaError {
        match self {
            Stage::Loki => ValhallaError::with_detail(ErrorCode::LokiUnknown, detail),
            Stage::Thor => ValhallaError::with_detail(ErrorCode::ThorUnknown, detail),
            Stage::Odin => ValhallaError::with_detail(ErrorCode::OdinUnknown, detail),
        }
    }
}

/// What happened to a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    /// The job was forwarded to the downstream proxy.
    Forwarded,
    /// A response with this status was sent to the loopback.
    Responded(StatusCode),
    /// The job was interrupted, before or during work, and nothing was sent.
    Interrupted,
}

/// A socket that reconnects when it has lost its peer.
///
/// zmq.rs only reconnects SUB sockets on its own.
/// A DEALER or PUSH whose peer goes away (say, a proxy restarts)
/// fails every send with [`ZmqError::ReturnToSender`] from then on,
/// where libzmq would reconnect in the background and queue the message.
/// So on that error we connect again and retry the send once.
struct Connection<S> {
    socket: S,
    role: SocketRole,
    endpoint: String,
}

impl<S: Socket + SocketSend + Send> Connection<S> {
    async fn connect(role: SocketRole, endpoint: &str) -> Result<Self, Error> {
        let mut socket = S::new();
        socket
            .connect(endpoint)
            .await
            .map_err(|source| Error::Connect {
                role,
                endpoint: endpoint.to_string(),
                source,
            })?;
        Ok(Self {
            socket,
            role,
            endpoint: endpoint.to_string(),
        })
    }

    async fn send(&mut self, message: ZmqMessage) -> Result<(), Error> {
        let role = self.role;
        match self.socket.send(message).await {
            Ok(()) => Ok(()),
            Err(ZmqError::ReturnToSender { message, .. }) => {
                warn!(
                    "The {role} socket has no peer; reconnecting to {}",
                    self.endpoint
                );
                self.socket
                    .connect(&self.endpoint)
                    .await
                    .map_err(|source| Error::Connect {
                        role,
                        endpoint: self.endpoint.clone(),
                        source,
                    })?;
                self.socket
                    .send(message)
                    .await
                    .map_err(|source| Error::Send { role, source })
            }
            Err(source) => Err(Error::Send { role, source }),
        }
    }
}

/// A worker function for `Api` jobs.
type ApiWorker = dyn Fn(Api, &Interrupt) -> WorkerResult + Send + Sync;
/// A worker function for raw HTTP jobs.
type HttpWorker = dyn Fn(&[u8], &Interrupt) -> WorkerResult + Send + Sync;

/// What a service's jobs are, and the function that handles them.
#[derive(Clone)]
enum WorkerFn {
    /// Jobs are `Api` messages (every stage after Loki).
    Api(Arc<ApiWorker>),
    /// Jobs are the server's raw HTTP requests (Loki).
    Http(Arc<HttpWorker>),
}

/// A Valhalla-compatible microservice (a `prime_server` worker).
///
/// Each [`tick`](ValhallaMicroservice::tick) handles one job:
/// it advertises to the upstream proxy, waits for a job,
/// runs the worker function on a blocking thread,
/// and sends the result where `prime_server`'s worker would.
pub struct ValhallaMicroservice {
    /// The DEALER connected to the upstream proxy's worker side.
    ///
    /// In the standard Valhalla configuration,
    /// this is `<this service>_out` (e.g., `thor_out` for Thor),
    /// which can be a bit confusing.
    upstream: Connection<DealerSocket>,
    /// The DEALER connected to the downstream proxy's client side.
    ///
    /// In the standard Valhalla configuration,
    /// this is `<downstream service>_in` (e.g., `odin_in` for Thor).
    downstream: Option<Connection<DealerSocket>>,
    /// The PUSH connected to the server's loopback, for final HTTP responses.
    loopback: Connection<PushSocket>,
    /// Interrupts from the server, if the service subscribes to them.
    interrupts: Option<InterruptListener>,
    stage: Stage,
    /// The worker function to be invoked for each job.
    worker_fn: WorkerFn,
}

impl ValhallaMicroservice {
    /// Runs one iteration of the worker loop.
    ///
    /// This advertises to the upstream proxy (again every second until a job arrives),
    /// waits for a job, runs the worker function, and sends its result:
    /// forwarded jobs go to the downstream proxy as `[request info, Api]`,
    /// and responses go to the loopback as `[request info, HTTP response]`,
    /// with the response code recorded in the request info.
    /// A job that the server has already interrupted is skipped.
    ///
    /// Callers should keep calling this until they decide to stop;
    /// none of the errors leave the service unusable.
    ///
    /// # Errors
    ///
    /// - [`Error::Receive`] if receiving from upstream fails.
    /// - [`Error::InvalidMessage`] if the job is malformed.
    ///   If only the payload is bad, the client has been sent an error response.
    /// - [`Error::Send`] or [`Error::Connect`] if the result could not be delivered.
    /// - [`Error::NoDownstream`] if the worker forwarded a job but there is no downstream.
    /// - [`Error::WorkerPanicked`] if the worker function panicked
    ///   (the client has been sent an error response),
    ///   or [`Error::WorkerCancelled`] if the runtime cancelled it.
    ///
    /// # Cancel safety
    ///
    /// Cancelling while waiting for a job loses nothing.
    /// Cancelling while the worker runs drops the job's result
    /// (the worker thread runs to completion regardless).
    pub async fn tick(&mut self) -> Result<TickOutcome, Error> {
        let message = self.next_job().await?;

        let [info_frame, payload] = <[Bytes; 2]>::try_from(message.into_vec())
            .map_err(|frames| InvalidMessage::FrameCount(frames.len()))?;
        let Some(mut request_info) = HttpRequestInfo::from_frame(&info_frame) else {
            return Err(InvalidMessage::RequestInfoSize(info_frame.len()).into());
        };
        let request_id = request_info.id();
        trace!("Handling request ID {request_id}");

        let request = match &self.worker_fn {
            WorkerFn::Api(_) => match Api::decode(payload) {
                Ok(request) => Job::Api(request),
                Err(source) => {
                    warn!("Request {request_id} has an invalid Api payload: {source}");
                    self.respond_with_error(&mut request_info, &self.stage.decode_error(), None)
                        .await?;
                    return Err(InvalidMessage::Protobuf { request_id, source }.into());
                }
            },
            WorkerFn::Http(_) => Job::Http(payload),
        };

        let interrupt = Interrupt::new(
            self.interrupts.as_ref().map(InterruptListener::state),
            request_info.key(),
        );
        if interrupt.is_interrupted() {
            warn!("Request {request_id} was interrupted");
            return Ok(TickOutcome::Interrupted);
        }

        // Keep what an error response needs, since the worker consumes the request.
        // (An HTTP job hasn't been parsed, so its error is in JSON.)
        let options = match &request {
            Job::Api(request) => request.options.as_ref().map(|options| Options {
                action: options.action,
                format: options.format,
                ..Options::default()
            }),
            Job::Http(_) => None,
        };
        let worker_fn = self.worker_fn.clone();
        let result = tokio::task::spawn_blocking(move || match (worker_fn, request) {
            (WorkerFn::Api(f), Job::Api(request)) => f(request, &interrupt),
            (WorkerFn::Http(f), Job::Http(payload)) => f(&payload, &interrupt),
            // `request` was made to match `worker_fn` above.
            _ => WorkerResult::error(
                &ValhallaError::new(ErrorCode::LokiUnknown),
                &mut Api::default(),
            ),
        })
        .await;

        match result {
            Ok(WorkerResult::Forward(api)) => {
                let Some(downstream) = self.downstream.as_mut() else {
                    return Err(Error::NoDownstream { request_id });
                };
                let mut message = ZmqMessage::from(info_frame);
                message.push_back(Bytes::from(api.encode_to_vec()));
                downstream.send(message).await?;
                Ok(TickOutcome::Forwarded)
            }
            Ok(WorkerResult::HttpResponse {
                status_code,
                headers,
                body,
            }) => {
                self.respond(&mut request_info, status_code, headers, &body)
                    .await?;
                Ok(TickOutcome::Responded(status_code))
            }
            Ok(WorkerResult::Interrupted(interrupted)) => {
                warn!("{interrupted}");
                Ok(TickOutcome::Interrupted)
            }
            Err(join_error) if join_error.is_panic() => {
                let message = panic_message(join_error.into_panic());
                let error = self.stage.unknown_error(&message);
                self.respond_with_error(&mut request_info, &error, options)
                    .await?;
                Err(Error::WorkerPanicked {
                    request_id,
                    message,
                })
            }
            Err(_) => Err(Error::WorkerCancelled { request_id }),
        }
    }

    /// Advertises and waits for the next job, re-advertising while idle.
    async fn next_job(&mut self) -> Result<ZmqMessage, Error> {
        loop {
            // `prime_server` advertises with the heartbeat its work function last returned;
            // Valhalla's are always empty, and the proxy only uses them to choose a worker.
            if let Err(e) = self.upstream.send(ZmqMessage::from("")).await {
                // As in `prime_server`, failing to advertise is not fatal; we try again shortly.
                warn!("Failed to advertise to the upstream proxy: {e}");
            }

            match tokio::time::timeout(ADVERTISE_INTERVAL, self.upstream.socket.recv()).await {
                Ok(Ok(message)) => return Ok(message),
                Ok(Err(e)) => return Err(Error::Receive(e)),
                Err(_elapsed) => {}
            }
        }
    }

    /// Sends an HTTP response to the loopback, as `prime_server`'s worker does for final results.
    async fn respond(
        &mut self,
        request_info: &mut HttpRequestInfo,
        status_code: StatusCode,
        headers: http::HeaderMap,
        body: &[u8],
    ) -> Result<(), Error> {
        request_info.set_response_code(status_code);
        let http_response = serialize_http(request_info, status_code, headers, body);

        let mut message = ZmqMessage::from(Bytes::copy_from_slice(request_info.as_bytes()));
        message.push_back(Bytes::from(http_response));
        self.loopback.send(message).await
    }

    /// Sends an error response for a request that the worker function could not handle.
    ///
    /// `options` carries the request's action and format, when they are known,
    /// so the error is serialized for the right format.
    async fn respond_with_error(
        &mut self,
        request_info: &mut HttpRequestInfo,
        error: &ValhallaError,
        options: Option<Options>,
    ) -> Result<(), Error> {
        let mut request = Api {
            options,
            ..Api::default()
        };
        let (status_code, headers, body) = error_response(error, &mut request);
        self.respond(request_info, status_code, headers, &body)
            .await
    }
}

/// A job's payload, decoded for the kind of worker function.
#[expect(
    clippy::large_enum_variant,
    reason = "a job is moved once, onto the worker thread"
)]
enum Job {
    Api(Api),
    Http(Bytes),
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => payload
            .downcast_ref::<&str>()
            .map_or_else(String::new, |message| (*message).to_string()),
    }
}

/// Configures and connects a [`ValhallaMicroservice`].
pub struct ValhallaMicroserviceBuilder<'a> {
    stage: Stage,
    upstream_endpoint: &'a str,
    loopback_endpoint: &'a str,
    downstream_endpoint: Option<&'a str>,
    interrupt_endpoint: Option<&'a str>,
}

impl<'a> ValhallaMicroserviceBuilder<'a> {
    /// Initializes a Valhalla microservice builder.
    ///
    /// The required socket endpoints are typically IPC sockets like `ipc:///tmp/odin_out`
    /// and `ipc:///tmp/loopback`.
    /// `stage` chooses which mainline service's internal errors to mirror.
    pub fn new(
        stage: Stage,
        upstream_endpoint: &'a str,
        loopback_endpoint: &'a str,
    ) -> ValhallaMicroserviceBuilder<'a> {
        ValhallaMicroserviceBuilder {
            stage,
            upstream_endpoint,
            loopback_endpoint,
            downstream_endpoint: None,
            interrupt_endpoint: None,
        }
    }

    /// Adds a downstream endpoint (the next proxy's `_in` socket).
    ///
    /// Terminal (last in the chain) services never need this,
    /// but others do.
    #[must_use]
    pub fn with_downstream_endpoint(
        self,
        downstream_endpoint: &'a str,
    ) -> ValhallaMicroserviceBuilder<'a> {
        ValhallaMicroserviceBuilder {
            downstream_endpoint: Some(downstream_endpoint),
            ..self
        }
    }

    /// Subscribes to the server's interrupts (`httpd.service.interrupt` in Valhalla's config).
    ///
    /// Without this, the service works on every job it receives,
    /// even ones the server has given up on.
    #[must_use]
    pub fn with_interrupt_endpoint(
        self,
        interrupt_endpoint: &'a str,
    ) -> ValhallaMicroserviceBuilder<'a> {
        ValhallaMicroserviceBuilder {
            interrupt_endpoint: Some(interrupt_endpoint),
            ..self
        }
    }

    /// Connects the sockets and builds the service.
    ///
    /// The worker function gets each job's request and an [`Interrupt`] to check during long work.
    /// It runs on Tokio's blocking thread pool, so it may take as long as it needs.
    ///
    /// # Rules for worker functions
    ///
    /// - Don't panic.
    ///   The service answers the client with an error if you do,
    ///   but the request is lost and the panic is reported as [`Error::WorkerPanicked`].
    /// - Pass through what you don't understand.
    ///   [`WorkerResult::Forward`] sends the `Api` exactly as you return it.
    ///
    /// # Errors
    ///
    /// This fails with [`Error::Connect`] (or [`Error::Subscribe`])
    /// if a socket cannot be connected.
    /// zmq.rs keeps trying to connect to an endpoint that does not exist yet
    /// for up to its connect timeout (30 seconds by default) before giving up.
    pub async fn build<F>(self, worker_fn: F) -> Result<ValhallaMicroservice, Error>
    where
        F: Fn(Api, &Interrupt) -> WorkerResult + Send + Sync + 'static,
    {
        self.connect(WorkerFn::Api(Arc::new(worker_fn))).await
    }

    /// Connects the sockets and builds a service whose jobs are raw HTTP requests,
    /// as the server sends them to Loki (which parses them itself).
    ///
    /// The worker function gets each job's request bytes and an [`Interrupt`].
    /// Otherwise this is [`build`](Self::build), with the same rules.
    ///
    /// # Errors
    ///
    /// As for [`build`](Self::build).
    pub async fn build_http<F>(self, worker_fn: F) -> Result<ValhallaMicroservice, Error>
    where
        F: Fn(&[u8], &Interrupt) -> WorkerResult + Send + Sync + 'static,
    {
        self.connect(WorkerFn::Http(Arc::new(worker_fn))).await
    }

    async fn connect(self, worker_fn: WorkerFn) -> Result<ValhallaMicroservice, Error> {
        let upstream = Connection::connect(SocketRole::Upstream, self.upstream_endpoint).await?;
        let downstream = match self.downstream_endpoint {
            Some(endpoint) => Some(Connection::connect(SocketRole::Downstream, endpoint).await?),
            None => None,
        };
        let loopback = Connection::connect(SocketRole::Loopback, self.loopback_endpoint).await?;
        let interrupts = match self.interrupt_endpoint {
            Some(endpoint) => Some(InterruptListener::start(endpoint).await?),
            None => None,
        };

        Ok(ValhallaMicroservice {
            upstream,
            downstream,
            loopback,
            interrupts,
            stage: self.stage,
            worker_fn,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thor_decode_error_matches_mainline() {
        let error = Stage::Thor.decode_error();
        assert_eq!(error.code(), ErrorCode::ThorIntermediateParseFailed);
        assert_eq!(
            error.message(),
            "Failed to parse intermediate request format: Failed parsing pbf in Thor::Worker"
        );
        assert_eq!(error.http_status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn odin_decode_error_matches_mainline() {
        // Odin throws 200, then its catch-all rewraps everything as 299 with the message.
        let error = Stage::Odin.decode_error();
        assert_eq!(error.code(), ErrorCode::OdinUnknown);
        assert_eq!(
            error.message(),
            "Unknown: Failed to parse intermediate request format: Failed parsing pbf in Odin::Worker"
        );
    }

    #[test]
    fn loki_errors_are_unknown() {
        let error = Stage::Loki.unknown_error("boom");
        assert_eq!(error.code(), ErrorCode::LokiUnknown);
        assert_eq!(error.message(), "Unknown: boom");
        assert_eq!(Stage::Loki.decode_error().code(), ErrorCode::LokiUnknown);
    }

    #[test]
    fn panic_messages_are_extracted() {
        assert_eq!(panic_message(Box::new("static")), "static");
        assert_eq!(panic_message(Box::new(String::from("owned"))), "owned");
        assert_eq!(panic_message(Box::new(42)), "");
    }
}
