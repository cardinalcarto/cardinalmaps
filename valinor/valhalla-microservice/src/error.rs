//! Errors from running a worker.

use core::fmt;
use thiserror::Error;
use valhalla_proto::prost;
use zeromq::ZmqError;

/// The sockets a worker uses, named as `prime_server`'s `worker_t` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketRole {
    /// The DEALER connected to the upstream proxy, which sends us work.
    Upstream,
    /// The DEALER connected to the downstream proxy, which we forward work to.
    Downstream,
    /// The PUSH connected to the server's loopback, which takes final responses.
    Loopback,
    /// The SUB connected to the server's interrupt publisher.
    Interrupt,
}

impl fmt::Display for SocketRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SocketRole::Upstream => "upstream",
            SocketRole::Downstream => "downstream",
            SocketRole::Loopback => "loopback",
            SocketRole::Interrupt => "interrupt",
        })
    }
}

/// A malformed job from the upstream proxy.
#[derive(Debug, Error)]
pub enum InvalidMessage {
    /// `prime_server` jobs have exactly two frames: the request info and the payload.
    #[error("parse the job: expected two frames (request info and an Api protobuf), got {0}")]
    FrameCount(usize),
    /// The request info frame is a fixed 12-byte struct.
    #[error("parse the request info: expected 12 bytes, got {0}")]
    RequestInfoSize(usize),
    /// The payload is not a valid `valhalla.Api` message.
    ///
    /// A Valhalla error response has already been sent to the client
    /// (see [`Stage`](crate::Stage)).
    #[error("decode the Api payload of request {request_id}: {source}")]
    Protobuf {
        /// `prime_server`'s ID for the request.
        request_id: u32,
        /// The decoding error.
        source: prost::DecodeError,
    },
}

/// An error from one iteration of a worker's loop.
#[derive(Debug, Error)]
pub enum Error {
    /// Connecting (or reconnecting) a socket failed.
    #[error("connect the {role} socket to {endpoint}: {source}")]
    Connect {
        /// The socket that failed to connect.
        role: SocketRole,
        /// The endpoint it was connecting to.
        endpoint: String,
        /// The underlying error.
        source: ZmqError,
    },
    /// Subscribing to interrupts failed.
    #[error("subscribe to interrupts: {0}")]
    Subscribe(ZmqError),
    /// Receiving from the upstream proxy failed.
    #[error("receive from the upstream proxy: {0}")]
    Receive(ZmqError),
    /// Sending a message failed, so the job's result was lost.
    ///
    /// The client will eventually get `prime_server`'s timeout response.
    #[error("send to the {role} socket: {source}")]
    Send {
        /// The socket that failed.
        role: SocketRole,
        /// The underlying error.
        source: ZmqError,
    },
    /// The job from upstream was malformed.
    #[error(transparent)]
    InvalidMessage(#[from] InvalidMessage),
    /// The worker asked to forward a job, but the service has no downstream endpoint.
    ///
    /// This is a configuration error.
    /// The job is dropped, and the client will eventually get `prime_server`'s timeout response.
    #[error("forward request {request_id}: this service has no downstream endpoint")]
    NoDownstream {
        /// `prime_server`'s ID for the request.
        request_id: u32,
    },
    /// The worker function panicked.
    ///
    /// A Valhalla error response has already been sent to the client
    /// (see [`Stage`](crate::Stage)).
    #[error("run the worker for request {request_id}: it panicked ({message})")]
    WorkerPanicked {
        /// `prime_server`'s ID for the request.
        request_id: u32,
        /// The panic message, if it had one.
        message: String,
    },
    /// The worker task was cancelled by the runtime (for example, during shutdown).
    #[error("run the worker for request {request_id}: the task was cancelled")]
    WorkerCancelled {
        /// `prime_server`'s ID for the request.
        request_id: u32,
    },
}
