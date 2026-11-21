//! Replacing a GeoJSON source's data at runtime, through the loader's workers.

use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::json;
use sparkle::geojson::GeoJsonIndex;
use sparkle::state::FeatureStates;
use sparkle::style::Source;
use sparkle::{ProcessedTile, Style};
use sparkle_view::camera::Camera;
use sparkle_view::loader::{LoaderConfig, TileLoader, TileMessage};
use sparkle_view::map::{MapState, TileUploader};
use sparkle_view::setup::{geojson_config, geojson_map_source};

/// Keeps nothing of tiles: queries use the features the map keeps itself.
struct Nothing;

impl TileUploader for Nothing {
    type Tile = ();
    fn upload(&mut self, _: &Style, _: ProcessedTile, _: &FeatureStates) {}
    fn tile_bytes(&self, _: &()) -> usize {
        0
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

struct Fixture {
    map: MapState<()>,
    messages: mpsc::Receiver<TileMessage>,
    camera: Camera,
}

/// The style of the [`Fixture`]: an empty GeoJSON source drawn as circles.
fn style_json() -> serde_json::Value {
    json!({
        "version": 8,
        "sources": {
            "selection": {
                "type": "geojson",
                "data": {"type": "FeatureCollection", "features": []}
            }
        },
        "layers": [
            {"id": "pin", "type": "circle", "source": "selection",
             "paint": {"circle-radius": 10}}
        ]
    })
}

impl Fixture {
    fn new() -> Self {
        let style = Arc::new(Style::from_json(&style_json()).unwrap());
        let Some(Source::GeoJson(source)) = style.source("selection") else {
            unreachable!()
        };
        let index = GeoJsonIndex::new(source, &source.data).unwrap();
        let map_source = geojson_map_source("selection", &index);
        let (tx, messages) = mpsc::channel();
        let loader = TileLoader::spawn(
            LoaderConfig {
                style: style.clone(),
                sources: vec![geojson_config("selection".into(), "".into(), index)],
                images: None,
                fonts: None,
            },
            move |m| {
                let _ = tx.send(m);
            },
        )
        .unwrap();
        let map = MapState::new(style, vec![map_source], loader, usize::MAX);
        let mut camera = Camera::new([800.0, 600.0], 1.0);
        camera.look_at(-122.33, 47.6, 14.0);
        Self {
            map,
            messages,
            camera,
        }
    }

    /// Draw frames until every tile has loaded.
    fn settle(&mut self) {
        for _ in 0..100 {
            let frame = self.map.frame(&self.camera, None);
            if frame.complete && frame.loading == 0 {
                return;
            }
            if let Ok(m) = self.messages.recv_timeout(Duration::from_secs(5)) {
                self.map.receive(&mut Nothing, m);
            }
            while let Ok(m) = self.messages.try_recv() {
                self.map.receive(&mut Nothing, m);
            }
        }
        panic!("never settled");
    }

    /// How many features are under the middle of the view.
    fn in_the_middle(&mut self) -> usize {
        self.settle();
        self.map.query_point(&self.camera, [400.0, 300.0]).len()
    }
}

#[test]
fn replacing_data_redraws_the_source() {
    let mut f = Fixture::new();
    assert_eq!(f.in_the_middle(), 0);

    let pin = json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "properties": {"name": "here"},
            "geometry": {"type": "Point", "coordinates": [-122.33, 47.6]}
        }]
    });
    f.map.set_geojson("selection", &pin).unwrap();
    assert_eq!(f.in_the_middle(), 1);

    f.map
        .set_geojson(
            "selection",
            &json!({"type": "FeatureCollection", "features": []}),
        )
        .unwrap();
    assert_eq!(f.in_the_middle(), 0);
}

#[test]
fn only_geojson_sources_take_data() {
    let mut f = Fixture::new();
    let empty = json!({"type": "FeatureCollection", "features": []});
    assert!(f.map.set_geojson("elsewhere", &empty).is_err());
    assert!(
        f.map
            .set_geojson("selection", &json!({"type": "Nonsense"}))
            .is_err()
    );
}

