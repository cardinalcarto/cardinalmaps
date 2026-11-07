//! A map on screen and what input does to it: a port of `sparkle_harness`'s window, with
//! Flutter's events (forwarded by the widget) in place of winit's.
//!
//! Positions come in logical pixels, as Flutter has them, and are turned into the physical
//! pixels the camera works in.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Instant, SystemTime};

use anyhow::{Result, bail};
use maplibre_expr::Value;
use sparkle::Style;
use sparkle::style::StyleDiff;
use sparkle::transition::Transitions;
use sparkle_view::args::Args;
use sparkle_view::camera::{Camera, Padding, Placement, lng_lat_to_world};
use sparkle_view::loader::{LoaderConfig, TileLoader, TileMessage};
use sparkle_view::map::MapState;
use sparkle_view::motion::Motion;
use sparkle_view::setup::{
    Marked, initial_camera, initial_feature_state, load_style, mark_feature, sources, sprites,
};

use crate::gpu::{GpuInfo, Stream};
use crate::render::{GpuTile, Renderer};

/// Bytes of new buffer and texture contents (mostly tiles) a frame takes in at most:
/// tiles that arrive beyond it wait for the next frames, so that a burst of them is spread
/// over several frames instead of stalling one.
const UPLOAD_BUDGET: usize = 2 << 20;

/// How far from a label a click still lands on it, in logical pixels, on top of what
/// the label covers: icons are smaller than a finger.
const LABEL_TOUCH_TOLERANCE: f32 = 12.0;

/// A feature of a label that was clicked.
pub struct ClickedFeature {
    pub source: String,
    pub source_layer: Option<String>,
    /// The tile it's in.
    pub tile: Placement,
    pub id: u64,
    /// Where the label is anchored.
    pub lng: f64,
    pub lat: f64,
    pub properties: BTreeMap<String, Value>,
}

/// Called from loader threads whenever a tile is ready, to have a frame drawn.
pub type Wake = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// What a mouse drag does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Pan,
    Rotate,
}

/// Which pointer button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
    Other,
}

/// One drawn frame.
pub struct Frame {
    pub stream: Stream,
    /// Draw another frame soon: something is moving or fading, or tiles are waiting.
    pub animating: bool,
    /// Every visible tile has loaded, so nothing is a stand-in.
    pub complete: bool,
    pub tiles_drawn: usize,
    pub loading: usize,
    pub zoom: f64,
    pub bearing: f64,
    pub pitch: f64,
}

/// The most a subject zooms out for padding that leaves little of the view, in zoom
/// levels below how it shows with none.
const SUBJECT_ZOOM_GIVE: f64 = 3.0;

/// Bounds for the camera to keep in view, as [`MapView::set_subject`] takes them.
#[derive(Clone, Copy)]
struct Subject {
    /// `[west, south, east, north]` in degrees.
    bounds: [f64; 4],
    /// The closest the camera zooms in on them.
    max_zoom: f64,
    /// Logical pixels kept clear around them.
    margin: f64,
}

pub struct MapView {
    args: Args,
    /// The style tiles are processed with.
    style: Arc<Style>,
    /// The style as drawn, with paint transitions applied to `style`.
    shown: Style,
    transitions: Transitions,
    /// Transitions are timed in milliseconds since this.
    clock: Instant,
    style_mtime: Option<SystemTime>,
    map: MapState<GpuTile>,
    renderer: Renderer,
    messages: mpsc::Receiver<TileMessage>,
    camera: Camera,
    /// Wheel zooms and flings still in progress.
    motion: Motion,
    /// What the app covers at the edges of the view (a sheet, say): the camera centers
    /// things in the rest, though the map is drawn behind it too.
    padding: Padding,
    /// What the camera keeps in view, until input moves the map.
    subject: Option<Subject>,
    /// The pointer, in physical pixels.
    cursor: [f64; 2],
    drag: Option<Drag>,
    /// Where the primary button went down, to tell clicks from drags.
    press: Option<[f64; 2]>,
    /// The labels the last click landed on, until they're taken.
    clicked: Vec<ClickedFeature>,
    hovered: Option<Marked>,
    selected: Option<Marked>,
}

