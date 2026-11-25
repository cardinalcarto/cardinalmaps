use crate::chunks::{self, Mosaic};
use crate::config::{Config, ContourConfig, SourceConfig};
use crate::contour::{self, Keep, Line, Params, Unit};
use crate::dem::{self, Grid, Rect};

fn params() -> Params {
    Params {
        interval_m: 10,
        interval_ft: 40,
        simplify_m: 0.0,
        min_loop_m: 0.0,
        round_corners: false,
    }
}

fn surface(x: i64, y: i64) -> f32 {
    let (fx, fy) = (x as f32, y as f32);
    400.0 + 90.0 * (fx * 0.021).sin() + 60.0 * (fy * 0.017).cos() + 25.0 * ((fx + fy) * 0.07).sin()
}

fn grid_over(rect: Rect, refine: i64) -> Grid {
    let mut grid = Grid::new(rect, refine);
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let i = (y - rect.y0) as usize * rect.width() + (x - rect.x0) as usize;
            grid.values[i] = surface(x, y);
        }
    }
    grid
}

/// Traces two adjacent chunks from different windows: their boundary endpoints must be the same.
#[test]
fn neighbors_share_exact_endpoints() {
    let (b, n) = (40usize, 200i64);
    let keep = Keep {
        cols: (b, b + n as usize),
        rows: (b, b + n as usize),
    };
    let rect = |x0: i64| Rect {
        x0: x0 - b as i64,
        y0: -(b as i64),
        x1: x0 + n + b as i64 + 1,
        y1: n + b as i64 + 1,
    };
    let a = contour::contours(&grid_over(rect(0), 1), keep, &params());
    let c = contour::contours(&grid_over(rect(n), 1), keep, &params());
    let boundary = Grid::lon(n as f64, 1) * 1e7;
    let ends = |lines: &[Line]| {
        let mut found: Vec<_> = lines
            .iter()
            .filter(|l| !l.closed)
            .flat_map(|l| {
                [l.points[0], *l.points.last().unwrap()].map(|p| (l.unit, l.elevation, p))
            })
            .filter(|(_, _, p)| (f64::from(p.0) - boundary).abs() <= 1.0)
            .collect();
        found.sort_by_key(|&(u, e, p)| (u, e, p));
        found
    };
    let (from_a, from_c) = (ends(&a), ends(&c));
    assert!(
        from_a.len() > 20,
        "the surface should cross the boundary often"
    );
    assert_eq!(from_a, from_c);
}

#[test]
fn contours_keep_uphill_on_the_left() {
    // A plane rising to the east: lines at a level run north with uphill... on the right
    // would be wrong, so they must run south.
    let rect = Rect {
        x0: 0,
        y0: 0,
        x1: 100,
        y1: 100,
    };
    let mut grid = Grid::new(rect, 1);
    for y in 0..100 {
        for x in 0..100 {
            grid.values[y * 100 + x] = x as f32 * 2.0;
        }
    }
    let keep = Keep {
        cols: (0, 99),
        rows: (0, 99),
    };
    let lines = contour::contours(&grid, keep, &params());
    let line = lines
        .iter()
        .find(|l| l.unit == Unit::Meters && l.elevation == 100)
        .unwrap();
    assert!(
        line.points[0].1 > line.points.last().unwrap().1,
        "should run south with east on its left"
    );
    assert!(
        lines.iter().all(|l| l.elevation != 0),
        "no contour at sea level"
    );
}

#[test]
fn small_loops_drop_and_large_loops_close() {
    let rect = Rect {
        x0: 0,
        y0: 0,
        x1: 200,
        y1: 200,
    };
    let mut grid = Grid::new(rect, 1);
    for y in 0..200i64 {
        for x in 0..200i64 {
            let d = (((x - 100).pow(2) + (y - 100).pow(2)) as f32).sqrt();
            grid.values[(y * 200 + x) as usize] = (100.0 - d * 2.0).max(0.0) + 5.0;
        }
    }
    let keep = Keep {
        cols: (0, 199),
        rows: (0, 199),
    };
    let mut p = params();
    p.min_loop_m = 800.0;
    let lines = contour::contours(&grid, keep, &p);
    let metric: Vec<_> = lines.iter().filter(|l| l.unit == Unit::Meters).collect();
    assert!(
        metric
            .iter()
            .all(|l| l.closed && l.points.first() == l.points.last())
    );
    // Radius 2.5 cells per 5 m of height: rings under 800 m around are the highest ones.
    let top = metric.iter().map(|l| l.elevation).max().unwrap();
    assert!((60..100).contains(&top), "highest kept ring: {top}");
}

