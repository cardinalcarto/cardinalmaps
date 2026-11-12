//! `cardinal-pack transfers`: packs one mode of a transit build's transfer tiles into a
//! PMTiles archive, for `cardinal-publish transit` to upload as one object instead of one
//! object per tile.
//!
//! ```text
//! cardinal-pack transfers --build BUILD --mode MODE --out OUT.pmtiles [--report REPORT.json]
//! ```
//!
//! Tiles stay as the build wrote them (`transfers/MODE/12/X/Y.bin.zst`, a zstd frame each),
//! at zoom 12, in tile ID (Hilbert) order; the header says tile type unknown and
//! compression none, and the metadata says what the tiles are. Every tile the manifest
//! lists for the mode must be there at the size it gives. The archive is written beside
//! `OUT` and renamed into place once whole.

use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use offline_map::Error;
use offline_map::pmtiles::{Compression, tile_id};
use offline_map::write::{ArchiveSpec, TileSize, write_archive};
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    manifest_id: String,
    valhalla_build_id: u32,
    tile_format_version: u16,
    zoom: u8,
    tiles: std::collections::BTreeMap<String, Vec<TileEntry>>,
}

#[derive(Deserialize)]
struct TileEntry {
    x: u32,
    y: u32,
    bytes: u64,
}

struct Args {
    build: PathBuf,
    mode: String,
    out: PathBuf,
    report: Option<PathBuf>,
}

fn usage() -> String {
    "usage: cardinal-pack transfers --build BUILD --mode MODE --out OUT.pmtiles [--report REPORT.json]"
        .to_owned()
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    if args.next().as_deref() != Some("transfers") {
        return Err(usage());
    }
    let (mut build, mut mode, mut out, mut report) = (None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(usage)?;
        match flag.as_str() {
            "--build" => build = Some(PathBuf::from(value)),
            "--mode" => mode = Some(value),
            "--out" => out = Some(PathBuf::from(value)),
            "--report" => report = Some(PathBuf::from(value)),
            _ => return Err(usage()),
        }
    }
    Ok(Args {
        build: build.ok_or_else(usage)?,
        mode: mode.ok_or_else(usage)?,
        out: out.ok_or_else(usage)?,
        report,
    })
}

fn pack(args: &Args) -> Result<String, String> {
    let started = Instant::now();
    let manifest_path = args.build.join("transfers/manifest.json");
    let manifest: Manifest = serde_json::from_reader(std::io::BufReader::new(
        File::open(&manifest_path).map_err(|e| format!("{}: {e}", manifest_path.display()))?,
    ))
    .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let entries = manifest
        .tiles
        .get(&args.mode)
        .ok_or_else(|| format!("the build has no {} transfer tiles", args.mode))?;
    let zoom = manifest.zoom;
    let mut tiles: Vec<(u64, &TileEntry)> = entries
        .iter()
        .map(|t| (tile_id(zoom, t.x, t.y), t))
        .collect();
    tiles.sort_unstable_by_key(|&(id, _)| id);
    if tiles.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(format!("the manifest lists a {} tile twice", args.mode));
    }
    let path = |t: &TileEntry| -> PathBuf {
        args.build.join(format!(
            "transfers/{}/{zoom}/{}/{}.bin.zst",
            args.mode, t.x, t.y
        ))
    };
    let mut sizes = Vec::with_capacity(tiles.len());
    for &(id, t) in &tiles {
        let p = path(t);
        let found = std::fs::metadata(&p)
            .map_err(|e| format!("{}: {e}", p.display()))?
            .len();
        if found != t.bytes {
            return Err(format!(
                "{} is {found} bytes; the manifest says {}",
                p.display(),
                t.bytes
            ));
        }
        sizes.push(TileSize {
            tile_id: id,
            length: u32::try_from(found).map_err(|_| format!("{} is too large", p.display()))?,
        });
    }
    let metadata = serde_json::json!({
        "name": "Valinor transfer tiles",
        "format": "zst",
        "description": format!(
            "Each tile is a zstd frame holding one valinor-walk transfer tile (format version \
             {}, magic VTFR) of {} walks between stops, as the transit build wrote it to \
             transfers/{}/{zoom}/X/Y.bin.zst.",
            manifest.tile_format_version, args.mode, args.mode
        ),
        "mode": args.mode,
        "zoom": zoom,
        "manifest_id": manifest.manifest_id,
        "valhalla_build_id": manifest.valhalla_build_id,
        "tile_format_version": manifest.tile_format_version,
    });
    let spec = ArchiveSpec {
        tile_type: 0,
        tile_compression: Compression::None,
        min_zoom: zoom,
        max_zoom: zoom,
        bounds: [-180.0, -85.051_128_8, 180.0, 85.051_128_8],
        metadata: metadata.to_string(),
    };
    let partial = partial_path(&args.out);
    let file = File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let mut buffer = Vec::new();
    let written = write_archive(&mut out, &spec, &sizes, |i, sink| {
        buffer.clear();
        File::open(path(tiles[i].1))?.read_to_end(&mut buffer)?;
        sink.write_all(&buffer)?;
        Ok::<(), Error>(())
    })
    .map_err(|e| e.to_string())?;
    let file = out.into_inner().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &args.out).map_err(|e| e.to_string())?;
    let tile_bytes: u64 = sizes.iter().map(|s| u64::from(s.length)).sum();
    Ok(serde_json::json!({
        "mode": args.mode,
        "manifest_id": manifest.manifest_id,
        "tiles": written.tiles,
        "tile_bytes": tile_bytes,
        "archive_bytes": written.bytes,
        "leaf_directories": written.leaf_directories,
        "seconds": started.elapsed().as_secs_f64(),
    })
    .to_string())
}

fn partial_path(out: &Path) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    out.with_file_name(name)
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(64);
        }
    };
    match pack(&args) {
        Ok(report) => {
            if let Some(path) = &args.report
                && let Err(e) = std::fs::write(path, &report)
            {
                eprintln!("{}: {e}", path.display());
                return ExitCode::FAILURE;
            }
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("cardinal-pack: {message}");
            ExitCode::FAILURE
        }
    }
}
