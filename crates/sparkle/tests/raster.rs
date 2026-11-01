//! Raster, raster-dem and image sources, through a style, to buckets; and the generator of
//! the synthetic fixtures in `tests/fixtures/raster` (run it with `--ignored`).
//!
//! The fixtures are a few Gaussian hills around Seattle as DEM tiles in the `terrarium` and
//! `mapbox` encodings, a colorful raster tileset, and an image for an image source, so the
//! harness can draw raster layers over the Seattle vector tiles without any real data.

use std::f64::consts::PI;
use std::path::Path;

use serde_json::json;
use sparkle::dem::{DemData, decode_slopes};
use sparkle::style::{Color, LayerKind, Source};
use sparkle::tile::{ColorRamp, RasterImage, image_source_tile};
use sparkle::{BucketGeometry, Style, TileId, TileProcessor};

/// The `tileSize` of every fixture tileset, and the pixels per side of raster tiles.
const TILE: u32 = 256;
/// Pixels per side of DEM tiles. Smaller than [`TILE`] to keep the files small, and to
/// exercise DEMs whose resolution differs from their `tileSize`.
const DEM: u32 = 128;
/// `[west, south, east, north]` of the fixture tilesets.
const BOUNDS: [f64; 4] = [-122.43, 47.55, -122.25, 47.70];
const MIN_ZOOM: u8 = 8;
const MAX_ZOOM: u8 = 12;

/// `(lng, lat, height in m, radius in m)`
const HILLS: [(f64, f64, f64, f64); 5] = [
    (-122.335, 47.608, 420.0, 1400.0),
    (-122.300, 47.650, 260.0, 900.0),
    (-122.390, 47.570, 330.0, 2200.0),
    (-122.270, 47.590, 180.0, 600.0),
    (-122.360, 47.680, 300.0, 1200.0),
];

fn world_to_lng_lat(x: f64, y: f64) -> (f64, f64) {
    let lng = (x - 0.5) * 360.0;
    let lat = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    (lng, lat)
}

fn lng_lat_to_world(lng: f64, lat: f64) -> (f64, f64) {
    let lat = lat.to_radians();
    (
        lng / 360.0 + 0.5,
        0.5 - (PI / 4.0 + lat / 2.0).tan().ln() / (2.0 * PI),
    )
}

/// The synthetic terrain: sea level slopes up to a plateau, with hills on it.
fn elevation(lng: f64, lat: f64) -> f64 {
    let m_per_deg_lat = 111_320.0;
    let m_per_deg_lng = m_per_deg_lat * lat.to_radians().cos();
    let base = 30.0 + 40.0 * ((lng + 122.34) * 40.0).sin();
    HILLS.iter().fold(base, |e, &(hx, hy, h, r)| {
        let dx = (lng - hx) * m_per_deg_lng;
        let dy = (lat - hy) * m_per_deg_lat;
        e + h * (-(dx * dx + dy * dy) / (2.0 * r * r)).exp()
    })
}

/// The elevation at the center of each pixel of a tile, rounded to quarter meters so the
/// PNGs compress well.
fn tile_elevations(id: TileId) -> Vec<f64> {
    let n = f64::from(1u32 << id.z);
    let mut out = Vec::with_capacity((DEM * DEM) as usize);
    for py in 0..DEM {
        for px in 0..DEM {
            let x = (f64::from(id.x) + (f64::from(px) + 0.5) / f64::from(DEM)) / n;
            let y = (f64::from(id.y) + (f64::from(py) + 0.5) / f64::from(DEM)) / n;
            let (lng, lat) = world_to_lng_lat(x, y);
            out.push((elevation(lng, lat) * 4.0).round() / 4.0);
        }
    }
    out
}

fn terrarium(e: f64) -> [u8; 4] {
    let v = e + 32768.0;
    [
        (v / 256.0) as u8,
        (v % 256.0) as u8,
        ((v.fract()) * 256.0) as u8,
        255,
    ]
}

fn mapbox(e: f64) -> [u8; 4] {
    let v = ((e + 10000.0) * 10.0).round() as u32;
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut png = png::Encoder::new(&mut out, width, height);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.set_compression(png::Compression::High);
    png.write_header().unwrap().write_image_data(rgba).unwrap();
    out
}