#[test]
fn missing_data_ends_lines_without_tracing_through() {
    let rect = Rect {
        x0: 0,
        y0: 0,
        x1: 50,
        y1: 50,
    };
    let mut grid = Grid::new(rect, 1);
    for y in 0..50 {
        for x in 0..50 {
            grid.values[y * 50 + x] = if x == 25 { f32::NAN } else { x as f32 * 4.0 };
        }
    }
    let lines = contour::contours(
        &grid,
        Keep {
            cols: (0, 49),
            rows: (0, 49),
        },
        &params(),
    );
    assert!(lines.iter().all(|l| !l.closed));
    assert!(!lines.is_empty());
    for line in &lines {
        for &(lon, _) in &line.points {
            let col = (f64::from(lon) * 1e-7 + 180.0) * 3600.0 - 0.5;
            assert!(
                !(24.01..25.99).contains(&col),
                "crossed the gap at column {col}"
            );
        }
    }
}

#[test]
fn feather_reaches_one_inside_and_zero_at_the_edge() {
    let rect = Rect {
        x0: 0,
        y0: 0,
        x1: 40,
        y1: 3,
    };
    let mut grid = Grid::new(rect, 1);
    for row in 0..3 {
        for col in 10..40 {
            grid.values[row * 40 + col] = 1.0;
        }
    }
    let weights = dem::feather_weights(&grid, 8);
    assert_eq!(weights[40 + 5], 0.0);
    assert!(weights[40 + 11] > 0.0 && weights[40 + 11] < 0.2);
    assert_eq!(weights[40 + 30], 1.0);
}

#[test]
fn simplify_pins_ends_and_respects_tolerance() {
    let line: Vec<(i32, i32)> = (0..100)
        .map(|i| (i * 1000, if i % 2 == 0 { 0 } else { 30 }))
        .collect();
    let simple = contour::simplify(&line, false, 5.0);
    assert_eq!(simple, vec![line[0], line[99]]);
    assert_eq!(contour::simplify(&line, false, 0.1).len(), 100);
}

#[test]
fn rounding_corners_keeps_the_ends_of_open_lines_and_closes_loops() {
    let open = [(0, 0), (1000, 0), (1000, 1000)];
    assert_eq!(
        contour::round_corners(&open, false),
        vec![(0, 0), (750, 0), (1000, 250), (1000, 1000)]
    );
    let square = [(0, 0), (1000, 0), (1000, 1000), (0, 1000), (0, 0)];
    let rounded = contour::round_corners(&square, true);
    assert_eq!(rounded.first(), rounded.last());
    assert_eq!(rounded.len(), 9);
}

