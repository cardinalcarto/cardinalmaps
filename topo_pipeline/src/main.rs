//! Cardinal Topo's data pipeline: DEMs to contour lines to an OSM file for Planetiler.

mod assemble;
mod chunks;
mod config;
mod contour;
mod dem;
mod pbf;
mod seams;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "DEMs to contour lines to an OSM file for Planetiler")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generates contours as a sorted .osm.pbf with both metric and imperial lines.
    Contours {
        #[arg(long)]
        config: PathBuf,
        /// Holds a cache of finished chunks; reruns only compute what changed.
        #[arg(long)]
        work: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// west,south,east,north
        #[arg(long)]
        bbox: Option<String>,
        /// Above OSM's largest node ID (about 1.4e10 in 2026), leaving it room to grow.
        #[arg(long, default_value_t = 20_000_000_000)]
        node_id_start: i64,
        /// Above OSM's largest way ID (about 1.5e9 in 2026), leaving it room to grow.
        #[arg(long, default_value_t = 4_000_000_000)]
        way_id_start: i64,
    },
    /// Writes a copy of a planet or extract with the contours spliced in.
    Append {
        #[arg(long)]
        planet: PathBuf,
        #[arg(long)]
        contours: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Checks that lines meet across chunk boundaries in the chunk cache.
    Seams {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        work: PathBuf,
        #[arg(long)]
        bbox: Option<String>,
    },
}

fn parse_bbox(arg: Option<&str>) -> Result<Option<(f64, f64, f64, f64)>> {
    let Some(arg) = arg else { return Ok(None) };
    let parts = arg
        .split(',')
        .map(|p| p.trim().parse::<f64>().context("--bbox needs west,south,east,north"))
        .collect::<Result<Vec<_>>>()?;
    let [w, s, e, n] = parts[..] else {
        bail!("--bbox needs west,south,east,north");
    };
    Ok(Some((w, s, e, n)))
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Contours { config, work, out, bbox, node_id_start, way_id_start } => {
            let config = config::Config::load(&config)?;
            let mosaic = chunks::Mosaic::open(&config)?;
            for source in &mosaic.sources {
                eprintln!("source {}: {} rasters", source.config.name, source.rasters.len());
            }
            let ids = mosaic.chunks(parse_bbox(bbox.as_deref())?);
            let files = mosaic.ensure(&work, &ids)?;
            let totals = assemble::write_contours(&files, node_id_start, way_id_start, &out)?;
            eprintln!(
                "wrote {}: {} lines, {} nodes, {} ways, {:.1} MB",
                out.display(),
                totals.lines,
                totals.nodes,
                totals.ways,
                std::fs::metadata(&out)?.len() as f64 / 1e6
            );
        }
        Command::Append { planet, contours, out } => {
            assemble::append(&planet, &contours, &out)?;
            eprintln!("wrote {}", out.display());
        }
        Command::Seams { config, work, bbox } => {
            let config = config::Config::load(&config)?;
            let mosaic = chunks::Mosaic::open(&config)?;
            let ids = mosaic.chunks(parse_bbox(bbox.as_deref())?);
            let files = mosaic.ensure(&work, &ids)?;
            let report = seams::check(&files)?;
            println!("{report:#?}");
            if report.unmatched > 0 {
                bail!("{} boundary endpoints have no partner", report.unmatched);
            }
        }
    }
    Ok(())
}