fn encode_webp(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out)
        .encode(rgba, width, height, image_webp::ColorType::Rgba8)
        .unwrap();
    out
}

fn encode_jpeg(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 95)
        .encode(
            rgba,
            width as u16,
            height as u16,
            jpeg_encoder::ColorType::Rgba,
        )
        .unwrap();
    out
}

fn dem_tile(id: TileId, encode: fn(f64) -> [u8; 4]) -> Vec<u8> {
    let rgba: Vec<u8> = tile_elevations(id).into_iter().flat_map(encode).collect();
    encode_png(DEM, DEM, &rgba)
}

/// A colorful tile: bands of hue by distance from downtown, and a darker line along the
/// tile's edges so tile boundaries and zoom levels can be told apart.
fn raster_rgba(id: TileId) -> Vec<u8> {
    let n = f64::from(1u32 << id.z);
    let center = lng_lat_to_world(-122.335, 47.608);
    let mut rgba = Vec::with_capacity((TILE * TILE * 4) as usize);
    for py in 0..TILE {
        for px in 0..TILE {
            let x = (f64::from(id.x) + (f64::from(px) + 0.5) / f64::from(TILE)) / n;
            let y = (f64::from(id.y) + (f64::from(py) + 0.5) / f64::from(TILE)) / n;
            let d = ((x - center.0).hypot(y - center.1) * 2f64.powi(14)).fract();
            // Twelve flat bands, so the tiles compress well.
            let hue = (d * 12.0).floor() / 2.0;
            let channel = |offset: f64| {
                let h = (hue + offset).rem_euclid(6.0);
                let v = (h - 3.0).abs() - 1.0;
                (v.clamp(0.0, 1.0) * 200.0 + 40.0) as u8
            };
            let edge = px < 2 || py < 2;
            let shade = if edge { 0.5 } else { 1.0 };
            let [r, g, b] =
                [channel(0.0), channel(4.0), channel(2.0)].map(|c| (f64::from(c) * shade) as u8);
            rgba.extend([r, g, b, 255]);
        }
    }
    rgba
}

fn tiles_in_bounds(z: u8) -> impl Iterator<Item = TileId> {
    let n = f64::from(1u32 << z);
    let (x0, y0) = lng_lat_to_world(BOUNDS[0], BOUNDS[3]);
    let (x1, y1) = lng_lat_to_world(BOUNDS[2], BOUNDS[1]);
    let xs = (x0 * n) as u32..=(x1 * n) as u32;
    let ys = (y0 * n) as u32..=(y1 * n) as u32;
    ys.flat_map(move |y| xs.clone().map(move |x| TileId::new(z, x, y)))
}

fn write_mbtiles(path: &Path, format: &str, max_zoom: u8, tile: impl Fn(TileId) -> Vec<u8>) {
    let _ = std::fs::remove_file(path);
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE metadata (name TEXT, value TEXT);
         CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER,
                             tile_data BLOB);
         CREATE UNIQUE INDEX tile_index ON tiles (zoom_level, tile_column, tile_row);",
    )
    .unwrap();
    let [w, s, e, n] = BOUNDS;
    for (name, value) in [
        (
            "name",
            path.file_stem().unwrap().to_str().unwrap().to_owned(),
        ),
        ("format", format.to_owned()),
        ("minzoom", MIN_ZOOM.to_string()),
        ("maxzoom", max_zoom.to_string()),
        ("bounds", format!("{w},{s},{e},{n}")),
        ("center", "-122.335,47.608,11".to_owned()),
    ] {
        db.execute("INSERT INTO metadata VALUES (?1, ?2)", (name, value))
            .unwrap();
    }
    for z in MIN_ZOOM..=max_zoom {
        for id in tiles_in_bounds(z) {
            let row = (1u32 << z) - 1 - id.y;
            db.execute(
                "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
                (z, id.x, row, tile(id)),
            )
            .unwrap();
        }
    }
    db.execute_batch("VACUUM").unwrap();
}

