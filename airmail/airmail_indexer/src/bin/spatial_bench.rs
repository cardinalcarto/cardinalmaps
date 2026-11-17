//! Searches in boxes of several sizes, for comparing indexes' latency and
//! results: one with coarser coverings against one without, say.
//!
//! ```text
//! spatial_bench sample --index idx --count 100 queries.tsv
//! spatial_bench run --index idx --queries queries.tsv out.tsv
//! spatial_bench compare base.tsv other.tsv
//! ```
//!
//! Queries are `text<TAB>lon<TAB>lat`; each is searched in boxes of each size
//! centered there. `run` writes `text<TAB>lon<TAB>lat<TAB>size_m<TAB>ms<TAB>ids`,
//! with the fastest of `--repeat` runs and the results' IDs comma-separated.
//! `compare` reports, per size, how many of the first file's results the
//! second found, and both files' latencies.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

use airmail::index::{AirmailIndex, SearchHit, SearchRequest};
use airmail::poi::Kind;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Writes queries: the names of POIs sampled from the index, where they are.
    Sample {
        #[clap(long)]
        index: String,
        #[clap(long, default_value_t = 100)]
        count: usize,
        out: PathBuf,
    },
    /// Runs the queries in boxes of each size.
    Run {
        #[clap(long)]
        index: String,
        #[clap(long)]
        queries: PathBuf,
        /// Box widths and heights in meters.
        #[clap(long, value_delimiter = ',', default_value = "300,1000,5000,25000,100000")]
        sizes: Vec<f64>,
        #[clap(long, default_value_t = 10)]
        limit: usize,
        /// Runs of each search; the fastest is reported.
        #[clap(long, default_value_t = 3)]
        repeat: usize,
        out: PathBuf,
    },
    /// Compares two runs' results and latencies, the first as the baseline.
    Compare { base: PathBuf, other: PathBuf },
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::Sample { index, count, out } => sample(&index, count, &out),
        Command::Run { index, queries, sizes, limit, repeat, out } => run(&index, &queries, &sizes, limit, repeat, &out),
        Command::Compare { base, other } => compare(&base, &other),
    }
}

fn sample(index: &str, count: usize, out: &PathBuf) -> Result<()> {
    let index = AirmailIndex::open_read_only(index)?;
    let docs = index.searcher()?.num_docs();
    // Sparse enough to take every POI sampled as is.
    let stride = u32::try_from((docs / (count as u64 * 20).max(1)).max(1)).unwrap_or(u32::MAX);
    let (hits, _) = airmail::tagger_data::sample(&index, stride, 0)?;
    let mut out = BufWriter::new(std::fs::File::create(out)?);
    let mut written = 0;
    for hit in hits.iter().filter(|hit| hit.kind == Kind::Poi) {
        let Some(name) = hit.tag("name").filter(|n| !n.contains(['\t', '\n'])) else { continue };
        writeln!(out, "{name}\t{}\t{}", hit.center.x(), hit.center.y())?;
        written += 1;
        if written == count {
            break;
        }
    }
    eprintln!("{written} queries");
    Ok(())
}

/// A box `size_m` across centered at `[lon, lat]`.
fn bbox(lon: f64, lat: f64, size_m: f64) -> [f64; 4] {
    let half_lat = size_m / 2.0 / 111_195.0;
    let half_lon = half_lat / lat.to_radians().cos().max(0.01);
    [lon - half_lon, (lat - half_lat).max(-90.0), lon + half_lon, (lat + half_lat).min(90.0)]
}

/// What identifies a result across indexes of the same data.
fn id(hit: &SearchHit) -> String {
    if let Some(osm) = &hit.osm {
        format!("osm:{}/{}", osm.osm_type.as_str(), osm.id)
    } else if let Some(wof) = &hit.wof {
        format!("wof:{}", wof.id)
    } else {
        format!("{}@{:.6},{:.6}", hit.kind.as_str(), hit.center.x(), hit.center.y())
    }
}

