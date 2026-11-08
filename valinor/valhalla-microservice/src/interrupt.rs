//! Interrupt handling, mirroring `prime_server`'s `worker_t::handle_interrupt`.
//!
//! `prime_server`'s HTTP server publishes an interrupt
//! when it gives up on a request (it timed out, or the client disconnected)
//! so that workers can skip or abandon work nobody will receive.
//! Each interrupt is a single 8-byte frame holding the request's key
//! (see [`HttpRequestInfo::key`](crate::http_protocol::HttpRequestInfo::key))
//! in host byte order, which we read as little-endian.
//!
//! Workers subscribe to everything and remember the keys they have seen.
//! Before starting a job, a worker checks whether its key was interrupted, and skips it if so;
//! during the job, the work function may check again (`prime_server`'s `bail` callback).
//! Either way, an interrupted job sends nothing to the loopback or downstream.
//! Keys whose request timestamp is more than [`INTERRUPT_AGE_CUTOFF_SECS`] old are forgotten.
//!
//! `prime_server` drains the subscriber only when it checks,
//! one message per check.
//! We instead read interrupts on a background task as they arrive,
//! so a check never misses an interrupt that has already been delivered,
//! and a long-running job sees one as soon as it checks.

use crate::error::{Error, SocketRole};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use zeromq::{Socket, SocketRecv, SubSocket};

/// How long `prime_server` remembers an interrupt, measured from the request's timestamp.
pub(crate) const INTERRUPT_AGE_CUTOFF_SECS: u32 = 600;

/// How long to wait before receiving again after the subscriber reports an error,
/// so a persistent failure cannot become a busy loop.
const ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// The interrupts a worker has seen and not yet forgotten.
#[derive(Debug, Default)]
pub(crate) struct InterruptState {
    keys: HashSet<u64>,
    /// Keys in the order they arrived, for expiry.
    ///
    /// Like `prime_server`'s history this is only loosely ordered by timestamp,
    /// so a key can outlive the cutoff while an older-arriving, newer key sits in front of it.
    history: VecDeque<u64>,
}

impl InterruptState {
    /// Records an interrupt and forgets any that are too old as of `now` (UNIX seconds).
    pub(crate) fn record(&mut self, key: u64, now: u32) {
        if self.keys.insert(key) {
            self.history.push_back(key);
        }
        let cutoff = now.saturating_sub(INTERRUPT_AGE_CUTOFF_SECS);
        while let Some(&oldest) = self.history.front()
            && timestamp_of(oldest) < cutoff
        {
            self.keys.remove(&oldest);
            self.history.pop_front();
        }
    }

    pub(crate) fn contains(&self, key: u64) -> bool {
        self.keys.contains(&key)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }
}

/// The request timestamp stored in the high 32 bits of a key.
fn timestamp_of(key: u64) -> u32 {
    // The shift leaves exactly 32 bits.
    (key >> 32) as u32
}

/// The current time in UNIX seconds, saturating at `u32::MAX` as `prime_server`'s timestamps would.
fn now() -> u32 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    u32::try_from(secs).unwrap_or(u32::MAX)
}

fn lock(state: &Mutex<InterruptState>) -> std::sync::MutexGuard<'_, InterruptState> {
    // The state is a set of keys, which stays consistent even if a holder panicked.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A job's view of the interrupts, passed to the worker function.
///
/// This plays the role of `prime_server`'s interrupt callback (`bail`):
/// long-running work should call [`Interrupt::check`] now and then
/// and stop with the [`Interrupted`] it returns.
#[derive(Debug, Clone)]
pub struct Interrupt {
    state: Option<Arc<Mutex<InterruptState>>>,
    key: u64,
}

impl Interrupt {
    pub(crate) fn new(state: Option<Arc<Mutex<InterruptState>>>, key: u64) -> Self {
        Self { state, key }
    }

    /// A handle that is never interrupted, for running a worker function outside a service.
    pub fn never() -> Self {
        Self {
            state: None,
            key: 0,
        }
    }

    /// Whether the server has interrupted this job.
    pub fn is_interrupted(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| lock(state).contains(self.key))
    }

    /// Returns an error if the server has interrupted this job.
    ///
    /// # Errors
    ///
    /// Returns [`Interrupted`] if the server has given up on the request.
    /// Return it from the worker function (it converts into [`WorkerResult`](crate::WorkerResult))
    /// so that nothing is sent.
    pub fn check(&self) -> Result<(), Interrupted> {
        if self.is_interrupted() {
            Err(Interrupted {
                request_id: self.request_id(),
            })
        } else {
            Ok(())
        }
    }

    fn request_id(&self) -> u32 {
        // The low 32 bits of the key are the request ID.
        #[expect(clippy::cast_possible_truncation)]
        let id = self.key as u32;
        id
    }
}