#[test]
fn chunk_files_round_trip() {
    let lines = vec![
        Line {
            unit: Unit::Feet,
            elevation: -40,
            closed: false,
            points: vec![(-1, 2), (1_000_000, -2_000_000)],
        },
        Line {
            unit: Unit::Meters,
            elevation: 8840,
            closed: true,
            points: vec![(0, 0), (5, 0), (5, 5), (0, 0)],
        },
    ];
    let dir = std::env::temp_dir().join(format!("topo-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.bin");
    std::fs::write(&path, chunks::encode(&lines).unwrap()).unwrap();
    assert_eq!(chunks::read_lines(&path).unwrap(), lines);
    assert_eq!(chunks::read_counts(&path).unwrap(), (5, 2));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A raster of 1 arc second cells, written as a little GeoTIFF.
fn write_tiff(
    path: &std::path::Path,
    west: f64,
    north: f64,
    dx: f64,
    values: &[i16],
    size: u32,
    nodata: Option<&str>,
) {
    let mut tags: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
    let short = |v: u16| v.to_le_bytes().to_vec();
    tags.push((256, 3, 1, short(size as u16)));
    tags.push((257, 3, 1, short(size as u16)));
    tags.push((258, 3, 1, short(16)));
    tags.push((259, 3, 1, short(1)));
    tags.push((262, 3, 1, short(1)));
    tags.push((277, 3, 1, short(1)));
    tags.push((278, 3, 1, short(size as u16)));
    tags.push((339, 3, 1, short(2)));
    let doubles = |v: &[f64]| v.iter().flat_map(|d| d.to_le_bytes()).collect::<Vec<_>>();
    tags.push((33550, 12, 3, doubles(&[dx, dx, 0.0])));
    tags.push((33922, 12, 6, doubles(&[0.0, 0.0, 0.0, west, north, 0.0])));
    let keys: Vec<u8> = [1u16, 1, 0, 2, 1024, 0, 1, 2, 1025, 0, 1, 1]
        .iter()
        .flat_map(|k| k.to_le_bytes())
        .collect();
    tags.push((34735, 3, 12, keys));
    if let Some(n) = nodata {
        let mut text = n.as_bytes().to_vec();
        text.push(0);
        tags.push((42113, 2, text.len() as u32, text));
    }
    let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    tags.push((273, 4, 1, Vec::new()));
    tags.push((279, 4, 1, (data.len() as u32).to_le_bytes().to_vec()));
    tags.sort_by_key(|t| t.0);
    let ifd_size = 2 + tags.len() * 12 + 4;
    let data_offset = 8 + ifd_size;
    let mut extra_offset = data_offset + data.len();
    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    out.extend((tags.len() as u16).to_le_bytes());
    let mut extra = Vec::new();
    for (tag, kind, count, bytes) in &tags {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        if *tag == 273 {
            out.extend((data_offset as u32).to_le_bytes());
        } else if bytes.len() <= 4 {
            let mut inline = bytes.clone();
            inline.resize(4, 0);
            out.extend(inline);
        } else {
            out.extend((extra_offset as u32).to_le_bytes());
            extra_offset += bytes.len();
            extra.extend_from_slice(bytes);
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(data);
    out.extend(extra);
    std::fs::write(path, out).unwrap();
}

fn raster_values(west: i64, north: i64, size: i64, step: i64) -> Vec<i16> {
    // `surface` on the global 1 arc second lattice, so separate files agree.
    let mut values = Vec::new();
    let col0 = (west + 180) * 3600 / step;
    let row_top = (north + 90) * 3600 / step;
    for row in 0..size {
        for col in 0..size {
            values.push(surface(col0 + col, row_top - 1 - row) as i16);
        }
    }
    values
}

/// End to end: two files, a seam between them inside one pair of chunks, and a chunk
/// boundary; every endpoint on a shared boundary has its partner in the neighbor.
#[test]
fn chunks_across_a_file_seam_meet() {
    let dir = std::env::temp_dir().join(format!("topo-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let px = 1.0 / 3600.0;
    for west in [10i64, 11] {
        write_tiff(
            &dir.join(format!("{west}_DSM.tif")),
            west as f64,
            2.0,
            px,
            &raster_values(west, 2, 3600, 1),
            3600,
            None,
        );
    }
    let config = Config {
        contours: ContourConfig {
            simplify_m: 2.0,
            min_loop_m: 250.0,
            cache_gb: 1.0,
            ..ContourConfig::default()
        },
        sources: vec![SourceConfig {
            name: "test".into(),
            dir: dir.clone(),
            s3: None,
            suffix: "_DSM.tif".into(),
            arcsec: 1.0,
            median: true,
            sigma: 1.2,
            feather: 0,
        }],
    };
    let mosaic = Mosaic::open(&config, &dir).unwrap();
    let work = dir.join("work");
    // Straddles the file seam at lon 11 and the chunk boundaries around it.
    let ids = mosaic.chunks(Some((10.76, 1.26, 11.24, 1.74)));
    assert_eq!(ids.len(), 4);
    let files = mosaic.ensure(&work, &ids).unwrap();
    let report = crate::seams::check(&files).unwrap();
    assert!(report.on_boundary > 50, "{report:?}");
    assert_eq!(report.unmatched, 0, "{report:?}");
    assert_eq!(report.matched_near, 0, "{report:?}");

    // The whole path through a planet: spliced in the right order and readable.
    let out = dir.join("contours.osm.pbf");
    let totals =
        crate::assemble::write_contours(&files, 20_000_000_000, 4_000_000_000, &out).unwrap();
    assert!(totals.ways > 0);
    let mut file = std::fs::File::open(&out).unwrap();
    let blobs = crate::pbf::scan_blobs(&mut file).unwrap();
    let kinds: Vec<_> = blobs[1..]
        .iter()
        .map(|b| crate::pbf::block_kind(&crate::pbf::read_blob(&mut file, b).unwrap()).unwrap())
        .collect();
    let first_way = kinds
        .iter()
        .position(|k| matches!(k, crate::pbf::BlockKind::Ways(_)))
        .unwrap();
    assert!(
        kinds[..first_way]
            .iter()
            .all(|k| matches!(k, crate::pbf::BlockKind::Nodes(_)))
    );
    assert!(
        kinds[first_way..]
            .iter()
            .all(|k| matches!(k, crate::pbf::BlockKind::Ways(_)))
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A 1/3 arc second source covering a quarter degree: the fine chunks' edges must meet the
/// coarse chunks around them, including on the boundary where the lattices differ.
#[test]
fn a_finer_source_meets_the_coarse_one() {
    let dir = std::env::temp_dir().join(format!("topo-fine-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("coarse")).unwrap();
    std::fs::create_dir_all(dir.join("fine")).unwrap();
    write_tiff(
        &dir.join("coarse/a_DSM.tif"),
        11.0,
        2.0,
        1.0 / 3600.0,
        &raster_values(11, 2, 3600, 1),
        3600,
        None,
    );
    // The same terrain and then some detail, as a better survey would show.
    let size = 2700usize;
    let mut fine = Vec::with_capacity(size * size);
    let (col0, row_top) = (
        (11 + 180) * 3600 * 3 + 3 * 900,
        (1i64 + 90) * 3600 * 3 + 3 * 2700,
    );
    for row in 0..size as i64 {
        for col in 0..size as i64 {
            let (x, y) = (col0 + col, row_top - 1 - row);
            let detail = 6.0 * (x as f32 * 0.09).sin() * (y as f32 * 0.07).cos();
            fine.push((surface(x / 3, y / 3) + detail) as i16);
        }
    }
    write_tiff(
        &dir.join("fine/f_DEM.tif"),
        11.25,
        1.75,
        1.0 / 10800.0,
        &fine,
        size as u32,
        None,
    );

    let config = Config {
        contours: ContourConfig {
            cache_gb: 2.0,
            ..ContourConfig::default()
        },
        sources: vec![
            SourceConfig {
                name: "coarse".into(),
                dir: dir.join("coarse"),
                s3: None,
                suffix: "_DSM.tif".into(),
                arcsec: 1.0,
                median: false,
                sigma: 0.0,
                feather: 0,
            },
            SourceConfig {
                name: "fine".into(),
                dir: dir.join("fine"),
                s3: None,
                suffix: "_DEM.tif".into(),
                arcsec: 1.0 / 3.0,
                median: false,
                sigma: 1.0,
                feather: 60,
            },
        ],
    };
    let mosaic = Mosaic::open(&config, &dir).unwrap();
    assert_eq!(mosaic.sources[1].refine, 3);
    let ids = mosaic.chunks(Some((11.1, 1.35, 11.6, 1.9)));
    let files = mosaic.ensure(&dir.join("work"), &ids).unwrap();
    let report = crate::seams::check(&files).unwrap();
    assert!(report.on_boundary > 50, "{report:?}");
    assert_eq!(report.unmatched, 0, "{report:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
