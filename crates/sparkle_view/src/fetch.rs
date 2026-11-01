//! Fetching tiles from tile servers on their own threads, so that the tile workers never
//! wait on the network. A tile is only fetched while the view still wants it; one that
//! fails is tried again later, backing off, for as long as it's wanted.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::loader::SourceTile;
use crate::remote::{Fetched, TileServer};

/// Requests in flight at once, across servers.
const THREADS: usize = 8;
/// The wait before the first retry, doubling with each failure up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub struct FetchJob {
    pub tile: SourceTile,
    /// The zoom to process it at, as requested.
    pub zoom: u8,
    pub server: Arc<TileServer>,
    /// Revalidating a cached tile that's drawn already, rather than fetching a missing
    /// one. Not retried: the cached tile stands until the next check.
    pub refresh: bool,
    pub etag: Option<String>,
    failures: u32,
}

impl FetchJob {
    pub fn new(tile: SourceTile, zoom: u8, server: Arc<TileServer>) -> Self {
        Self {
            tile,
            zoom,
            server,
            refresh: false,
            etag: None,
            failures: 0,
        }
    }

    pub fn refresh(
        tile: SourceTile,
        zoom: u8,
        server: Arc<TileServer>,
        etag: Option<String>,
    ) -> Self {
        Self {
            refresh: true,
            etag,
            ..Self::new(tile, zoom, server)
        }
    }
}

/// Jobs waiting out their backoff, soonest first.
#[derive(Default)]
struct Retries {
    queue: Mutex<(BinaryHeap<Reverse<Delayed>>, bool)>,
    changed: Condvar,
}

struct Delayed(Instant, FetchJob);

impl PartialEq for Delayed {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Delayed {}
impl PartialOrd for Delayed {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Delayed {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

pub struct Fetcher {
    jobs: Sender<FetchJob>,
    retries: Arc<Retries>,
}

impl Fetcher {
    /// Start fetching. `done` gets each job's answer, or `None` for a job dropped because
    /// the view no longer wants its tile.
    pub fn spawn(
        wanted: Arc<Mutex<HashSet<SourceTile>>>,
        done: impl Fn(FetchJob, Option<Fetched>) + Send + Sync + 'static,
    ) -> Result<Self> {
        let (jobs, queue) = mpsc::channel::<FetchJob>();
        let queue = Arc::new(Mutex::new(queue));
        let retries = Arc::new(Retries::default());
        let done = Arc::new(done);
        for i in 0..THREADS {
            let (queue, retries, wanted, done) =
                (queue.clone(), retries.clone(), wanted.clone(), done.clone());
            thread::Builder::new()
                .name(format!("tile-fetch-{i}"))
                .spawn(move || {
                    while let Some(mut job) = next(&queue) {
                        let (_, id) = job.tile;
                        if !wanted.lock().unwrap().contains(&job.tile) {
                            if job.refresh {
                                job.server.end_revalidating(id);
                            }
                            done(job, None);
                            continue;
                        }
                        match job.server.fetch(id, job.etag.as_deref()) {
                            Ok(fetched) => {
                                if job.refresh {
                                    job.server.end_revalidating(id);
                                }
                                done(job, Some(fetched));
                            }
                            Err(err) if job.refresh => {
                                log::debug!("revalidating: {err:#}");
                                job.server.end_revalidating(id);
                            }
                            Err(err) => {
                                log::debug!("{err:#}");
                                let backoff = FIRST_BACKOFF
                                    .saturating_mul(1 << job.failures.min(16))
                                    .min(MAX_BACKOFF);
                                job.failures += 1;
                                retries.push(Instant::now() + backoff, job);
                            }
                        }
                    }
                })?;
        }
        let (sender, timer) = (jobs.clone(), retries.clone());
        thread::Builder::new()
            .name("tile-fetch-retry".into())
            .spawn(move || {
                while let Some(job) = timer.pop() {
                    if sender.send(job).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { jobs, retries })
    }

    pub fn fetch(&self, job: FetchJob) {
        // The threads only stop once this is dropped.
        let _ = self.jobs.send(job);
    }
}

impl Drop for Fetcher {
    fn drop(&mut self) {
        // Stop the retry thread, whose sender would otherwise keep the fetch threads.
        self.retries.queue.lock().unwrap().1 = true;
        self.retries.changed.notify_all();
    }
}

impl Retries {
    fn push(&self, at: Instant, job: FetchJob) {
        let mut queue = self.queue.lock().unwrap();
        queue.0.push(Reverse(Delayed(at, job)));
        self.changed.notify_all();
    }

    /// The next job once its wait is over, or `None` once the fetcher is gone.
    fn pop(&self) -> Option<FetchJob> {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if queue.1 {
                return None;
            }
            let now = Instant::now();
            match queue.0.peek() {
                Some(Reverse(Delayed(at, ..))) if *at <= now => {
                    return queue.0.pop().map(|Reverse(Delayed(_, job))| job);
                }
                Some(Reverse(Delayed(at, ..))) => {
                    let wait = *at - now;
                    queue = self.changed.wait_timeout(queue, wait).unwrap().0;
                }
                None => queue = self.changed.wait(queue).unwrap(),
            }
        }
    }
}

fn next(queue: &Mutex<Receiver<FetchJob>>) -> Option<FetchJob> {
    queue.lock().unwrap().recv().ok()
}
