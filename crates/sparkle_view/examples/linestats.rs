//! PERF-TEMP: line triangle statistics for the z13 tiles around downtown Seattle.
use std::collections::BTreeMap;
use std::path::Path;

use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};
use sparkle::{BucketGeometry, Style, TileId, TileProcessor};
use sparkle_view::mbtiles::MbTiles;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let style: Style = std::fs::read_to_string(root.join("sparkle_harness/styles/headway.json"))
        .unwrap()
        .parse()
        .unwrap();
    let tiles = MbTiles::open(&root.join("seattle.mbtiles")).unwrap();
    let (lon, lat) = (-122.335f64, 47.608f64);
    let n = 8192.0;
    let x = ((lon + 180.0) / 360.0 * n) as u32;
    let lr = lat.to_radians();
    let y = ((1.0 - (lr.tan() + 1.0 / lr.cos()).ln() / std::f64::consts::PI) / 2.0 * n) as u32;
    let mut processor = TileProcessor::new();
    let mut by_layer: BTreeMap<String, [usize; 4]> = BTreeMap::new();
    let mut count = 0;
    let mut caps = 0;
    let mut fills: BTreeMap<String, [usize; 2]> = BTreeMap::new();
    for dx in [-1i32, 0, 1] {
        for dy in [-1i32, 0, 1] {
            let id = TileId::new(13, (x as i32 + dx) as u32, (y as i32 + dy) as u32);
            let Some(mvt) = tiles.tile(id).unwrap() else {
                continue;
            };
            let cfg = EncoderConfig::from_profile(Profile::Native(Network::Offline))
                .with_tessellation(Tessellation::Off);
            let mut data = Vec::new();
            for layer in mlt_core::mvt::mvt_to_tile_layers(&mvt).unwrap() {
                data.extend(layer.encode(cfg).unwrap());
            }
            count += 1;
            let t = processor
                .process(&style, "openmaptiles", id, &data)
                .unwrap();
            for b in &t.buckets {
                if let BucketGeometry::Fill(fill) = &b.geometry {
                    let e = fills
                        .entry(style.layers[b.style_layer].id.clone())
                        .or_default();
                    e[0] += fill.triangles.len() / 3;
                    e[1] += fill.outlines.len() / 2;
                }
                let BucketGeometry::Line(line) = &b.geometry else {
                    continue;
                };
                let e = by_layer
                    .entry(style.layers[b.style_layer].id.clone())
                    .or_default();
                let tris = line.triangles.len() / 3;
                let fan = line
                    .triangles
                    .chunks(3)
                    .filter(|t| {
                        t.iter()
                            .any(|&i| line.vertices[i as usize].extrude == [0.0, 0.0])
                    })
                    .count();
                let cap = line
                    .triangles
                    .chunks(3)
                    .filter(|t| t.iter().any(|&i| line.vertices[i as usize].along != 0.0))
                    .count();
                caps += cap;
                e[0] += tris;
                e[1] += fan;
                e[2] += line.vertices.len();
                e[3] += 1;
            }
        }
    }
    let mut total = [0usize; 3];
    let mut rows: Vec<_> = by_layer.into_iter().collect();
    rows.sort_by_key(|(_, v)| std::cmp::Reverse(v[0]));
    println!("{count} tiles; per tile: layer, triangles, fan (join/cap) triangles, vertices");
    for (id, [t, f, v, _]) in &rows {
        total[0] += t;
        total[1] += f;
        total[2] += v;
        println!(
            "{:>40} {:>8} {:>8} {:>8}",
            id,
            t / count,
            f / count,
            v / count
        );
    }
    println!("cap triangles per tile: {}", caps / count);
    for (id, [t, o]) in &fills {
        println!(
            "fill {:>30} triangles {:>7} outline segments {:>7}",
            id,
            t / count,
            o / count
        );
    }
    println!(
        "{:>40} {:>8} {:>8} {:>8}",
        "TOTAL",
        total[0] / count,
        total[1] / count,
        total[2] / count
    );
}
