//! Fetching tiles from tile servers on their own threads, so that the tile workers never
//! wait on the network. A tile is only fetched while the view still wants it, the tiles it
//! wants most first; one that fails is tried again later, backing off, for as long as it's
//! wanted.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::loader::{SourceTile, Wanted, rank};
use crate::queue::{Queue, lowest};
use crate::remote::{Fetched, TileServer};

/// Requests in flight at once, across servers.
const THREADS: usize = 16;
/// Threads that may fetch tiles wanted for later at once: the rest are kept for tiles on
/// screen, so one the view needs now never waits for a request already under way to finish.
const MAX_PREDICTING: usize = 8;
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
    jobs: Arc<Queue<FetchJob>>,
    retries: Arc<Retries>,
}

impl Fetcher {
    /// Start fetching. `done` gets each job's answer, or `None` for a job dropped because
    /// the view no longer wants its tile.
    pub fn spawn(
        wanted: Wanted,
        done: impl Fn(FetchJob, Option<Fetched>) + Send + Sync + 'static,
    ) -> Result<Self> {
        let jobs = Arc::new(Queue::default());
        let retries = Arc::new(Retries::default());
        let done = Arc::new(done);
        // Threads fetching tiles wanted for later.
        let predicting = Arc::new(AtomicUsize::new(0));
        for i in 0..THREADS {
            let (queue, retries, wanted, done, predicting) = (
                jobs.clone(),
                retries.clone(),
                wanted.clone(),
                done.clone(),
                predicting.clone(),
            );
            thread::Builder::new()
                .name(format!("tile-fetch-{i}"))
                .spawn(move || {
                    while let Some((mut job, predicted)) =
                        next(&queue, |j: &FetchJob| j.tile, &wanted, &predicting)
                    {
                        let _predicting = predicted.then(|| Predicting(&predicting, &queue));
                        let (_, id) = job.tile;
                        if !wanted.lock().unwrap().contains_key(&job.tile) {
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
        let (queue, timer) = (jobs.clone(), retries.clone());
        thread::Builder::new()
            .name("tile-fetch-retry".into())
            .spawn(move || {
                while let Some(job) = timer.pop() {
                    queue.push(job);
                }
            })?;
        Ok(Self { jobs, retries })
    }

    pub fn fetch(&self, job: FetchJob) {
        self.jobs.push(job);
    }

    /// The view's wanted tiles changed: a job left waiting for a free thread may be
    /// wanted on screen now.
    pub fn wake(&self) {
        self.jobs.wake();
    }
}

impl Drop for Fetcher {
    fn drop(&mut self) {
        self.jobs.close();
        self.retries.queue.lock().unwrap().1 = true;
        self.retries.changed.notify_all();
    }
}

/// Held while a thread fetches a tile wanted for later; letting go frees its place for
/// another such tile.
struct Predicting<'a>(&'a AtomicUsize, &'a Queue<FetchJob>);

impl Drop for Predicting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
        self.1.wake();
    }
}

/// The job for the tile the view wants most, or one it no longer wants, to drop; and
/// whether it's wanted for later, which only so many threads take at once (`predicting`
/// counts them).
fn next<J>(
    queue: &Queue<J>,
    tile: impl Fn(&J) -> SourceTile,
    wanted: &Wanted,
    predicting: &AtomicUsize,
) -> Option<(J, bool)> {
    let mut predicted = false;
    let job = queue.pop(|jobs| {
        let wanted = wanted.lock().unwrap();
        // Only raised under the queue's lock, held here, so no two threads take the last
        // place.
        let busy = predicting.load(Ordering::Relaxed) >= MAX_PREDICTING;
        let i = lowest(jobs, |job| match wanted.get(&tile(job)) {
            Some(w) if w.tier.predicted() && busy => None,
            _ => Some(rank(&wanted, &tile(job))),
        })?;
        predicted = wanted
            .get(&tile(&jobs[i]))
            .is_some_and(|w| w.tier.predicted());
        if predicted {
            predicting.fetch_add(1, Ordering::Relaxed);
        }
        Some(i)
    })?;
    Some((job, predicted))
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use sparkle::TileId;

    use super::*;
    use crate::loader::{Tier, Want};

    fn tile(x: u32) -> SourceTile {
        (0, TileId::new(10, x, 0))
    }

    /// A queue of tiles `0..n`, wanted as `tiers` says, in that order.
    fn setup(n: u32, tiers: &[(u32, Tier)]) -> (Queue<SourceTile>, Wanted) {
        let queue = Queue::default();
        (0..n).for_each(|x| queue.push(tile(x)));
        let wanted: HashMap<_, _> = tiers
            .iter()
            .enumerate()
            .map(|(rank, &(x, tier))| {
                let want = Want {
                    rank: rank as u32,
                    tier,
                };
                (tile(x), want)
            })
            .collect();
        (queue, Arc::new(Mutex::new(wanted)))
    }

    #[test]
    fn takes_unwanted_tiles_first_to_drop_then_in_the_views_order() {
        let tiers = [
            (2, Tier::Visible),
            (1, Tier::Visible),
            (3, Tier::Fallback),
            (0, Tier::Ring),
        ];
        let (queue, wanted) = setup(5, &tiers);
        let predicting = AtomicUsize::new(0);
        let order: Vec<(u32, bool)> = (0..5)
            .map(|_| {
                let (t, predicted) = next(&queue, |&t| t, &wanted, &predicting).unwrap();
                (t.1.x, predicted)
            })
            .collect();
        assert_eq!(
            order,
            [(4, false), (2, false), (1, false), (3, true), (0, true)]
        );
    }

    #[test]
    fn tiles_for_later_leave_threads_for_visible_ones() {
        let mut tiers: Vec<_> = (0..20).map(|x| (x, Tier::Destination)).collect();
        tiers.push((20, Tier::Visible));
        let (queue, wanted) = setup(20, &tiers);
        let predicting = AtomicUsize::new(0);
        for x in 0..MAX_PREDICTING as u32 {
            let (t, predicted) = next(&queue, |&t| t, &wanted, &predicting).unwrap();
            assert_eq!((t.1.x, predicted), (x, true));
        }
        // The rest wait while those are fetched, but a visible tile is taken at once.
        queue.push(tile(20));
        let (t, predicted) = next(&queue, |&t| t, &wanted, &predicting).unwrap();
        assert_eq!((t.1.x, predicted), (20, false));
        // Once one is done, the next tile for later goes.
        predicting.fetch_sub(1, Ordering::Relaxed);
        let (t, _) = next(&queue, |&t| t, &wanted, &predicting).unwrap();
        assert_eq!(t.1.x, MAX_PREDICTING as u32);
    }
}