impl MapView {
    /// A map set up from command line `arguments` (as `sparkle_harness` takes them),
    /// `size` physical pixels with `pixel_ratio` of them per logical pixel.
    pub fn new(
        arguments: Vec<String>,
        info: GpuInfo,
        size: [u32; 2],
        pixel_ratio: f64,
        wake: Wake,
    ) -> Result<Self> {
        let args = sparkle_view::args::parse(arguments)?;
        if args.headless {
            bail!("--headless is for sparkle_harness; the Flutter map always has a window");
        }
        let style = Arc::new(load_style(args.style.as_deref(), &args)?);
        let (configs, map_sources, metadata) = sources(&args, &style)?;
        if let Some(fonts) = &args.fonts {
            log::info!("fonts: {}", fonts.display());
        }
        let pixel_ratio = args.pixel_ratio.unwrap_or(pixel_ratio);
        let mut renderer = Renderer::new(info, size);
        // A screenshot shows transitions' end state, as the harness's headless one does.
        renderer.animate = args.screenshot.is_none();
        let atlas = sprites(&args, &style, pixel_ratio)?.map(Arc::new);
        if let Some(atlas) = &atlas {
            renderer.set_images(atlas);
        }
        let (tx, messages) = mpsc::channel();
        let loader = TileLoader::spawn(
            LoaderConfig {
                style: style.clone(),
                sources: configs,
                images: atlas,
                fonts: args.fonts.clone(),
            },
            move |m| {
                // Sending only fails once the map is gone, when nobody needs the tile.
                if tx.send(m).is_ok()
                    && let Some(wake) = &*wake.lock().expect("never poisoned")
                {
                    wake();
                }
            },
        )?;
        let mut map = MapState::new(style.clone(), map_sources, loader, args.tile_memory);
        let mut camera = Camera::new(size.map(f64::from), pixel_ratio);
        initial_camera(&mut camera, &args, &style, metadata.as_ref());
        initial_feature_state(&mut map, &mut renderer, &args);
        Ok(Self {
            style_mtime: args.style.as_deref().and_then(modified),
            shown: (*style).clone(),
            args,
            style,
            transitions: Transitions::new(),
            clock: Instant::now(),
            map,
            renderer,
            messages,
            camera,
            motion: Motion::default(),
            padding: Padding::default(),
            subject: None,
            cursor: [0.0; 2],
            drag: None,
            press: None,
            clicked: Vec::new(),
            hovered: None,
            selected: None,
        })
    }

    fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64() * 1000.0
    }

    /// Where `--screenshot` wants the first complete frame saved, if anywhere.
    pub fn screenshot_path(&self) -> Option<&Path> {
        self.args.screenshot.as_deref()
    }

    /// The window is now `size` physical pixels, `pixel_ratio` of them per logical one.
    pub fn resize(&mut self, size: [u32; 2], pixel_ratio: f64) {
        let middle = self.camera.padded_center(self.padding);
        self.renderer.resize(size);
        self.camera.viewport = size.map(f64::from);
        if self.args.pixel_ratio.is_none() {
            self.camera.pixel_ratio = pixel_ratio;
        }
        self.follow_padding(middle);
    }

    /// Draw a frame, taking in the tiles that arrived since the last one, as many as
    /// [`UPLOAD_BUDGET`] allows. `recycled` is the stream of an earlier frame that the Dart
    /// side is done with, to reuse.
    pub fn frame(&mut self, recycled: Stream) -> Frame {
        let mut backlog = false;
        loop {
            if self.renderer.gpu().uploaded() >= UPLOAD_BUDGET {
                backlog = true;
                break;
            }
            let Ok(message) = self.messages.try_recv() else {
                break;
            };
            self.map.receive(&mut self.renderer, message);
        }
        let now = self.now();
        let moving = self.motion.step(&mut self.camera, Instant::now());
        self.map.update_camera(&mut self.camera);
        let transitioning =
            self.transitions
                .apply(&self.map.style, &mut self.shown, now, self.camera.zoom);
        let frame = self.map.frame(&self.camera);
        let draws = self.map.draws(&frame);
        self.renderer
            .render(&self.shown, &self.camera, &draws, self.map.terrain());
        Frame {
            stream: self.renderer.gpu().take(recycled),
            // Tiles left waiting are taken in by the next frames.
            animating: moving || transitioning || self.renderer.animating() || backlog,
            complete: frame.complete,
            tiles_drawn: frame.tiles_drawn(),
            loading: frame.loading,
            zoom: self.camera.zoom,
            bearing: self.camera.bearing,
            pitch: self.camera.pitch,
        }
    }

    /// Reload the `--style` file if it changed, transitioning to it. Returns whether it
    /// did, so a frame should be drawn.
    pub fn poll_style(&mut self) -> bool {
        let Some(path) = self.args.style.clone() else {
            return false;
        };
        let mtime = modified(&path);
        if mtime == self.style_mtime {
            return false;
        }
        self.style_mtime = mtime;
        let new = match load_style(Some(&path), &self.args) {
            Ok(style) => style,
            Err(err) => {
                log::warn!("not reloading the style: {err:#}");
                return false;
            }
        };
        let diff: StyleDiff = self.map.style.diff(&new);
        log::info!(
            "reloaded {}: {}",
            path.display(),
            if diff.reload_all {
                "everything changed".to_owned()
            } else {
                format!(
                    "{} layers reprocessed, {} repainted, {} restyled",
                    diff.reprocess.len(),
                    diff.repaint.len(),
                    diff.restyle.len()
                )
            }
        );
        let now = self.now();
        self.transitions.start(&self.shown, &new, now);
        let new = Arc::new(new);
        self.map.set_style(&mut self.renderer, new.clone(), &diff);
        self.shown = (*new).clone();
        self.style = new;
        true
    }

    /// The features of the labels shown under the pointer, nearest first. Labels without
    /// a feature ID aren't included: the ID is how the feature is found.
    fn features_at_cursor(&self) -> Vec<ClickedFeature> {
        let ratio = self.camera.pixel_ratio;
        let point = self.cursor.map(|c| (c / ratio) as f32);
        self.renderer
            .labels_at(point, LABEL_TOUCH_TOLERANCE)
            .into_iter()
            .filter_map(|label| {
                // Tiles use zero for features without an ID.
                let id = label.feature_id.filter(|&id| id != 0)?;
                let key = label.key;
                let placement = Placement {
                    z: key.z,
                    x: key.x,
                    y: key.y,
                };
                let (source, source_layer) = self.map.layer_source(key.layer)?;
                let properties = self.map.tile_feature(source, placement, source_layer, id)?;
                let [x, y] = label.world;
                let [lng, lat] = sparkle::projection::world_to_lng_lat([x.rem_euclid(1.0), y])
                    .map(f64::to_degrees);
                Some(ClickedFeature {
                    source: source.to_owned(),
                    source_layer: source_layer.map(str::to_owned),
                    tile: placement,
                    id,
                    lng,
                    lat,
                    properties,
                })
            })
            .collect()
    }

    /// The properties of the feature with `id` in `source_layer` of `source`'s tile at
    /// `tile`, as of now.
    pub fn tile_feature(
        &self,
        source: &str,
        tile: Placement,
        source_layer: Option<&str>,
        id: u64,
    ) -> Option<BTreeMap<String, Value>> {
        self.map.tile_feature(source, tile, source_layer, id)
    }

    /// The features of the labels the last click landed on, nearest first, once.
    pub fn take_clicked(&mut self) -> Vec<ClickedFeature> {
        std::mem::take(&mut self.clicked)
    }

    /// Hover the feature under the pointer, or with `click`, toggle its selection.
    /// Returns whether anything changed on screen.
    fn mark(&mut self, click: bool) -> bool {
        if click {
            self.clicked = self.features_at_cursor();
        }
        let (key, slot) = if click {
            ("selected", &mut self.selected)
        } else {
            ("hover", &mut self.hovered)
        };
        let (marked, changed) = mark_feature(
            &mut self.map,
            &mut self.renderer,
            &self.camera,
            self.cursor,
            key,
            slot.as_ref(),
            click,
        );
        *slot = marked;
        changed
    }

    fn physical(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let ratio = self.camera.pixel_ratio;
        [x * ratio, y * ratio]
    }

    /// The pointer moved to `position`. Returns whether a frame should be drawn.
    pub fn pointer_move(&mut self, position: [f64; 2]) -> bool {
        let p = self.physical(position);
        let from = std::mem::replace(&mut self.cursor, p);
        let camera = &mut self.camera;
        match self.drag {
            Some(Drag::Pan) => {
                camera.drag(from, p);
                self.motion.track(p, Instant::now());
            }
            Some(Drag::Rotate) => {
                let ratio = camera.pixel_ratio;
                camera.set_bearing(camera.bearing - (p[0] - from[0]) / ratio * 0.5);
                camera.set_pitch(camera.pitch - (p[1] - from[1]) / ratio * 0.5);
            }
            None => return self.mark(false),
        }
        true
    }

    /// A button went down at `position`; `rotate` turns a primary drag into rotating (as
    /// with ctrl held). Returns whether it stopped the camera moving, in which case it's
    /// part of panning and can't be a click, and a frame should be drawn.
    pub fn pointer_down(&mut self, position: [f64; 2], button: Button, rotate: bool) -> bool {
        self.subject = None;
        self.cursor = self.physical(position);
        if button == Button::Primary {
            self.press = Some(self.cursor);
        }
        let mut stopped = false;
        self.drag = match button {
            Button::Secondary => Some(Drag::Rotate),
            Button::Primary if rotate => Some(Drag::Rotate),
            Button::Primary => {
                stopped = self.motion.grab(self.cursor, Instant::now());
                if stopped {
                    self.press = None;
                }
                Some(Drag::Pan)
            }
            Button::Other => self.drag,
        };
        stopped
    }

    /// A button went up at `position`; with `click`, a release where it went down is a
    /// click. Returns whether a frame should be drawn.
    pub fn pointer_up(&mut self, position: [f64; 2], button: Button, click: bool) -> bool {
        self.cursor = self.physical(position);
        if button == Button::Primary
            && let Some(p) = self.press.take()
            && click
            && (p[0] - self.cursor[0]).hypot(p[1] - self.cursor[1]) < 4.0
        {
            self.drag = None;
            return self.mark(true);
        }
        if self.drag == Some(Drag::Pan) {
            self.motion.release(&self.camera, Instant::now());
        }
        self.drag = None;
        true
    }

    /// A click at `position` made separately from its release, as a tap is once it can't
    /// be a double tap: toggle the selection there. Returns whether a frame should be drawn.
    pub fn click(&mut self, position: [f64; 2]) -> bool {
        self.cursor = self.physical(position);
        self.mark(true)
    }

    /// A mouse wheel turned by `delta` logical pixels at `position`: notches glide, as in
    /// the harness.
    pub fn scroll(&mut self, position: [f64; 2], delta: f64) {
        self.subject = None;
        self.cursor = self.physical(position);
        // Flutter reports a notch as about 50 logical pixels; half a zoom level per notch.
        let levels = -delta / 100.0;
        self.motion
            .zoom(&self.camera, levels, self.cursor, Instant::now());
    }

    /// A touchpad or pinch gesture at `position` moved by `pan` logical pixels, scaled by
    /// `scale` and turned clockwise by `rotation` degrees since its last update.
    pub fn pan_zoom(&mut self, position: [f64; 2], pan: [f64; 2], scale: f64, rotation: f64) {
        self.subject = None;
        let p = self.physical(position);
        let ratio = self.camera.pixel_ratio;
        let to = [p[0] + pan[0] * ratio, p[1] + pan[1] * ratio];
        self.camera.drag(p, to);
        if scale > 0.0 && scale != 1.0 {
            self.camera.zoom_around(scale.log2(), to);
        }
        if rotation != 0.0 {
            // Turning the map clockwise turns the view to the west.
            self.camera.rotate_around(-rotation, to);
        }
        self.cursor = to;
    }

    /// Two fingers dragged up or down together by `dy` logical pixels: up tilts further.
    pub fn tilt(&mut self, dy: f64) {
        self.subject = None;
        let camera = &mut self.camera;
        camera.set_pitch(camera.pitch - dy * 0.5);
    }

    /// A double tap at `position`: glide in a level around it.
    pub fn double_tap_zoom(&mut self, position: [f64; 2]) {
        self.subject = None;
        self.cursor = self.physical(position);
        self.motion
            .zoom(&self.camera, 1.0, self.cursor, Instant::now());
    }

    /// A finger held down after a double tap at `position` dragged by `dy` logical pixels:
    /// down zooms in, up zooms out.
    pub fn quick_zoom(&mut self, position: [f64; 2], dy: f64) {
        self.subject = None;
        self.cursor = self.physical(position);
        self.camera.zoom_around(dy / 100.0, self.cursor);
    }

    /// A key went down: `key` is the character, or the name of a named key such as
    /// `ArrowLeft`. Returns whether a frame should be drawn.
    pub fn key(&mut self, key: &str, shift: bool) -> bool {
        let camera = &mut self.camera;
        let center = [camera.viewport[0] / 2.0, camera.viewport[1] / 2.0];
        let step = 100.0 * camera.pixel_ratio;
        match key {
            "+" | "=" => self.motion.zoom(camera, 1.0, center, Instant::now()),
            "-" => self.motion.zoom(camera, -1.0, center, Instant::now()),
            "n" | "N" => {
                camera.set_bearing(0.0);
                camera.set_pitch(0.0);
            }
            "ArrowLeft" if shift => camera.set_bearing(camera.bearing - 15.0),
            "ArrowRight" if shift => camera.set_bearing(camera.bearing + 15.0),
            "ArrowUp" if shift => camera.set_pitch(camera.pitch + 10.0),
            "ArrowDown" if shift => camera.set_pitch(camera.pitch - 10.0),
            "ArrowLeft" => camera.pan(step, 0.0),
            "ArrowRight" => camera.pan(-step, 0.0),
            "ArrowUp" => camera.pan(0.0, step),
            "ArrowDown" => camera.pan(0.0, -step),
            _ => return false,
        }
        self.subject = None;
        true
    }

    /// Replace the data of the style's GeoJSON source `source` with `geojson`, a GeoJSON
    /// object as text.
    pub fn set_geojson(&mut self, source: &str, geojson: &str) -> Result<()> {
        let data: serde_json::Value = serde_json::from_str(geojson)?;
        self.map.set_geojson(source, &data)
    }

    /// The camera: the longitude and latitude in the middle of the view less the
    /// padding, zoom, bearing and pitch.
    pub fn camera(&self) -> [f64; 5] {
        let [lng, lat] = to_lng_lat(self.camera.padded_center(self.padding));
        let c = &self.camera;
        [lng, lat, c.zoom, c.bearing, c.pitch]
    }

    /// The longitude and latitude of the ground at `position` (logical pixels), if it
    /// shows the ground.
    pub fn screen_to_lng_lat(&self, position: [f64; 2]) -> Option<[f64; 2]> {
        let world = self.camera.screen_to_world(self.physical(position))?;
        Some(to_lng_lat([world[0].rem_euclid(1.0), world[1]]))
    }

    /// Cover `padding` at the edges of the view. With a subject, the camera keeps it in
    /// the rest of the view, steering any flight there; otherwise the middle of the rest
    /// stays where it is on the map, as does the zoom. Returns whether a frame should be
    /// drawn.
    pub fn set_padding(&mut self, padding: Padding) -> bool {
        if padding == self.padding {
            return false;
        }
        let middle = self.camera.padded_center(self.padding);
        self.padding = padding;
        self.follow_padding(middle);
        true
    }

    /// After the padding or the viewport changed: keep the subject in view, or else
    /// `middle` (world coordinates) in the middle of the view less the padding.
    fn follow_padding(&mut self, middle: [f64; 2]) {
        if let Some(subject) = self.subject {
            let (center, zoom) = self.subject_view(subject);
            if !self.motion.retarget(center, zoom) {
                self.camera.center = center;
                self.camera.zoom = zoom;
            }
        } else if !self.motion.flying() {
            self.camera.center = self
                .camera
                .center_for(middle, self.camera.zoom, self.padding);
        }
    }

    /// Fly to `[west, south, east, north]` and keep it in view as the padding changes,
    /// zoomed in no closer than `max_zoom`, with `margin` logical pixels clear around it,
    /// until input moves the map or [`MapView::clear_subject`].
    pub fn set_subject(&mut self, bounds: [f64; 4], max_zoom: f64, margin: f64) {
        let subject = Subject {
            bounds,
            max_zoom,
            margin,
        };
        self.subject = Some(subject);
        let (center, zoom) = self.subject_view(subject);
        self.motion.fly(&self.camera, center, zoom, Instant::now());
    }

    /// Stop keeping the subject in view, leaving the camera where it is.
    pub fn clear_subject(&mut self) {
        self.subject = None;
    }

    /// Whether the camera is keeping a subject in view: one was set, and neither input
    /// nor [`MapView::clear_subject`] has let go of it since.
    pub fn has_subject(&self) -> bool {
        self.subject.is_some()
    }

    /// The camera's center and zoom for showing `subject` with the current padding.
    fn subject_view(&self, subject: Subject) -> ([f64; 2], f64) {
        let m = subject.margin;
        let p = self.padding;
        let fit = Padding {
            top: p.top + m,
            right: p.right + m,
            bottom: p.bottom + m,
            left: p.left + m,
        };
        let (middle, zoom) = self.camera.camera_for_bounds(subject.bounds, fit);
        let zoom = zoom.min(subject.max_zoom);
        // However little of the view the padding leaves (none, under a full screen
        // sheet), don't zoom out more than SUBJECT_ZOOM_GIVE below how it shows with no
        // padding: past that it isn't worth seeing all of. Easing towards the limit, as a
        // stiffening spring would, so the zoom never stops short.
        let all = Padding {
            top: m,
            right: m,
            bottom: m,
            left: m,
        };
        let open = self
            .camera
            .camera_for_bounds(subject.bounds, all)
            .1
            .min(subject.max_zoom);
        let zoom = if zoom < open {
            open - SUBJECT_ZOOM_GIVE * ((open - zoom) / SUBJECT_ZOOM_GIVE).tanh()
        } else {
            zoom
        };
        (self.camera.center_for(middle, zoom, self.padding), zoom)
    }

    /// Fly to show `lng, lat` at `zoom` in the middle of the view less the padding,
    /// dropping any subject.
    pub fn fly_to(&mut self, lng: f64, lat: f64, zoom: f64) {
        self.subject = None;
        let target = lng_lat_to_world(lng, lat);
        let center = self.camera.center_for(target, zoom, self.padding);
        self.motion.fly(&self.camera, center, zoom, Instant::now());
    }

    /// With `--hover x,y`: hover that point (physical pixels) as the pointer would.
    pub fn hover_argument(&mut self) -> bool {
        let Some(point) = self.args.hover else {
            return false;
        };
        self.cursor = point;
        self.mark(false)
    }
}

