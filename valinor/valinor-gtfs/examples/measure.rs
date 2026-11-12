//! Compiles one feed and reports sizes and timings. Takes a zip, a feed ID, and an output path.
//!
//! ```text
//! cargo run --release -p valinor-gtfs --example measure -- FEED.zip ID OUT.vgtfs YYYYMMDD
//! ```
//!
//! `YYYYMMDD` centers the three-day window: yesterday, that date, and tomorrow.

#![allow(clippy::print_stdout, reason = "reporting tool")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::error::Error;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use valinor_gtfs::{FeedId, FeedInput, ZipSource, compile, parse_service_date};
use valinor_transit::ServiceDate;

/// Measures live heap bytes, so a report is what a program would actually pay for an allocation.
struct CountingAllocator;

static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

// SAFETY: forwards every call to the system allocator, only adding counters.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            bump(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
        ALLOCATED.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }
}

fn bump(size: usize) {
    let now = ALLOCATED.fetch_add(size as u64, Ordering::Relaxed) + size as u64;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn allocated() -> u64 {
    ALLOCATED.load(Ordering::Relaxed)
}

fn peak_reset() -> u64 {
    PEAK.swap(allocated(), Ordering::Relaxed)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(zip), Some(id), Some(out), Some(date)) =
        (args.get(1), args.get(2), args.get(3), args.get(4))
    else {
        return Err("usage: measure FEED.zip ID OUT.vgtfs YYYYMMDD".into());
    };
    let date = parse_service_date(date).ok_or("bad date")?;

    let started = Instant::now();
    peak_reset();
    let output = compile(vec![FeedInput {
        id: FeedId::try_new(id.clone())?,
        source: Box::new(ZipSource::from_path(Path::new(zip))?),
    }])?;
    let compile_time = started.elapsed();
    let compile_peak = peak_reset();
    println!("compile: {compile_time:?} (peak heap {compile_peak} bytes)");
    for report in &output.reports {
        println!("{report:#?}");
    }
    println!("compiled file: {} bytes", output.bytes.len());
    std::fs::write(out, &output.bytes)?;
    drop(output);

    let file = std::fs::read(out)?;
    let before = allocated();
    let started = Instant::now();
    let view = valinor_gtfs::CompiledTimetable::parse(&file)?;
    println!(
        "parse (zero-copy, with checksum and validation): {:?}, allocated {} bytes",
        started.elapsed(),
        allocated() - before,
    );

    let before = allocated();
    let started = Instant::now();
    let timetable = view.to_timetable()?;
    println!(
        "to_timetable: {:?}, retained {} bytes",
        started.elapsed(),
        allocated() - before,
    );

    let days = window_days(date);
    let window = view.service_window(&days)?;
    println!("window: {window:?}");
    let before = allocated();
    peak_reset();
    let started = Instant::now();
    let base = timetable.materialize(&window.days)?;
    println!(
        "materialize 3 days: {:?}, retained {} bytes (peak delta {} bytes)",
        started.elapsed(),
        allocated() - before,
        peak_reset() - before,
    );
    drop(base);

    let count: usize = window
        .days
        .iter()
        .map(|day| view.active_trip_count(day.date))
        .sum();
    println!("active trips in window: {count}");
    for day in &window.days {
        let trips = view.active_trip_count(day.date);
        let events = view.active_stop_event_count(day.date);
        println!("  date {}: {trips} trips, {events} stop events", day.date);
    }
    Ok(())
}

fn window_days(today: ServiceDate) -> Vec<ServiceDate> {
    let days = today.into_inner();
    vec![
        ServiceDate::new(days.saturating_sub(1)),
        ServiceDate::new(days),
        ServiceDate::new(days + 1),
    ]
}