fn run(index: &str, queries: &PathBuf, sizes: &[f64], limit: usize, repeat: usize, out: &PathBuf) -> Result<()> {
    let index = AirmailIndex::open_read_only(index)?;
    let queries: Vec<(String, f64, f64)> = std::io::BufReader::new(std::fs::File::open(queries)?)
        .lines()
        .map(|line| {
            let line = line?;
            let mut parts = line.split('\t');
            let (Some(text), Some(lon), Some(lat)) = (parts.next(), parts.next(), parts.next()) else {
                bail!("not text<TAB>lon<TAB>lat: {line:?}");
            };
            Ok((text.to_string(), lon.parse()?, lat.parse()?))
        })
        .collect::<Result<_>>()?;
    // The first search opens the index.
    index.search(&SearchRequest { text: "warm up", limit, ..SearchRequest::default() })?;
    let mut out = BufWriter::new(std::fs::File::create(out)?);
    for (text, lon, lat) in &queries {
        for &size in sizes {
            let request = SearchRequest { text, bbox: Some(bbox(*lon, *lat, size)), limit, ..SearchRequest::default() };
            let mut fastest = f64::INFINITY;
            let mut hits = Vec::new();
            for _ in 0..repeat.max(1) {
                let start = Instant::now();
                hits = index.search(&request)?.hits;
                fastest = fastest.min(start.elapsed().as_secs_f64() * 1000.0);
            }
            let ids: Vec<String> = hits.iter().map(id).collect();
            writeln!(out, "{text}\t{lon}\t{lat}\t{size}\t{fastest:.3}\t{}", ids.join(","))?;
        }
    }
    Ok(())
}

/// `(text, lon, lat, size)` → (ms, ids).
type Runs = BTreeMap<(String, String, String, String), (f64, BTreeSet<String>)>;

fn read_run(path: &PathBuf) -> Result<Runs> {
    let mut runs = Runs::new();
    for line in std::io::BufReader::new(std::fs::File::open(path)?).lines() {
        let line = line?;
        let parts: Vec<&str> = line.split('\t').collect();
        let [text, lon, lat, size, ms, ids] = parts[..] else { bail!("not a run line: {line:?}") };
        let ids = ids.split(',').filter(|id| !id.is_empty()).map(str::to_string).collect();
        runs.insert((text.into(), lon.into(), lat.into(), size.into()), (ms.parse().context("ms")?, ids));
    }
    Ok(runs)
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(((values.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f64::NAN)
}

fn compare(base: &PathBuf, other: &PathBuf) -> Result<()> {
    let (base, other) = (read_run(base)?, read_run(other)?);
    // By size: queries, base results, of those found, queries losing any,
    // base and other latencies.
    let mut by_size: BTreeMap<u64, (usize, usize, usize, usize, Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for (key, (base_ms, base_ids)) in &base {
        let Some((other_ms, other_ids)) = other.get(key) else { continue };
        let size: f64 = key.3.parse()?;
        let entry = by_size.entry(size as u64).or_default();
        let found = base_ids.intersection(other_ids).count();
        entry.0 += 1;
        entry.1 += base_ids.len();
        entry.2 += found;
        entry.3 += usize::from(found < base_ids.len());
        entry.4.push(*base_ms);
        entry.5.push(*other_ms);
    }
    println!("size_m\tqueries\trecall\tqueries_losing\tbase_p50_ms\tbase_p95_ms\tother_p50_ms\tother_p95_ms");
    for (size, (queries, wanted, found, losing, mut base_ms, mut other_ms)) in by_size {
        let recall = if wanted == 0 { 1.0 } else { found as f64 / wanted as f64 };
        println!(
            "{size}\t{queries}\t{recall:.4}\t{losing}\t{:.2}\t{:.2}\t{:.2}\t{:.2}",
            percentile(&mut base_ms, 0.5),
            percentile(&mut base_ms, 0.95),
            percentile(&mut other_ms, 0.5),
            percentile(&mut other_ms, 0.95)
        );
    }
    Ok(())
}
