//! Checks that contours meet across chunk boundaries.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use rayon::prelude::*;

use crate::chunks::{self, ChunkId};

#[derive(Debug, Default)]
pub struct Report {
    pub endpoints: u64,
    pub on_boundary: u64,
    pub matched: u64,
    /// Boundary endpoints with a partner within a few units; expected only where chunks of
    /// different resolutions meet, with rounding in the interpolated samples.
    pub matched_near: u64,
    /// Boundary endpoints with no partner in the neighboring chunk.
    pub unmatched: u64,
    pub outer: u64,
    /// Ends away from any chunk boundary: missing data inside the DEM. Ends at the rim of
    /// the data (a lattice column or row short of the last chunk's edge) count as outer.
    pub interior_ends: u64,
    pub examples: Vec<String>,
}

type Key = (u8, i32);

/// Each boundary endpoint must appear in exactly the two chunks that share it.
pub fn check(files: &[(ChunkId, PathBuf)]) -> Result<Report> {
    let present: std::collections::HashSet<ChunkId> = files.iter().map(|(id, _)| *id).collect();

    #[derive(Default)]
    struct Partial {
        endpoints: u64,
        outer: u64,
        interior: u64,
        examples: Vec<String>,
        points: Vec<(Key, (i32, i32))>,
    }
    let parts = files
        .par_iter()
        .map(|(id, path)| -> Result<Partial> {
            let mut part = Partial::default();
            let (w, s, e, n) = id.bounds();
            let near = |v: i32, b: f64| (f64::from(v) - b * 1e7).abs() <= 2.0;
            for line in chunks::read_lines(path)? {
                if line.closed {
                    continue;
                }
                for &(lon, lat) in [line.points.first(), line.points.last()].into_iter().flatten() {
                    part.endpoints += 1;
                    let west = near(lon, w);
                    let east = near(lon, e);
                    let south = near(lat, s);
                    let north = near(lat, n);
                    if !(west || east || south || north) {
                        // The rim of the data stops a lattice step short of the chunk's edge,
                        // when nothing lies past it.
                        let rim = |v: i32, b: f64| (f64::from(v) - b * 1e7).abs() <= 3.0e7 / 3600.0;
                        let next = |dx: i32, dy: i32| !present.contains(&ChunkId { x: id.x + dx, y: id.y + dy });
                        let at_rim = (rim(lon, e) && next(1, 0))
                            || (rim(lon, w) && next(-1, 0))
                            || (rim(lat, n) && next(0, 1))
                            || (rim(lat, s) && next(0, -1));
                        if at_rim {
                            part.outer += 1;
                        } else {
                            part.interior += 1;
                            if part.examples.len() < 4 {
                                part.examples.push(format!(
                                    "interior end {:?} {} at {:.6},{:.6}",
                                    line.unit,
                                    line.elevation,
                                    f64::from(lon) * 1e-7,
                                    f64::from(lat) * 1e-7
                                ));
                            }
                        }
                        continue;
                    }
                    let neighbor = ChunkId {
                        x: id.x + i32::from(east) - i32::from(west),
                        y: id.y + i32::from(north) - i32::from(south),
                    };
                    if !present.contains(&neighbor) {
                        part.outer += 1;
                        continue;
                    }
                    part.points.push(((line.unit as u8, line.elevation), (lon, lat)));
                }
            }
            Ok(part)
        })
        .collect::<Result<Vec<_>>>()?;

    let mut report = Report::default();
    let mut counts: HashMap<(Key, (i32, i32)), u32> = HashMap::new();
    for part in parts {
        report.endpoints += part.endpoints;
        report.outer += part.outer;
        report.interior_ends += part.interior;
        if report.examples.len() < 8 {
            report.examples.extend(part.examples);
        }
        report.on_boundary += part.points.len() as u64;
        for point in part.points {
            *counts.entry(point).or_default() += 1;
        }
    }
    let mut lonely: HashMap<Key, Vec<(i32, i32)>> = HashMap::new();
    for (&(key, point), &count) in &counts {
        report.matched += u64::from(count & !1);
        if count % 2 == 1 {
            lonely.entry(key).or_default().push(point);
        }
    }
    for (key, mut points) in lonely {
        points.sort_unstable();
        let mut i = 0;
        while i < points.len() {
            let close = i + 1 < points.len()
                && (points[i].0 - points[i + 1].0).abs() <= 4
                && (points[i].1 - points[i + 1].1).abs() <= 4;
            if close {
                report.matched_near += 2;
                i += 2;
            } else {
                report.unmatched += 1;
                if report.examples.len() < 8 {
                    report.examples.push(format!(
                        "{key:?} at {:.7},{:.7}",
                        f64::from(points[i].0) * 1e-7,
                        f64::from(points[i].1) * 1e-7
                    ));
                }
                i += 1;
            }
        }
    }
    Ok(report)
}