/// The image source fixture: a translucent frame with a gradient inside.
fn overlay_rgba() -> (u32, u32, Vec<u8>) {
    let (w, h) = (96, 64);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let frame = x < 4 || y < 4 || x >= w - 4 || y >= h - 4;
            if frame {
                rgba.extend([20, 20, 120, 255]);
            } else {
                rgba.extend([(x * 255 / w) as u8, 60, (y * 255 / h) as u8, 160]);
            }
        }
    }
    (w, h, rgba)
}

/// Regenerates `tests/fixtures/raster`.
#[test]
#[ignore]
fn write_fixtures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/raster");
    std::fs::create_dir_all(&dir).unwrap();
    write_mbtiles(&dir.join("dem-terrarium.mbtiles"), "png", MAX_ZOOM, |id| {
        dem_tile(id, terrarium)
    });
    write_mbtiles(&dir.join("dem-mapbox.mbtiles"), "png", MAX_ZOOM, |id| {
        dem_tile(id, mapbox)
    });
    // One zoom level less than the DEMs keeps the file small.
    write_mbtiles(&dir.join("raster.mbtiles"), "png", MAX_ZOOM - 1, |id| {
        encode_png(TILE, TILE, &raster_rgba(id))
    });
    let (w, h, rgba) = overlay_rgba();
    std::fs::write(dir.join("overlay.png"), encode_png(w, h, &rgba)).unwrap();
}

fn style(layers: serde_json::Value) -> Style {
    let style = Style::from_json(&json!({
        "version": 8,
        "sources": {
            "terrarium": {"type": "raster-dem", "encoding": "terrarium", "tileSize": 256},
            "mapbox": {"type": "raster-dem", "tileSize": 256},
            "raster": {"type": "raster", "tileSize": 256},
            "overlay": {"type": "image", "url": "overlay.png", "coordinates": [
                [-122.36, 47.62], [-122.32, 47.62], [-122.32, 47.60], [-122.36, 47.60]
            ]}
        },
        "layers": layers
    }))
    .unwrap();
    assert!(style.warnings.is_empty(), "{:?}", style.warnings);
    style
}

/// A zoom 12 tile over the first hill, and its neighbours.
fn hill_tile() -> TileId {
    let (x, y) = lng_lat_to_world(HILLS[0].0, HILLS[0].1);
    TileId::new(12, (x * 4096.0) as u32, (y * 4096.0) as u32)
}

#[test]
fn both_dem_encodings_decode_to_the_same_terrain() {
    let style = style(json!([
        {"id": "a", "type": "hillshade", "source": "terrarium"},
        {"id": "b", "type": "color-relief", "source": "mapbox",
         "paint": {"color-relief-color": ["interpolate", ["linear"], ["elevation"],
                                          0, "blue", 500, "red"]}}
    ]));
    let id = hill_tile();
    let mut processor = TileProcessor::new();
    let a = processor
        .process(&style, "terrarium", id, &dem_tile(id, terrarium))
        .unwrap();
    let b = processor
        .process(&style, "mapbox", id, &dem_tile(id, mapbox))
        .unwrap();
    let [a, b] = [&a, &b].map(|t| {
        assert_eq!(t.buckets.len(), 1);
        assert_eq!(t.buckets[0].extent, DEM);
        match &t.buckets[0].geometry {
            BucketGeometry::Hillshade(h) => {
                assert_eq!(h.tile, id);
                assert_eq!((h.slopes.width, h.slopes.height), (DEM, DEM));
                h.dem.clone()
            }
            BucketGeometry::ColorRelief(c) => c.dem.clone(),
            other => panic!("{other:?}"),
        }
    });
    let expected = tile_elevations(id);
    for (i, &e) in expected.iter().enumerate().step_by(97) {
        let (x, y) = ((i as u32 % DEM) as i32, (i as u32 / DEM) as i32);
        assert_eq!(f64::from(a.get(x, y)), e);
        assert!((f64::from(b.get(x, y)) - e).abs() < 0.06);
    }
    let (lo, hi) = a.min_max();
    assert!(lo > 0.0 && hi > 300.0, "{lo} {hi}");
    // The sampled elevation follows the terrain between pixel centers too.
    let (x, y) = lng_lat_to_world(HILLS[0].0, HILLS[0].1);
    let (u, v) = (x * 4096.0 - f64::from(id.x), y * 4096.0 - f64::from(id.y));
    let peak = f64::from(a.elevation_at(u, v));
    assert!(
        (peak - elevation(HILLS[0].0, HILLS[0].1)).abs() < 3.0,
        "{peak}"
    );
}

