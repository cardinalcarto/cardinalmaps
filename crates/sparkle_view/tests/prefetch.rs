//! Loading tiles ahead of the view, and a tile server's local MBTiles read before it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use sparkle::geojson::GeoJsonIndex;
use sparkle::state::FeatureStates;
use sparkle::style::Source;
use sparkle::{ProcessedTile, Style, TileId};
use sparkle_view::camera::Camera;
use sparkle_view::loader::{LoaderConfig, Tier, TileLoader, TileMessage};
use sparkle_view::map::{MapState, TileUploader};
use sparkle_view::motion::Motion;
use sparkle_view::setup::{geojson_config, geojson_map_source};

/// Takes each tile to need a megabyte.
struct Megabytes;

impl TileUploader for Megabytes {
    type Tile = ();
    fn upload(&mut self, _: &Style, _: ProcessedTile, _: &FeatureStates) {}
    fn tile_bytes(&self, _: &()) -> usize {
        1 << 20
    }
    fn style_changed(&mut self) {}
    fn rebuild_paint(&mut self, _: &mut (), _: &Style, _: usize, _: &FeatureStates) -> bool {
        true
    }
    fn update_feature_state(
        &mut self,
        _: &mut (),
        _: &Style,
        _: &FeatureStates,
        _: Option<&str>,
        _: Option<&[u64]>,
    ) -> bool {
        false
    }
}

/// A style of one GeoJSON source, a square over Seattle that every tile there draws.
fn geojson_loader() -> (
    Arc<Style>,
    TileLoader,
    mpsc::Receiver<TileMessage>,
    GeoJsonIndex,
) {
    let square = json!({
        "type": "Polygon",
        "coordinates": [[[-123, 47], [-121, 47], [-121, 48], [-123, 48], [-123, 47]]]
    });
    let style = Arc::new(
        Style::from_json(&json!({
            "version": 8,
            "sources": {"area": {"type": "geojson", "data": square}},
            "layers": [{"id": "fill", "type": "fill", "source": "area"}]
        }))
        .unwrap(),
    );
    let Some(Source::GeoJson(source)) = style.source("area") else {
        unreachable!()
    };
    let index = GeoJsonIndex::new(source, &source.data).unwrap();
    let config = geojson_config(
        "area".into(),
        "".into(),
        GeoJsonIndex::new(source, &source.data).unwrap(),
    );
    let (tx, rx) = mpsc::channel();
    let loader = TileLoader::spawn(
        LoaderConfig {
            style: style.clone(),
            sources: vec![config],
            images: None,
            fonts: None,
        },
        move |m| drop(tx.send(m)),
    )
    .unwrap();
    (style, loader, rx, index)
}

#[test]
fn tiles_wanted_for_later_load_and_unwanted_ones_are_dropped() {
    let (_, loader, rx, _) = geojson_loader();
    let tile = |x| (0, TileId::new(12, x, 1430));
    loader.set_wanted([(tile(655), Tier::Visible), (tile(656), Tier::Ring)]);
    for x in [655, 656, 657] {
        loader.request(tile(x), 12);
    }
    let mut outcomes = HashMap::new();
    for _ in 0..3 {
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            TileMessage::Loaded(s, t, ..) => outcomes.insert((s, t.id), "loaded"),
            TileMessage::Empty(t) => outcomes.insert(t, "empty"),
            TileMessage::Cancelled(t) => outcomes.insert(t, "cancelled"),
        };
    }
    assert_eq!(outcomes[&tile(655)], "loaded");
    assert_eq!(outcomes[&tile(656)], "loaded");
    assert_eq!(outcomes[&tile(657)], "cancelled");
}

struct Fixture {
    map: MapState<()>,
    rx: mpsc::Receiver<TileMessage>,
}

impl Fixture {
    fn new(memory_budget: usize) -> Self {
        let (style, loader, rx, index) = geojson_loader();
        let map = MapState::new(
            style,
            vec![geojson_map_source("area", &index)],
            loader,
            memory_budget,
        );
        Self { map, rx }
    }

    /// Frames of `camera` heading for `end` until nothing is loading.
    fn settle(&mut self, camera: &Camera, end: Option<&Camera>) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let frame = self.map.frame(camera, end);
            if frame.complete && frame.loading == 0 {
                return;
            }
            assert!(Instant::now() < deadline, "never settled");
            if let Ok(m) = self.rx.recv_timeout(Duration::from_secs(5)) {
                self.map.receive(&mut Megabytes, m);
            }
            while let Ok(m) = self.rx.try_recv() {
                self.map.receive(&mut Megabytes, m);
            }
        }
    }
}

