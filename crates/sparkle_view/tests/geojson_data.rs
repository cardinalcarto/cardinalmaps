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

impl Fixture {
    fn new() -> Self {
        let style = Arc::new(
            Style::from_json(&json!({
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
            }))
            .unwrap(),
        );
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