#[test]
fn replaced_data_is_drawn_until_the_new_is_in_and_never_after() {
    let mut f = Fixture::new();
    let pin = json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "properties": {},
            "geometry": {"type": "Point", "coordinates": [-122.33, 47.6]}
        }]
    });
    f.map.set_geojson("selection", &pin).unwrap();
    assert_eq!(f.in_the_middle(), 1);
    // Zoomed out and back, so tiles of other zooms hold the pin too.
    f.camera.look_at(-122.33, 47.6, 12.0);
    assert_eq!(f.in_the_middle(), 1);
    f.camera.look_at(-122.33, 47.6, 14.0);
    f.settle();

    f.map
        .set_geojson(
            "selection",
            &json!({"type": "FeatureCollection", "features": []}),
        )
        .unwrap();
    // Until the new tiles are in, the old ones are drawn, so the source doesn't blink.
    f.map.frame(&f.camera, None);
    assert_eq!(f.map.query_point(&f.camera, [400.0, 300.0]).len(), 1);
    assert!(f.map.swapping("selection"));
    f.settle();
    assert!(!f.map.swapping("selection"));
    // Once they are, the old ones are gone at every zoom, even standing in for tiles of
    // the new data still loading.
    for zoom in [14.0, 13.0, 12.0] {
        f.camera.look_at(-122.33, 47.6, zoom);
        f.map.frame(&f.camera, None);
        assert!(
            f.map.query_point(&f.camera, [400.0, 300.0]).is_empty(),
            "zoom {zoom}"
        );
    }
}

/// A pin in the middle of the view, numbered `n`.
fn numbered(n: u32) -> serde_json::Value {
    json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "properties": {"n": n},
            "geometry": {"type": "Point", "coordinates": [-122.33, 47.6]}
        }]
    })
}

fn empty() -> serde_json::Value {
    json!({"type": "FeatureCollection", "features": []})
}

impl Fixture {
    /// Take in the tiles cut so far, once the workers have nothing more to send.
    fn take_in_what_was_cut(&mut self) {
        while let Ok(m) = self.messages.recv_timeout(Duration::from_millis(100)) {
            self.map.receive(&mut Nothing, m);
        }
    }

    /// Draw a frame. Returns the number of the pin it shows, if it shows one.
    fn draw(&mut self) -> Option<u32> {
        self.map.frame(&self.camera, None);
        let found = self.map.query_point(&self.camera, [400.0, 300.0]);
        assert!(found.len() <= 1, "data of two sets drawn together");
        let n = found.first()?.properties.get("n")?;
        (0..1000).find(|&i| *n == maplibre_expr::Value::from_json(&json!(i)))
    }
}

/// The scrub marker's case: the app sets the data each frame before the frame is drawn,
/// and the tiles cut for one frame's data arrive after the next frame's data is set.
#[test]
fn data_set_every_frame_shows_the_data_of_the_frame_before() {
    let mut f = Fixture::new();
    f.settle();
    for n in 0..8 {
        f.map.set_geojson("selection", &numbered(n)).unwrap();
        f.take_in_what_was_cut();
        assert_eq!(f.draw(), n.checked_sub(1), "frame {n}");
    }
    // Once the changes stop, the last of them is drawn and nothing is left to cut.
    f.take_in_what_was_cut();
    assert_eq!(f.draw(), Some(7));
    f.settle();
    assert!(!f.map.swapping("selection"));
    assert_eq!(f.draw(), Some(7));
}

/// Data set many times while earlier data is cut: what was being cut is shown, then the
/// newest, and none of those between.
#[test]
fn of_data_set_while_older_data_is_cut_only_the_newest_follows() {
    let mut f = Fixture::new();
    f.settle();
    f.map.set_geojson("selection", &numbered(0)).unwrap();
    assert_eq!(f.draw(), None);
    for n in 1..=5 {
        f.map.set_geojson("selection", &numbered(n)).unwrap();
        // Frames drawn before the tiles arrive show what was there.
        assert_eq!(f.draw(), None);
    }
    let mut shown = Vec::new();
    for _ in 0..4 {
        f.take_in_what_was_cut();
        shown.push(f.draw());
    }
    assert_eq!(shown, [Some(0), Some(5), Some(5), Some(5)]);
    assert!(!f.map.swapping("selection"));
}