/// A view of Seattle, and where a flight from it to well past the tiles around it lands.
fn flight() -> (Camera, Camera) {
    let mut camera = Camera::new([800.0, 600.0], 1.0);
    camera.look_at(-122.33, 47.6, 13.0);
    let mut motion = Motion::default();
    let to = sparkle_view::camera::lng_lat_to_world(-121.8, 47.7);
    motion.fly(&camera, to, 13.0, Instant::now());
    let end = motion.end(&camera).unwrap();
    (camera, end)
}

#[test]
fn the_view_where_a_motion_comes_to_rest_is_loaded_on_the_way() {
    let (camera, end) = flight();
    let mut f = Fixture::new(usize::MAX);
    f.settle(&camera, Some(&end));
    // Nothing more to load once there.
    assert!(f.map.frame(&end, None).complete);
}

#[test]
fn nothing_is_loaded_ahead_without_room_in_memory() {
    let (camera, end) = flight();
    // No more than the view itself takes.
    let mut f = Fixture::new(1 << 20);
    f.settle(&camera, Some(&end));
    assert!(!f.map.frame(&end, None).complete);
}

/// An HTTP server of a TileJSON (`/tiles.json`) and the tiles of `mbtiles`
/// (`/{z}/{x}/{y}`), counting the tile requests it answers by path.
fn serve(mbtiles: &Path) -> (String, Arc<Mutex<HashMap<String, usize>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(HashMap::new()));
    let (counts, path) = (requests.clone(), mbtiles.to_owned());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let (counts, path) = (counts.clone(), path.clone());
            thread::spawn(move || {
                let mut stream = stream.unwrap();
                let tiles = sparkle_view::mbtiles::MbTiles::open(&path).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let target = line.split(' ').nth(1).unwrap_or("").to_owned();
                    // Skip the headers.
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).unwrap();
                        if header == "\r\n" || header.is_empty() {
                            break;
                        }
                    }
                    let (status, body) = if target == "/tiles.json" {
                        let json = json!({
                            "tilejson": "3.0.0",
                            "tiles": [format!("http://127.0.0.1:{port}/{{z}}/{{x}}/{{y}}")],
                            "minzoom": 8, "maxzoom": 11, "format": "png"
                        });
                        ("200 OK", json.to_string().into_bytes())
                    } else {
                        *counts.lock().unwrap().entry(target.clone()).or_insert(0) += 1;
                        let z: Vec<u32> = target
                            .trim_matches('/')
                            .split('/')
                            .map(|n| n.parse().unwrap())
                            .collect();
                        match tiles.tile(TileId::new(z[0] as u8, z[1], z[2])).unwrap() {
                            Some(data) => ("200 OK", data),
                            None => ("204 No Content", Vec::new()),
                        }
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(head.as_bytes()).unwrap();
                    stream.write_all(&body).unwrap();
                }
            });
        }
    });
    (format!("http://127.0.0.1:{port}/tiles.json"), requests)
}

/// A new, empty directory for this test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sparkle-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write an MBTiles file as an offline download would: WAL, with a `tiles` table.
fn create_mbtiles(path: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "CREATE TABLE metadata (name TEXT, value TEXT);
         CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);
         INSERT INTO metadata VALUES ('format', 'png');",
    )
    .unwrap();
    conn
}

fn copy_tile(conn: &rusqlite::Connection, from: &sparkle_view::mbtiles::MbTiles, id: TileId) {
    let data = from.tile(id).unwrap().unwrap();
    conn.execute(
        "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
        (id.z, id.x, (1u32 << id.z) - 1 - id.y, data),
    )
    .unwrap();
}