/// The server interrupted a job (see [`Interrupt::check`]).
///
/// This can only be created by an [`Interrupt`] that was actually interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("finish request {request_id}: the server interrupted it")]
pub struct Interrupted {
    request_id: u32,
}

impl Interrupted {
    /// `prime_server`'s ID for the interrupted request.
    #[inline]
    pub fn request_id(&self) -> u32 {
        self.request_id
    }
}

/// Subscribes to interrupts and records them on a background task.
pub(crate) struct InterruptListener {
    state: Arc<Mutex<InterruptState>>,
    task: JoinHandle<()>,
}

impl InterruptListener {
    /// Connects to the server's interrupt publisher and starts listening.
    pub(crate) async fn start(endpoint: &str) -> Result<Self, Error> {
        let mut socket = SubSocket::new();
        socket
            .connect(endpoint)
            .await
            .map_err(|source| Error::Connect {
                role: SocketRole::Interrupt,
                endpoint: endpoint.to_string(),
                source,
            })?;
        socket.subscribe("").await.map_err(Error::Subscribe)?;

        let state = Arc::new(Mutex::new(InterruptState::default()));
        let task = tokio::spawn(listen(socket, Arc::clone(&state)));
        Ok(Self { state, task })
    }

    pub(crate) fn state(&self) -> Arc<Mutex<InterruptState>> {
        Arc::clone(&self.state)
    }
}

impl Drop for InterruptListener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn listen(mut socket: SubSocket, state: Arc<Mutex<InterruptState>>) {
    loop {
        match socket.recv().await {
            Ok(message) => {
                let key = match message.get(0) {
                    Some(frame) if message.len() == 1 => <[u8; 8]>::try_from(frame.as_ref())
                        .ok()
                        .map(u64::from_le_bytes),
                    _ => None,
                };
                let Some(key) = key else {
                    warn!(
                        "Ignoring an interrupt that is not a single 8-byte frame ({} frames)",
                        message.len()
                    );
                    continue;
                };
                debug!("Interrupt for request {}", key & u64::from(u32::MAX));
                lock(&state).record(key, now());
            }
            Err(e) => {
                // zmq.rs reconnects SUB sockets on its own; a receive error means a peer went away.
                warn!("Interrupt socket error: {e}");
                tokio::time::sleep(ERROR_BACKOFF).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: u32, timestamp: u32) -> u64 {
        u64::from(id) | (u64::from(timestamp) << 32)
    }

    #[test]
    fn records_and_reports_interrupts() {
        let state = Arc::new(Mutex::new(InterruptState::default()));
        let interrupt = Interrupt::new(Some(Arc::clone(&state)), key(7, 1_000));
        assert!(!interrupt.is_interrupted());
        assert_eq!(interrupt.check(), Ok(()));

        lock(&state).record(key(8, 1_000), 1_000);
        assert!(!interrupt.is_interrupted());

        lock(&state).record(key(7, 1_000), 1_000);
        assert!(interrupt.is_interrupted());
        let interrupted = interrupt.check().expect_err("interrupted");
        assert_eq!(interrupted.request_id(), 7);
    }

    #[test]
    fn same_id_with_another_timestamp_is_another_request() {
        // IDs wrap, so `prime_server` keys on the ID and the timestamp together.
        let mut state = InterruptState::default();
        state.record(key(7, 1_000), 1_000);
        assert!(!state.contains(key(7, 1_001)));
    }

    #[test]
    fn forgets_interrupts_past_the_cutoff() {
        let mut state = InterruptState::default();
        state.record(key(1, 1_000), 1_000);
        state.record(key(2, 1_300), 1_300);
        // Exactly at the cutoff is kept (`prime_server` drops only strictly older ones).
        state.record(key(3, 1_600), 1_600);
        assert_eq!(state.len(), 3);
        state.record(key(4, 1_601), 1_601);
        assert!(!state.contains(key(1, 1_000)));
        assert!(state.contains(key(2, 1_300)));
        assert_eq!(state.len(), 3);
    }

    #[test]
    fn duplicate_interrupts_are_recorded_once() {
        let mut state = InterruptState::default();
        state.record(key(1, 1_000), 1_000);
        state.record(key(1, 1_000), 1_000);
        assert_eq!(state.len(), 1);
        assert_eq!(state.history.len(), 1);
    }

    #[test]
    fn early_clock_does_not_underflow() {
        let mut state = InterruptState::default();
        state.record(key(1, 0), 10);
        assert!(state.contains(key(1, 0)));
    }

    #[test]
    fn never_is_never_interrupted() {
        assert_eq!(Interrupt::never().check(), Ok(()));
    }
}
