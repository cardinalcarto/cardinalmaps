//! Drives a worker over real sockets, with the test playing `prime_server`'s roles:
//! the upstream proxy (a ROUTER that hands out jobs to workers that advertise),
//! the downstream proxy (a ROUTER that takes forwarded jobs),
//! the server's loopback (a PULL), and its interrupt publisher (a PUB).

use bytes::Bytes;
use core::time::Duration;
use http::StatusCode;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::timeout;
use valhalla_microservice::{
    Error, Interrupt, InvalidMessage, Stage, TickOutcome, ValhallaMicroserviceBuilder, WorkerResult,
};
use valhalla_proto::prost::Message;
use valhalla_proto::{Api, options};
use valhalla_response::{ErrorCode, ValhallaError};
use zeromq::{PubSocket, PullSocket, RouterSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

/// Long enough for anything that should happen to happen.
const PATIENCE: Duration = Duration::from_secs(10);
/// How long to wait to be confident that something will not happen.
const QUIET: Duration = Duration::from_millis(300);

fn manifest_path(relative: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// A request info frame and its Loki→Thor `Api` from the Andorra captures.
///
/// The request timestamp is moved to now:
/// workers forget interrupts for requests more than ten minutes old, as `prime_server` does,
/// so an interrupt for the captured request would be ignored.
fn captured_job() -> (Bytes, Bytes) {
    let mut info = fs::read(manifest_path(
        "fixtures/captures/andorra/auto_town/json/loki-thor.info.bin",
    ))
    .expect("info fixture");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs();
    info[4..8].copy_from_slice(&u32::try_from(now).expect("before 2106").to_le_bytes());
    let api = fs::read(manifest_path(
        "../valhalla-proto/fixtures/captures/andorra/auto_town/json/loki-thor.bin",
    ))
    .expect("Api fixture");
    (Bytes::from(info), Bytes::from(api))
}

fn key_of(info: &[u8]) -> u64 {
    let id = u32::from_le_bytes([info[0], info[1], info[2], info[3]]);
    let timestamp = u32::from_le_bytes([info[4], info[5], info[6], info[7]]);
    u64::from(id) | (u64::from(timestamp) << 32)
}

struct Harness {
    upstream_endpoint: String,
    upstream: RouterSocket,
    downstream: RouterSocket,
    loopback: PullSocket,
    interrupts: PubSocket,
    outcomes: mpsc::UnboundedReceiver<Result<TickOutcome, Error>>,
    /// The worker's identity on the upstream ROUTER, learned from its first advertisement.
    worker: Bytes,
}

async fn bind<S: Socket>(socket: &mut S) -> String {
    socket
        .bind("tcp://127.0.0.1:0")
        .await
        .expect("bind")
        .to_string()
}

async fn recv<S: SocketRecv>(socket: &mut S) -> Vec<Bytes> {
    timeout(PATIENCE, socket.recv())
        .await
        .expect("a message in time")
        .expect("receive")
        .into_vec()
}

impl Harness {
    async fn start<F>(worker_fn: F, with_downstream: bool) -> Harness
    where
        F: Fn(Api, &Interrupt) -> WorkerResult + Send + Sync + 'static,
    {
        let mut upstream = RouterSocket::new();
        let mut downstream = RouterSocket::new();
        let mut loopback = PullSocket::new();
        let mut interrupts = PubSocket::new();
        let upstream_endpoint = bind(&mut upstream).await;
        let downstream_endpoint = bind(&mut downstream).await;
        let loopback_endpoint = bind(&mut loopback).await;
        let interrupt_endpoint = bind(&mut interrupts).await;

        let mut builder =
            ValhallaMicroserviceBuilder::new(Stage::Thor, &upstream_endpoint, &loopback_endpoint)
                .with_interrupt_endpoint(&interrupt_endpoint);
        if with_downstream {
            builder = builder.with_downstream_endpoint(&downstream_endpoint);
        }
        let mut service = timeout(PATIENCE, builder.build(worker_fn))
            .await
            .expect("connected in time")
            .expect("connected");

        let (sender, outcomes) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let outcome = service.tick().await;
                if sender.send(outcome).is_err() {
                    break;
                }
            }
        });

        let mut harness = Harness {
            upstream_endpoint,
            upstream,
            downstream,
            loopback,
            interrupts,
            outcomes,
            worker: Bytes::new(),
        };
        harness.worker = harness.expect_advertisement().await;
        harness
    }

    /// Waits for the worker to advertise, returning its identity.
    async fn expect_advertisement(&mut self) -> Bytes {
        let frames = recv(&mut self.upstream).await;
        assert_eq!(frames.len(), 2, "an advertisement is [identity, heartbeat]");
        assert!(frames[1].is_empty(), "Valhalla's heartbeats are empty");
        frames[0].clone()
    }

    /// Sends a job to the worker, as the upstream proxy does: `[worker, frames...]`.
    async fn send_job(&mut self, frames: &[Bytes]) {
        let mut message = ZmqMessage::from(self.worker.clone());
        for frame in frames {
            message.push_back(frame.clone());
        }
        self.upstream.send(message).await.expect("send job");
    }

    async fn outcome(&mut self) -> Result<TickOutcome, Error> {
        timeout(PATIENCE, self.outcomes.recv())
            .await
            .expect("an outcome in time")
            .expect("the service is running")
    }

    /// Publishes an interrupt for a request until the worker has surely seen it.
    ///
    /// PUB drops messages for subscribers whose subscription has not arrived yet,
    /// so we publish repeatedly rather than once.
    async fn interrupt(&mut self, key: u64) {
        for _ in 0..15 {
            self.interrupts
                .send(ZmqMessage::from(key.to_le_bytes().to_vec()))
                .await
                .expect("publish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn assert_loopback_quiet(&mut self) {
        assert!(
            timeout(QUIET, self.loopback.recv()).await.is_err(),
            "nothing should reach the loopback"
        );
    }
}

fn forward_unchanged(api: Api, _: &Interrupt) -> WorkerResult {
    WorkerResult::forward(api)
}

#[tokio::test(flavor = "multi_thread")]
async fn forwards_request_info_and_api_downstream() {
    let mut harness = Harness::start(forward_unchanged, true).await;
    let (info, api) = captured_job();
    harness.send_job(&[info.clone(), api.clone()]).await;

    assert_eq!(harness.outcome().await.expect("ok"), TickOutcome::Forwarded);
    let frames = recv(&mut harness.downstream).await;
    assert_eq!(frames.len(), 3, "[identity, request info, Api]");
    assert_eq!(frames[1], info, "the request info passes through untouched");
    assert_eq!(frames[2].len(), api.len());
    assert_eq!(
        Api::decode(frames[2].clone()).expect("valid Api"),
        Api::decode(api).expect("valid Api")
    );

    // The worker is ready for more.
    harness.expect_advertisement().await;
    harness.assert_loopback_quiet().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sends_responses_to_the_loopback() {
    let mut harness = Harness::start(
        |mut api: Api, _: &Interrupt| {
            WorkerResult::error(&ValhallaError::new(ErrorCode::NoSuitableEdges), &mut api)
        },
        true,
    )
    .await;
    let (info, mut api) = captured_job();
    // Ask for OSRM format, so we see the error serialized for it.
    let mut request = Api::decode(api).expect("valid Api");
    if let Some(options) = request.options.as_mut() {
        options.format = options::Format::Osrm.into();
    }
    api = Bytes::from(request.encode_to_vec());
    harness.send_job(&[info.clone(), api]).await;

    assert_eq!(
        harness.outcome().await.expect("ok"),
        TickOutcome::Responded(StatusCode::BAD_REQUEST)
    );
    let frames = recv(&mut harness.loopback).await;
    assert_eq!(frames.len(), 2, "[request info, HTTP response]");
    // Only the response code (bits 5-14 of the bitfield) changes.
    let mut expected_info = info.to_vec();
    let bitfield = u16::from_le_bytes([info[8], info[9]]) | (400 << 5);
    expected_info[8..10].copy_from_slice(&bitfield.to_le_bytes());
    assert_eq!(frames[0].as_ref(), expected_info.as_slice());
    assert_eq!(
        String::from_utf8(frames[1].to_vec()).expect("UTF-8"),
        "HTTP/1.1 400 Bad Request\r\n\
         content-type: application/json;charset=utf-8\r\n\
         access-control-allow-origin: *\r\n\
         content-length: 104\r\n\r\n\
         {\"code\":\"NoSegment\",\"message\":\"One of the supplied input coordinates could not snap to street segment.\"}"
    );
    harness.expect_advertisement().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn readvertises_while_idle() {
    let mut harness = Harness::start(forward_unchanged, true).await;
    // prime_server workers advertise after every one-second poll, so a restarted proxy finds them.
    let worker = harness.expect_advertisement().await;
    assert_eq!(worker, harness.worker);
}

#[tokio::test(flavor = "multi_thread")]
async fn skips_jobs_that_were_already_interrupted() {
    let mut harness = Harness::start(forward_unchanged, true).await;
    let (info, api) = captured_job();
    harness.interrupt(key_of(&info)).await;
    harness.send_job(&[info, api]).await;

    assert_eq!(
        harness.outcome().await.expect("ok"),
        TickOutcome::Interrupted
    );
    harness.assert_loopback_quiet().await;
    assert!(
        timeout(QUIET, harness.downstream.recv()).await.is_err(),
        "nothing should be forwarded"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupts_for_other_requests_do_not_skip_jobs() {
    let mut harness = Harness::start(forward_unchanged, true).await;
    let (info, api) = captured_job();
    harness.interrupt(key_of(&info) + 1).await;
    harness.send_job(&[info, api]).await;

    assert_eq!(harness.outcome().await.expect("ok"), TickOutcome::Forwarded);
}

#[tokio::test(flavor = "multi_thread")]
async fn workers_can_bail_when_interrupted() {
    // The worker waits to be interrupted, checking as long-running work would.
    let mut harness = Harness::start(
        |api: Api, interrupt: &Interrupt| {
            for _ in 0..1_000 {
                if let Err(interrupted) = interrupt.check() {
                    return interrupted.into();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            WorkerResult::forward(api)
        },
        true,
    )
    .await;
    let (info, api) = captured_job();
    harness.send_job(&[info.clone(), api]).await;
    harness.interrupt(key_of(&info)).await;

    assert_eq!(
        harness.outcome().await.expect("ok"),
        TickOutcome::Interrupted
    );
    harness.assert_loopback_quiet().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_undecodable_payloads_with_an_error() {
    let mut harness = Harness::start(forward_unchanged, true).await;
    let (info, _) = captured_job();
    // A truncated length-delimited field.
    harness
        .send_job(&[info, Bytes::from_static(&[0x0a, 0x05, 0x01])])
        .await;

    assert!(matches!(
        harness.outcome().await,
        Err(Error::InvalidMessage(InvalidMessage::Protobuf {
            request_id: 2,
            ..
        }))
    ));
    let frames = recv(&mut harness.loopback).await;
    let response = String::from_utf8(frames[1].to_vec()).expect("UTF-8");
    assert!(
        response.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "{response}"
    );
    assert!(
        response.ends_with(
            r#"{"error_code":401,"error":"Failed to parse intermediate request format: Failed parsing pbf in Thor::Worker","status_code":500,"status":"Internal Server Error"}"#
        ),
        "{response}"
    );
    harness.expect_advertisement().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_malformed_jobs() {
    let mut harness = Harness::start(forward_unchanged, true).await;
    let (info, api) = captured_job();

    harness.send_job(std::slice::from_ref(&info)).await;
    assert!(matches!(
        harness.outcome().await,
        Err(Error::InvalidMessage(InvalidMessage::FrameCount(1)))
    ));
    harness.expect_advertisement().await;

    harness
        .send_job(&[info.clone(), api.clone(), api.clone()])
        .await;
    assert!(matches!(
        harness.outcome().await,
        Err(Error::InvalidMessage(InvalidMessage::FrameCount(3)))
    ));
    harness.expect_advertisement().await;

    harness.send_job(&[info.slice(..11), api]).await;
    assert!(matches!(
        harness.outcome().await,
        Err(Error::InvalidMessage(InvalidMessage::RequestInfoSize(11)))
    ));
    harness.assert_loopback_quiet().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_worker_panics_with_an_error() {
    let mut harness = Harness::start(
        |_: Api, _: &Interrupt| -> WorkerResult { panic!("boom") },
        true,
    )
    .await;
    let (info, api) = captured_job();
    harness.send_job(&[info, api]).await;

    assert!(matches!(
        harness.outcome().await,
        Err(Error::WorkerPanicked { request_id: 2, message }) if message == "boom"
    ));
    let frames = recv(&mut harness.loopback).await;
    let response = String::from_utf8(frames[1].to_vec()).expect("UTF-8");
    assert!(
        response.ends_with(
            r#"{"error_code":499,"error":"Unknown: boom","status_code":500,"status":"Internal Server Error"}"#
        ),
        "{response}"
    );
    harness.expect_advertisement().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarding_without_a_downstream_is_an_error() {
    let mut harness = Harness::start(forward_unchanged, false).await;
    let (info, api) = captured_job();
    harness.send_job(&[info, api]).await;

    assert!(matches!(
        harness.outcome().await,
        Err(Error::NoDownstream { request_id: 2 })
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnects_after_the_upstream_proxy_restarts() {
    let mut harness = Harness::start(forward_unchanged, true).await;

    // Restart the upstream proxy on the same address.
    // zmq.rs does not reconnect DEALER sockets itself; the service must,
    // and its periodic advertisement is what tells the new proxy about it.
    drop(core::mem::replace(
        &mut harness.upstream,
        RouterSocket::new(),
    ));
    // zmq.rs closes the old listener in the background, so the address may be busy briefly.
    let endpoint = harness.upstream_endpoint.clone();
    let mut attempts = 0;
    while let Err(e) = harness.upstream.bind(&endpoint).await {
        attempts += 1;
        assert!(attempts < 100, "failed to rebind {endpoint}: {e}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    harness.worker = harness.expect_advertisement().await;

    let (info, api) = captured_job();
    harness.send_job(&[info, api]).await;
    assert_eq!(harness.outcome().await.expect("ok"), TickOutcome::Forwarded);
}