#[test]
fn a_local_mbtiles_is_read_before_the_server_once_it_exists() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sparkle/tests/fixtures/raster/raster.mbtiles");
    let source = sparkle_view::mbtiles::MbTiles::open(&fixture).unwrap();
    let (url, requests) = serve(&fixture);
    let dir = scratch("local-tiles");
    let style_path = dir.join("style.json");
    let style = json!({
        "version": 8,
        "sources": {"r": {"type": "raster", "tileSize": 256}},
        "layers": [{"id": "r", "type": "raster", "source": "r"}]
    });
    std::fs::write(&style_path, style.to_string()).unwrap();
    let local = dir.join("offline.mbtiles");
    let args = sparkle_view::args::parse(
        [
            "--style",
            style_path.to_str().unwrap(),
            "--cache",
            dir.join("cache").to_str().unwrap(),
            "--tiles",
            &format!("r={url}"),
            "--local-tiles",
            &format!("r={}", local.display()),
        ]
        .map(String::from),
    )
    .unwrap();
    let style = Arc::new(sparkle_view::setup::load_style(args.style.as_deref(), &args).unwrap());
    let (configs, _, _) = sparkle_view::setup::sources(&args, &style).unwrap();
    let (tx, rx) = mpsc::channel();
    let loader = TileLoader::spawn(
        LoaderConfig {
            style,
            sources: configs,
            images: None,
            fonts: None,
        },
        move |m| drop(tx.send(m)),
    )
    .unwrap();
    let load = |id: TileId| {
        loader.set_wanted([((0, id), Tier::Visible)]);
        loader.request((0, id), id.z);
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            TileMessage::Loaded(_, tile, ..) => assert_eq!(tile.id, id),
            _ => panic!("{id:?} didn't load"),
        }
    };
    let fetched = |id: TileId| {
        let path = format!("/{}/{}/{}", id.z, id.x, id.y);
        requests.lock().unwrap().get(&path).copied().unwrap_or(0)
    };
    let [a, b, c, d] =
        [(327, 714), (328, 714), (327, 715), (328, 715)].map(|(x, y)| TileId::new(11, x, y));

    // No file yet: from the server.
    load(a);
    assert_eq!(fetched(a), 1);

    // The download starts, with `b`: read from the file.
    let conn = create_mbtiles(&local);
    copy_tile(&conn, &source, b);
    load(b);
    assert_eq!(fetched(b), 0);

    // What it lacks still comes from the server.
    load(c);
    assert_eq!(fetched(c), 1);

    // And what it gains while open is read from it too.
    copy_tile(&conn, &source, d);
    load(d);
    assert_eq!(fetched(d), 0);
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_the_tilejson_a_tile_server_reads_only_its_local_mbtiles() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sparkle/tests/fixtures/raster/raster.mbtiles");
    let source = sparkle_view::mbtiles::MbTiles::open(&fixture).unwrap();
    // A port nothing listens on: offline, with nothing cached.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}/tiles.json");
    let dir = scratch("tilejson-fallback");
    let style_path = dir.join("style.json");
    let style = json!({
        "version": 8,
        "sources": {"r": {"type": "raster", "tileSize": 256}},
        "layers": [{"id": "r", "type": "raster", "source": "r"}]
    });
    std::fs::write(&style_path, style.to_string()).unwrap();
    let local = dir.join("offline.mbtiles");
    let args = sparkle_view::args::parse(
        [
            "--style",
            style_path.to_str().unwrap(),
            "--cache",
            dir.join("cache").to_str().unwrap(),
            "--tiles",
            &format!("r={url}"),
            "--local-tiles",
            &format!("r={}", local.display()),
        ]
        .map(String::from),
    )
    .unwrap();
    let style = Arc::new(sparkle_view::setup::load_style(args.style.as_deref(), &args).unwrap());

    // No local file either: nothing to describe the source with.
    assert!(sparkle_view::setup::sources(&args, &style).is_err());

    let conn = create_mbtiles(&local);
    let id = TileId::new(11, 328, 714);
    copy_tile(&conn, &source, id);
    let (configs, _, _) = sparkle_view::setup::sources(&args, &style).unwrap();
    assert!(configs[0].server.is_none());
    assert_eq!(configs[0].path, local);
    let (tx, rx) = mpsc::channel();
    let loader = TileLoader::spawn(
        LoaderConfig {
            style,
            sources: configs,
            images: None,
            fonts: None,
        },
        move |m| drop(tx.send(m)),
    )
    .unwrap();
    loader.set_wanted([((0, id), Tier::Visible)]);
    loader.request((0, id), id.z);
    match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
        TileMessage::Loaded(_, tile, ..) => assert_eq!(tile.id, id),
        _ => panic!("{id:?} didn't load"),
    }
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}