#[test]
fn backfilled_borders_make_slopes_seamless() {
    // Terrain rising steadily to the east and curving to the south, across two tiles.
    let z = 12;
    let terrain = |x: u32, y: u32| (x as f32 * 10.0) + (y as f32 * 0.02).powi(2) * 50.0;
    let mut left = DemData::from_fn(TILE, terrain);
    let mut right = DemData::from_fn(TILE, |x, y| terrain(x + TILE, y));
    let px = |image: &RasterImage, x: u32, y: u32| {
        let i = ((y * image.width + x) * 4) as usize;
        decode_slopes(image.pixels[i..i + 4].try_into().unwrap())
    };
    // How far the slopes on either side of the seam are from their neighbours inside
    // each tile, at worst. The corner pixels need the diagonal neighbours too, so they're
    // left out.
    let seam = |left: &DemData, right: &DemData| {
        let (l, r) = (left.hillshade_slopes(z), right.hillshade_slopes(z));
        let diff = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs().max((a[1] - b[1]).abs());
        (1..TILE - 1)
            .map(|y| {
                let edge = [px(&l, TILE - 1, y), px(&r, 0, y)];
                diff(edge[0], px(&l, TILE - 2, y))
                    .max(diff(edge[1], px(&r, 1, y)))
                    .max(diff(edge[0], edge[1]))
            })
            .fold(0.0, f32::max)
    };
    let interior = left.hillshade_slopes(z);
    let before = seam(&left, &right);
    let (left_copy, right_copy) = (left.clone(), right.clone());
    left.backfill_border(&right_copy, 1, 0);
    right.backfill_border(&left_copy, -1, 0);
    let after = seam(&left, &right);
    assert!(
        before > 0.1 && after < 0.002,
        "before {before}, after {after}"
    );
    // Only the edge column changes.
    let backfilled = left.hillshade_slopes(z);
    for y in 0..TILE {
        assert_eq!(px(&interior, TILE - 2, y), px(&backfilled, TILE - 2, y));
    }
}

#[test]
fn raster_tiles_decode_from_every_format() {
    let style = style(json!([
        {"id": "r", "type": "raster", "source": "raster"}
    ]));
    let id = hill_tile();
    let rgba = raster_rgba(id);
    let mut processor = TileProcessor::new();
    for (format, data) in [
        ("png", encode_png(TILE, TILE, &rgba)),
        ("webp", encode_webp(TILE, TILE, &rgba)),
        ("jpeg", encode_jpeg(TILE, TILE, &rgba)),
    ] {
        let tile = processor.process(&style, "raster", id, &data).unwrap();
        let BucketGeometry::Raster(r) = &tile.buckets[0].geometry else {
            panic!("{format}: {:?}", tile.buckets);
        };
        assert_eq!(tile.buckets[0].extent, TILE, "{format}");
        assert_eq!(r.tile, id);
        assert_eq!((r.image.width, r.image.height), (TILE, TILE), "{format}");
        let tolerance = if format == "jpeg" { 24 } else { 0 };
        let diff = r
            .image
            .pixels
            .iter()
            .zip(&rgba)
            .map(|(&a, &b)| a.abs_diff(b))
            .max()
            .unwrap();
        assert!(diff <= tolerance, "{format}: {diff}");
        assert_eq!(r.vertices[3].position, [TILE as f32; 2]);
        assert_eq!(r.vertices[3].texcoord, [1.0, 1.0]);
    }
}

#[test]
fn raster_images_are_premultiplied() {
    let style = style(json!([{"id": "r", "type": "raster", "source": "raster"}]));
    let data = encode_png(1, 1, &[200, 100, 50, 128]);
    let tile = TileProcessor::new()
        .process(&style, "raster", TileId::new(0, 0, 0), &data)
        .unwrap();
    let BucketGeometry::Raster(r) = &tile.buckets[0].geometry else {
        panic!()
    };
    assert_eq!(r.image.pixels, [100, 50, 25, 128]);
}

