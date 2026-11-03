//! Startup and per-query timings.
//!
//! `cargo run --release -p airmail_parser --example timing`

use std::time::Instant;

const QUERIES: &[&str] = &[
    "starbucks seattle",
    "123 main st springfield il",
    "coffee near capitol hill",
    "zürich hb",
    "vegan food",
    "seattle",
    "30 w 26 st nyc 10010",
    "West 26th Street, New York, NYC, 10010",
    "Main St & Second Ave",
    "20 Boulevard Saint-Germain, Paris, France",
    "Grolmanstraße 23, Berlin",
    "1900 SE A ST, SAN FRANCISCO",
    "University of Washington Medical Center",
    "Rua Raul Leite Magalhães, 65, Tapiraí - SP, 18180-000, Brazil",
    // long / adversarial inputs
    "Flat 2, 123 North Main Street West, Saint Paul, Ramsey County, Minnesota, 55101, United States of America",
    "new york new york new york new york new york new york new york new york new york",
    "1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30",
    "main & second & third & fourth and fifth and sixth at seventh",
    "paris london berlin rome madrid lisbon vienna prague warsaw dublin oslo",
];

/// Resident set size in KiB (Linux).
fn rss_kib() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4)
}

fn main() {
    let rss_before = rss_kib();
    let start = Instant::now();
    let parser = airmail_parser::AddressParser::new();
    let build = start.elapsed();
    let rss_after = rss_kib();
    println!("parser construction: {build:?}");
    if let (Some(a), Some(b)) = (rss_before, rss_after) {
        println!(
            "resident memory: {:.1} MiB before, {:.1} MiB after",
            a as f64 / 1024.0,
            b as f64 / 1024.0
        );
    }

    let mut total = std::time::Duration::ZERO;
    let mut runs = 0u32;
    for q in QUERIES {
        // warm up
        let _ = parser.parse(q);
        let n = 200;
        let start = Instant::now();
        for _ in 0..n {
            std::hint::black_box(parser.parse(std::hint::black_box(q)));
        }
        let elapsed = start.elapsed();
        total += elapsed;
        runs += n;
        println!(
            "{:>9.1} µs  {q}",
            elapsed.as_secs_f64() * 1e6 / f64::from(n)
        );
    }
    println!(
        "mean: {:.1} µs/query",
        total.as_secs_f64() * 1e6 / f64::from(runs)
    );
}
