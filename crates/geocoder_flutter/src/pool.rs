//! Building documents on several threads. Making a document's text (its names' and roads'
//! forms) is most of a build's work; the documents still go to the writer in the order
//! their features came, so a build's index doesn't depend on the threads.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, channel, sync_channel};
use std::thread::JoinHandle;

use airmail::index::{AirmailIndexWriter, Document, DocumentBuilder, FormsCache};
use airmail::poi::ToIndexPoi;

/// Documents sent to a thread at once: enough that sending costs little next to building
/// them, few enough that neighbours (which share streets and areas) go to one thread.
const BATCH: usize = 256;

/// Batches waiting for each thread, beyond the one it's building.
const QUEUED: usize = 2;

type Built = anyhow::Result<Vec<Document>>;

/// Threads building documents, in batches, and the writer they're added to in order.
pub struct DocumentPool {
    senders: Vec<SyncSender<(u64, Vec<ToIndexPoi>)>>,
    /// Behind a lock only so the pool is `Sync`, as Dart's handle on a build must be.
    results: Mutex<Receiver<(u64, Built)>>,
    threads: Vec<JoinHandle<()>>,
    batch: Vec<ToIndexPoi>,
    /// The number of the next batch to send, and of the next to add.
    sent: u64,
    added: u64,
    /// Batches built ahead of one still being built.
    ready: BTreeMap<u64, Built>,
}

impl DocumentPool {
    /// `threads` threads (at least one) building with `builder`, for `source`.
    #[must_use]
    pub fn new(builder: &DocumentBuilder, source: &'static str, threads: usize) -> Self {
        let (done, results) = channel();
        let (senders, threads) = (0..threads.max(1))
            .map(|i| {
                let (send, receive) = sync_channel(QUEUED);
                let builder = builder.clone();
                let done = done.clone();
                let thread = std::thread::Builder::new()
                    .name(format!("geocoder-docs-{i}"))
                    .spawn(move || build_batches(&builder, source, &receive, &done))
                    .expect("starting a document thread");
                (send, thread)
            })
            .unzip();
        Self {
            senders,
            results: Mutex::new(results),
            threads,
            batch: Vec::with_capacity(BATCH),
            sent: 0,
            added: 0,
            ready: BTreeMap::new(),
        }
    }

    /// Builds the document of `poi` and adds it to `writer`, sooner or later (by
    /// [`DocumentPool::finish`]). Adds what's been built meanwhile.
    ///
    /// # Errors
    ///
    /// If a document built meanwhile couldn't be built or added.
    pub fn add(&mut self, poi: ToIndexPoi, writer: &AirmailIndexWriter) -> anyhow::Result<()> {
        self.batch.push(poi);
        if self.batch.len() >= BATCH {
            self.send()?;
        }
        self.add_ready(writer, false)
    }

    fn send(&mut self) -> anyhow::Result<()> {
        let batch = std::mem::replace(&mut self.batch, Vec::with_capacity(BATCH));
        let thread = (self.sent % self.senders.len() as u64) as usize;
        self.senders[thread]
            .send((self.sent, batch))
            .map_err(|_| anyhow::anyhow!("a document thread stopped"))?;
        self.sent += 1;
        Ok(())
    }

    /// Adds the built batches that are next, waiting for them if `wait` until every batch
    /// sent is added.
    fn add_ready(&mut self, writer: &AirmailIndexWriter, wait: bool) -> anyhow::Result<()> {
        loop {
            while let Some(built) = self.ready.remove(&self.added) {
                self.added += 1;
                for doc in built? {
                    writer.add_document(doc)?;
                }
            }
            if self.added == self.sent {
                return Ok(());
            }
            let results = self
                .results
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (n, built) = if wait {
                results
                    .recv()
                    .map_err(|_| anyhow::anyhow!("a document thread stopped"))?
            } else {
                match results.try_recv() {
                    Ok(result) => result,
                    Err(TryRecvError::Empty) => return Ok(()),
                    Err(TryRecvError::Disconnected) => {
                        anyhow::bail!("a document thread stopped")
                    }
                }
            };
            self.ready.insert(n, built);
        }
    }

    /// Adds every document still to come, and stops the threads.
    ///
    /// # Errors
    ///
    /// If a document couldn't be built or added.
    pub fn finish(mut self, writer: &AirmailIndexWriter) -> anyhow::Result<()> {
        if !self.batch.is_empty() {
            self.send()?;
        }
        let result = self.add_ready(writer, true);
        self.senders.clear();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        result
    }
}

fn build_batches(
    builder: &DocumentBuilder,
    source: &str,
    batches: &Receiver<(u64, Vec<ToIndexPoi>)>,
    done: &Sender<(u64, Built)>,
) {
    // Each thread's own: its batches are neighbours, and share streets and areas.
    let mut forms = FormsCache::default();
    while let Ok((n, batch)) = batches.recv() {
        let built = batch
            .into_iter()
            .map(|poi| builder.document_cached(poi, source, &mut forms))
            .collect();
        if done.send((n, built)).is_err() {
            return;
        }
    }
}