/// World coordinates as longitude and latitude in degrees.
fn to_lng_lat(world: [f64; 2]) -> [f64; 2] {
    sparkle::projection::world_to_lng_lat(world).map(f64::to_degrees)
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A map of nothing but an empty GeoJSON source, 800 by 600 at 1 pixel per logical
    /// pixel, looking at Seattle at zoom 12.
    fn view() -> MapView {
        // One per view: tests run in parallel.
        static VIEWS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sparkle-app-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let style = dir.join("style.json");
        std::fs::write(
            &style,
            r#"{"version": 8, "sources": {"s": {"type": "geojson",
                "data": {"type": "FeatureCollection", "features": []}}},
                "layers": [{"id": "c", "type": "circle", "source": "s"}]}"#,
        )
        .unwrap();
        let args = [
            "--style",
            style.to_str().unwrap(),
            "--view",
            "-122.33,47.6,12",
        ]
        .map(String::from)
        .to_vec();
        let info = GpuInfo {
            uniform_alignment: 256,
            sample_count: 1,
            half_float: false,
        };
        MapView::new(args, info, [800, 600], 1.0, Arc::new(Mutex::new(None))).unwrap()
    }

    fn sheet(bottom: f64) -> Padding {
        Padding {
            bottom,
            ..Padding::default()
        }
    }

    /// Draw frames until nothing moves.
    fn settle(view: &mut MapView) {
        for _ in 0..400 {
            if !view.frame(Stream::default()).animating {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("never settled");
    }

    fn close(a: [f64; 5], b: [f64; 5]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6)
    }

    #[test]
    fn padding_alone_keeps_the_middle_and_zoom() {
        let mut v = view();
        let before = v.camera();
        assert!(v.set_padding(sheet(300.0)));
        assert!(close(v.camera(), before), "{:?} {before:?}", v.camera());
        // The middle of the uncovered top half shows it.
        let [lng, lat] = v.screen_to_lng_lat([400.0, 150.0]).unwrap();
        assert!((lng - before[0]).abs() < 1e-6 && (lat - before[1]).abs() < 1e-6);
        assert!(!v.set_padding(sheet(300.0)), "nothing changed");
    }

    #[test]
    fn a_subject_stays_in_view_as_the_padding_changes() {
        let mut v = view();
        v.set_padding(sheet(300.0));
        // A point: shown at the most zoom allowed.
        v.set_subject([-122.3493, 47.6205, -122.3493, 47.6205], 14.0, 48.0);
        settle(&mut v);
        let [lng, lat, zoom, ..] = v.camera();
        assert!((lng + 122.3493).abs() < 1e-6 && (lat - 47.6205).abs() < 1e-6);
        assert!((zoom - 14.0).abs() < 1e-9);

        // The sheet pulled up: still in the middle of what's left.
        v.set_padding(sheet(450.0));
        let [lng, lat, ..] = v.camera();
        assert!((lng + 122.3493).abs() < 1e-6 && (lat - 47.6205).abs() < 1e-6);

        // Bounds too big for zoom 14 in what's left zoom out to fit it, less the margin.
        v.set_subject([-122.44, 47.50, -122.24, 47.73], 14.0, 48.0);
        settle(&mut v);
        let zoom = v.camera()[2];
        assert!(zoom < 12.0, "{zoom}");
        v.set_padding(sheet(300.0));
        assert!(v.camera()[2] > zoom, "more room, so closer");
    }

    #[test]
    fn a_subject_steers_its_flight_as_the_padding_changes() {
        let mut v = view();
        v.set_padding(sheet(500.0));
        v.set_subject([-122.3493, 47.6205, -122.3493, 47.6205], 14.0, 48.0);
        v.frame(Stream::default());
        // The sheet settles lower while the camera is on its way.
        v.set_padding(sheet(120.0));
        settle(&mut v);
        let [lng, lat, zoom, ..] = v.camera();
        assert!((lng + 122.3493).abs() < 1e-6 && (lat - 47.6205).abs() < 1e-6);
        assert!((zoom - 14.0).abs() < 1e-9);
    }

    #[test]
    fn a_subject_zooms_out_only_so_far_however_little_shows() {
        let mut v = view();
        let seattle = [-122.44, 47.50, -122.24, 47.73];
        v.set_subject(seattle, 14.0, 48.0);
        settle(&mut v);
        let open = v.camera()[2];
        let mut last = open;
        // Pulling a sheet up the whole height: the zoom eases out, never past the limit.
        for bottom in (0..=600).step_by(20) {
            v.set_padding(sheet(f64::from(bottom)));
            let zoom = v.camera()[2];
            assert!(zoom <= last + 1e-9, "only ever out");
            assert!(zoom > open - SUBJECT_ZOOM_GIVE, "{bottom}: {zoom}");
            // While there's a map to see, at least: with all but a few pixels covered,
            // the zoom fit to them falls off fast.
            if bottom <= 480 {
                assert!(last - zoom < 0.5, "no jumps: {last} to {zoom}");
            }
            last = zoom;
        }
        assert!(
            last < open - 0.8 * SUBJECT_ZOOM_GIVE,
            "most of the way by a full sheet: {last}"
        );
    }

    #[test]
    fn touching_the_map_lets_go_of_the_subject() {
        let mut v = view();
        v.set_subject([-122.3493, 47.6205, -122.3493, 47.6205], 14.0, 48.0);
        settle(&mut v);
        v.pointer_down([400.0, 300.0], Button::Primary, false);
        v.pointer_up([400.0, 300.0], Button::Primary, false);
        assert!(!v.has_subject());
        let before = v.camera();
        v.set_padding(sheet(300.0));
        assert!(
            close(v.camera(), before),
            "the middle stays, not the subject"
        );
    }
}
