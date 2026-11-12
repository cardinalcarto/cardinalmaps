//! How much a small tile file can make the decoder allocate.
#![allow(clippy::all, clippy::pedantic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use valinor_walk::transfer_tile::{DecodeLimits, decode_tile};

struct Peak;
static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Tests mustn't overlap, since the allocator counts every thread.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

unsafe impl GlobalAlloc for Peak {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        let n = NOW.fetch_add(l.size(), Ordering::Relaxed) + l.size();
        PEAK.fetch_max(n, Ordering::Relaxed);
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        NOW.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        NOW.fetch_sub(l.size(), Ordering::Relaxed);
        let n = NOW.fetch_add(new, Ordering::Relaxed) + new;
        PEAK.fetch_max(n, Ordering::Relaxed);
        q
    }
}

#[global_allocator]
static ALLOCATOR: Peak = Peak;

const MIB: usize = 1 << 20;

fn header(origins: u32, walks: u32) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend_from_slice(b"VTFR");
    h.extend_from_slice(&1u16.to_le_bytes());
    h.push(0); // walking
    h.push(valinor_walk::transfer_tile::TILE_ZOOM); // zoom
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&0u64.to_le_bytes());
    h.extend_from_slice(&900u32.to_le_bytes());
    h.extend_from_slice(&origins.to_le_bytes());
    h.extend_from_slice(&walks.to_le_bytes());
    h.extend_from_slice(&[0; 4]);
    assert_eq!(h.len(), 40);
    h
}

fn seal(mut raw: Vec<u8>) -> Vec<u8> {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&raw);
    let crc = hasher.finalize();
    raw[36..40].copy_from_slice(&crc.to_le_bytes());
    zstd::bulk::compress(&raw, 3).expect("compress")
}

fn varint(mut v: u32, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// One origin whose row claims every pair: the body is all zeros, one byte a value.
fn walks_bomb(limit: usize) -> Vec<u8> {
    let pairs = (limit - 64) / 2;
    let mut raw = header(1, pairs as u32);
    raw.extend_from_slice(&[0, 0]); // the origin
    varint(pairs as u32, &mut raw); // its walk count
    raw.resize(raw.len() + pairs * 2, 0); // keyed pairs; there is no seconds column
    seal(raw)
}

/// Many origins, 3 bytes each.
fn origins_bomb(limit: usize) -> Vec<u8> {
    let origins = (limit - 64) / 3;
    let mut raw = header(origins as u32, 0);
    raw.resize(raw.len() + origins * 3, 0);
    seal(raw)
}

fn measure(name: &str, frame: &[u8], limit: usize) -> usize {
    let before = NOW.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let started = std::time::Instant::now();
    let outcome = decode_tile(
        frame,
        DecodeLimits {
            max_decompressed_bytes: limit,
        },
    );
    let peak = PEAK.load(Ordering::Relaxed) - before;
    eprintln!(
        "{name}: frame {} bytes, limit {} MiB, peak {} MiB ({:.1}x the limit), {:.1} s, outcome {:?}",
        frame.len(),
        limit / MIB,
        peak / MIB,
        peak as f64 / limit as f64,
        started.elapsed().as_secs_f64(),
        outcome.as_ref().err()
    );
    peak
}

/// `DecodeLimits` is the promised bound, for any bytes.
#[test]
fn a_small_file_stays_within_a_small_multiple_of_the_limit() {
    let _serial = serial();
    let limit = 64 * MIB;
    let walks = measure("walks bomb", &walks_bomb(limit), limit);
    let origins = measure("origins bomb", &origins_bomb(limit), limit);
    assert!(
        walks < 3 * limit && origins < 3 * limit,
        "walks {walks}, origins {origins}"
    );
}

/// The default limit, as a client that did not pass one gets.
#[test]
fn the_default_limit_holds_too() {
    let _serial = serial();
    let limit = valinor_walk::transfer_tile::DEFAULT_MAX_DECOMPRESSED_BYTES;
    let peak = measure("walks bomb, default limit", &walks_bomb(limit), limit);
    assert!(peak < 3 * limit, "peak {peak}");
}

/// A frame the zstd CLI or a library default might produce, with a content checksum.
#[test]
fn a_checksummed_frame_decodes() {
    let _serial = serial();
    let raw = {
        let mut r = header(0, 0);
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&r);
        let crc = hasher.finalize();
        r[36..40].copy_from_slice(&crc.to_le_bytes());
        r
    };
    let mut compressor = zstd::bulk::Compressor::new(3).expect("compressor");
    compressor.include_checksum(true).expect("flag");
    let frame = compressor.compress(&raw).expect("compress");
    let outcome = decode_tile(&frame, DecodeLimits::default());
    eprintln!("checksummed frame: {:?}", outcome.as_ref().err());
    assert!(outcome.is_ok());
}

/// A header-only frame asking for the largest window the decoder allows.
#[test]
fn a_large_window_is_not_allocated_up_front() {
    let _serial = serial();
    // zstd magic, window descriptor for 2^26 (64 MiB), then a last raw block of zero bytes.
    let frame = [
        0x28,
        0xB5,
        0x2F,
        0xFD,
        0x00,
        (26 - 10) << 3,
        0x01,
        0x00,
        0x00,
    ];
    let peak = measure("window frame", &frame, 256 * MIB);
    assert!(peak < 8 * MIB, "peak {} MiB", peak / MIB);
}
