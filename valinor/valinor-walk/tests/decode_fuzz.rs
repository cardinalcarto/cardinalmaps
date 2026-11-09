//! Mutated real tiles and frames through the whole decoder.
//!
//! Needs a transfer stage's output under `/mnt/maps/cardinal`; run with `-- --ignored`.
#![allow(clippy::all, clippy::pedantic)]

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use valinor_walk::transfer_tile::{DecodeLimits, decode_tile};

fn corpus() -> Vec<Vec<u8>> {
    let root = PathBuf::from("/mnt/maps/cardinal/transit/global/us-wa-2026-11-09/transfers");
    // Written by the transfer stage; a rebuild replaces this directory.
    let mut out = Vec::new();
    for mode in ["walking", "cycling"] {
        for x in std::fs::read_dir(root.join(mode).join("12"))
            .expect("dir")
            .flatten()
        {
            for y in std::fs::read_dir(x.path()).expect("dir").flatten() {
                let bytes = std::fs::read(y.path()).expect("read");
                if bytes.len() < 40_000 {
                    out.push(bytes);
                }
            }
        }
    }
    let mut more = Vec::new();
    for tile in out.iter().step_by(11).take(40) {
        let raw = zstd::bulk::decompress(tile, 1 << 26).expect("raw");
        for level in [1, 3, 9, -3] {
            more.push(zstd::bulk::compress(&raw, level).expect("compress"));
        }
        let mut with_check = zstd::bulk::Compressor::new(5).expect("c");
        with_check.include_checksum(true).expect("flag");
        more.push(with_check.compress(&raw).expect("compress"));
    }
    out.extend(more);
    out
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(rng: &mut Rng, corpus: &[Vec<u8>], out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(&corpus[rng.below(corpus.len())]);
    for _ in 0..1 + rng.below(6) {
        let len = out.len().max(1);
        // Headers, block headers, and table descriptions matter most: favor the front.
        let at = if rng.below(2) == 0 {
            rng.below(len.min(64))
        } else {
            rng.below(len)
        };
        match rng.below(7) {
            0 => out[at] ^= 1 << rng.below(8),
            1 => out[at] = rng.next() as u8,
            2 => out[at] = [0, 1, 0x7f, 0x80, 0xff][rng.below(5)],
            3 => {
                let end = (at + 1 + rng.below(16)).min(len);
                out.drain(at..end);
                if out.is_empty() {
                    out.push(0);
                }
            }
            4 => {
                let bytes: Vec<u8> = (0..1 + rng.below(16)).map(|_| rng.next() as u8).collect();
                let at = at.min(out.len());
                out.splice(at..at, bytes);
            }
            5 => {
                let other = &corpus[rng.below(corpus.len())];
                if other.len() > 8 {
                    let from = rng.below(other.len() - 4);
                    let n = (1 + rng.below(32)).min(other.len() - from);
                    let n = n.min(out.len() - at);
                    out[at..at + n].copy_from_slice(&other[from..from + n]);
                }
            }
            _ => out.truncate(at.max(1)),
        }
    }
}

#[test]
#[ignore = "needs /mnt/maps/cardinal data"]
fn mutated_frames_never_panic() {
    let seconds: u64 = std::env::var("FUZZ_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let threads: u64 = std::env::var("FUZZ_THREADS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let corpus = corpus();
    eprintln!("corpus: {} frames", corpus.len());
    panic::set_hook(Box::new(|_| {}));
    let total = AtomicU64::new(0);
    let accepted = AtomicU64::new(0);
    let panics: Mutex<Vec<(Vec<u8>, String)>> = Mutex::new(Vec::new());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    std::thread::scope(|scope| {
        for t in 0..threads {
            let (corpus, total, accepted, panics) = (&corpus, &total, &accepted, &panics);
            scope.spawn(move || {
                let mut rng =
                    Rng(0x9E37_79B9_7F4A_7C15 ^ (t + 1).wrapping_mul(0xD1B5_4A32_D192_ED03));
                let mut frame = Vec::new();
                let limits = DecodeLimits {
                    max_decompressed_bytes: 16 << 20,
                };
                while std::time::Instant::now() < deadline {
                    for _ in 0..256 {
                        mutate(&mut rng, corpus, &mut frame);
                        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
                            decode_tile(&frame, limits).is_ok()
                        }));
                        total.fetch_add(1, Ordering::Relaxed);
                        match outcome {
                            Ok(true) => {
                                accepted.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(false) => {}
                            Err(payload) => {
                                let text = payload
                                    .downcast_ref::<&str>()
                                    .map(|s| s.to_string())
                                    .or_else(|| payload.downcast_ref::<String>().cloned())
                                    .unwrap_or_default();
                                let mut found = panics.lock().unwrap();
                                if found.len() < 50 {
                                    found.push((frame.clone(), text));
                                }
                            }
                        }
                    }
                }
            });
        }
    });
    let _ = panic::take_hook();
    let found = panics.lock().unwrap();
    eprintln!(
        "tried {}, decoded {}, panics {}",
        total.load(Ordering::Relaxed),
        accepted.load(Ordering::Relaxed),
        found.len()
    );
    let mut messages: Vec<&str> = found.iter().map(|(_, m)| m.as_str()).collect();
    messages.sort_unstable();
    messages.dedup();
    for m in &messages {
        eprintln!("panic: {m}");
    }
    assert!(found.is_empty());
}