#[test]
fn image_source_is_placed_in_its_tile() {
    let style = style(json!([
        {"id": "o", "type": "raster", "source": "overlay"},
        {"id": "h", "type": "hillshade", "source": "overlay"}
    ]));
    let Some(Source::Image(source)) = style.source("overlay") else {
        panic!()
    };
    let id = image_source_tile(source);
    let (w, h, rgba) = overlay_rgba();
    let tile = TileProcessor::new()
        .process(&style, "overlay", id, &encode_png(w, h, &rgba))
        .unwrap();
    // Hillshade layers can't draw an image source.
    assert_eq!(tile.buckets.len(), 1);
    let bucket = &tile.buckets[0];
    let BucketGeometry::Raster(r) = &bucket.geometry else {
        panic!()
    };
    assert_eq!((r.image.width, r.image.height), (w, h));
    let extent = bucket.extent as f32;
    for v in r.vertices {
        assert!(
            v.position.iter().all(|p| (0.0..=extent).contains(p)),
            "{v:?}"
        );
    }
    // Top left, top right, bottom left, bottom right.
    let [tl, tr, bl, br] = r.vertices.map(|v| v.position);
    assert!(tl[0] < tr[0] && tl[1] < bl[1] && (br[0] - tr[0]).abs() < 1e-3);
}

#[test]
fn color_ramp_follows_the_terrain() {
    let style = style(json!([
        {"id": "c", "type": "color-relief", "source": "terrarium",
         "paint": {"color-relief-color": ["interpolate", ["linear"], ["elevation"],
                                          0, "#000000", 400, "#ffffff"]}}
    ]));
    let LayerKind::ColorRelief(layer) = &style.layers[0].kind else {
        panic!()
    };
    let ramp = ColorRamp::bake(&layer.paint.color);
    let grey = ramp.color_at(200.0);
    assert!((126..=129).contains(&grey[0]), "{grey:?}");
    assert_eq!(layer.paint.color.evaluate_at_zoom(10.0), Some(Color::BLACK));
}

#[test]
fn layers_of_other_sources_get_no_bucket() {
    let style = style(json!([
        {"id": "r", "type": "raster", "source": "raster"},
        {"id": "h", "type": "hillshade", "source": "terrarium", "maxzoom": 10}
    ]));
    let id = hill_tile();
    let tile = TileProcessor::new()
        .process(&style, "terrarium", id, &dem_tile(id, terrarium))
        .unwrap();
    // Zoom 12 is past the hillshade layer's maxzoom.
    assert!(tile.buckets.is_empty());
}

#[test]
fn terrain_source_tiles_carry_elevations() {
    // No layer draws the terrain source; its tiles still bring their elevations.
    let mut json = json!({
        "version": 8,
        "sources": {
            "terrarium": {"type": "raster-dem", "encoding": "terrarium", "tileSize": 256},
            "raster": {"type": "raster", "tileSize": 256}
        },
        "terrain": {"source": "terrarium", "exaggeration": 2},
        "layers": [{"id": "r", "type": "raster", "source": "raster"}]
    });
    let style = Style::from_json(&json).unwrap();
    let id = hill_tile();
    let mut processor = TileProcessor::new();
    let tile = processor
        .process(&style, "terrarium", id, &dem_tile(id, terrarium))
        .unwrap();
    assert!(tile.buckets.is_empty());
    let dem = tile.dem.expect("terrain tiles carry their DEM");
    let mut tiles = sparkle::terrain::TerrainTiles::new();
    tiles.insert(id, dem);
    let (x, y) = lng_lat_to_world(HILLS[0].0, HILLS[0].1);
    let peak = f64::from(tiles.elevation_at([x, y]).unwrap());
    assert!(
        (peak - elevation(HILLS[0].0, HILLS[0].1)).abs() < 3.0,
        "{peak}"
    );

    // Raster tiles, and DEM tiles of a style without terrain, carry none.
    let raster = processor
        .process(
            &style,
            "raster",
            id,
            &encode_png(TILE, TILE, &raster_rgba(id)),
        )
        .unwrap();
    assert!(raster.dem.is_none());
    json.as_object_mut().unwrap().remove("terrain");
    let flat = Style::from_json(&json).unwrap();
    let tile = processor
        .process(&flat, "terrarium", id, &dem_tile(id, terrarium))
        .unwrap();
    assert!(tile.dem.is_none());
}