/// Data set again before any tile of the earlier data was asked for takes its place, as
/// when a source is set several times between frames.
#[test]
fn data_set_twice_before_a_frame_shows_only_the_second() {
    let mut f = Fixture::new();
    f.settle();
    f.map.set_geojson("selection", &numbered(0)).unwrap();
    f.map.set_geojson("selection", &numbered(1)).unwrap();
    assert_eq!(f.draw(), None);
    f.take_in_what_was_cut();
    assert_eq!(f.draw(), Some(1));
    assert!(!f.map.swapping("selection"));
}

/// Emptied while data set every frame is on its way in: it clears as soon as what was
/// being cut is in, and nothing set before the emptying is drawn after it.
#[test]
fn emptied_data_clears_and_nothing_older_comes_back() {
    let mut f = Fixture::new();
    f.settle();
    f.map.set_geojson("selection", &numbered(0)).unwrap();
    assert_eq!(f.draw(), None);
    f.map.set_geojson("selection", &numbered(1)).unwrap();
    f.take_in_what_was_cut();
    assert_eq!(f.draw(), Some(0));
    // Pin 1 is being cut, and pin 2 waits for it, when the source is emptied.
    f.map.set_geojson("selection", &numbered(2)).unwrap();
    f.map.set_geojson("selection", &empty()).unwrap();
    assert_eq!(f.draw(), Some(0));
    f.take_in_what_was_cut();
    assert_eq!(f.draw(), Some(1));
    for _ in 0..4 {
        f.take_in_what_was_cut();
        assert_eq!(f.draw(), None);
    }
    assert!(!f.map.swapping("selection"));
    // At other zooms too, where tiles of the pins might have lingered.
    for zoom in [13.0, 12.0, 14.0] {
        f.camera.look_at(-122.33, 47.6, zoom);
        f.settle();
        assert_eq!(f.draw(), None, "zoom {zoom}");
    }
}

/// Emptied with nothing else on its way in, the source clears in the frame after.
#[test]
fn emptied_data_clears_in_the_next_frame() {
    let mut f = Fixture::new();
    f.map.set_geojson("selection", &numbered(0)).unwrap();
    f.settle();
    assert_eq!(f.draw(), Some(0));
    f.map.set_geojson("selection", &empty()).unwrap();
    assert_eq!(f.draw(), Some(0));
    f.take_in_what_was_cut();
    assert_eq!(f.draw(), None);
    assert!(!f.map.swapping("selection"));
}

/// A style change that drops every tile loads the data that was waiting, not the older
/// data it was waiting for.
#[test]
fn a_reloading_style_change_loads_the_data_that_was_waiting() {
    let mut f = Fixture::new();
    f.settle();
    f.map.set_geojson("selection", &numbered(0)).unwrap();
    assert_eq!(f.draw(), None);
    f.map.set_geojson("selection", &numbered(1)).unwrap();
    let mut json = style_json();
    json["layers"].as_array_mut().unwrap().push(json!(
        {"id": "ring", "type": "circle", "source": "selection", "filter": ["==", "n", -1]}
    ));
    let next = Style::from_json(&json).unwrap();
    let diff = f.map.style.diff(&next);
    assert!(diff.reload_all);
    f.map.set_style(&mut Nothing, Arc::new(next), &diff);
    for _ in 0..4 {
        f.take_in_what_was_cut();
        let shown = f.draw();
        assert!(shown.is_none() || shown == Some(1), "{shown:?}");
    }
    assert_eq!(f.draw(), Some(1));
    assert!(!f.map.swapping("selection"));
}
