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
use sparkle::sprite::{ImageAtlas, SpriteImage};
use sparkle::style::StyleDiff;
use sparkle::transition::Transitions;
use sparkle_view::args::Args;
use sparkle_view::camera::{Camera, Framing, Padding, Placement, lng_lat_to_world};
use sparkle_view::loader::{LoaderConfig, TileLoader, TileMessage};
use sparkle_view::map::MapState;
use sparkle_view::motion::{Curve, Motion};
use sparkle_view::puck::{Pose, Puck, PuckStyle};
use sparkle_view::setup::{
    Marked, initial_camera, initial_feature_state, load_style, mark_feature, sources, sprites,
};

use crate::gpu::{GpuInfo, Stream};
use crate::render::puck::PuckDraw;
use crate::render::{GpuTile, Renderer};

/// Bytes of new buffer and texture contents (mostly tiles) a frame takes in at most:
/// tiles that arrive beyond it wait for the next frames, so that a burst of them is spread
/// over several frames instead of stalling one.
const UPLOAD_BUDGET: usize = 2 << 20;

/// How far, in physical pixels, a press can move and still be a click.
const CLICK_SLOP: f64 = 4.0;

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
    /// The camera is following the puck: see [`MapView::follow`].
    pub following: bool,
}

/// The camera following the puck: see [`MapView::follow`].
#[derive(Clone, Copy)]
struct Follow {
    zoom: f64,
    pitch: f64,
    /// Fixed, or `None` to turn with the puck's course.
    bearing: Option<f64>,
    /// Kept clear on top of the view's padding, to put the puck elsewhere than the
    /// middle of what that leaves.
    padding: Padding,
    /// Easing in from where the camera was, toward the moving puck: the framing it
    /// started from, when, and for how many seconds.
    entry: Option<(Framing, Instant, f64)>,
}

/// The most a subject zooms out for padding that leaves little of the view, in zoom
/// levels below how it shows with none.
const SUBJECT_ZOOM_GIVE: f64 = 2.0;

/// Bounds for the camera to keep in view, as [`MapView::set_subject`] takes them.
#[derive(Clone, Copy)]
struct Subject {
    /// `[west, south, east, north]` in degrees.
    bounds: [f64; 4],
    /// The closest the camera zooms in on them.
    max_zoom: f64,
    /// Logical pixels kept clear around them.
    margin: f64,
    /// More kept clear, edge by edge, on top of the view's padding and the margin: what
    /// the app draws over the map there, such as the navigation screen's banner, which
    /// the view's padding doesn't know about.
    clear: Padding,
    /// The bearing and pitch to show them at, or `None` for the camera's own: it turns
    /// and tilts there on its way, and fits them as they'll be seen once there.
    bearing: Option<f64>,
    pitch: Option<f64>,
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
    /// Where the navigation puck is, once it has been put anywhere.
    puck: Option<Puck>,
    puck_shown: bool,
    puck_style: PuckStyle,
    /// The camera follows the puck, until input moves the map.
    follow: Option<Follow>,
    /// The pointer, in physical pixels.
    cursor: [f64; 2],
    drag: Option<Drag>,
    /// Where the primary button went down, to tell clicks from drags.
    press: Option<[f64; 2]>,
    /// Where the last button went down, any button: a drag from there that goes no
    /// further than a click could leaves the camera following.
    grabbed: [f64; 2],
    /// The labels the last click landed on, until they're taken.
    clicked: Vec<ClickedFeature>,
    /// The sprite images tiles are laid out against.
    images: Option<Arc<ImageAtlas>>,
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
        let images = atlas.clone();
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
        let puck = args.puck.map(|[lng, lat, bearing]| {
            let pose = Pose {
                world: lng_lat_to_world(lng, lat),
                bearing,
            };
            Puck::new(pose, Instant::now())
        });
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
            puck_shown: puck.is_some(),
            puck,
            puck_style: PuckStyle::default(),
            follow: None,
            cursor: [0.0; 2],
            drag: None,
            press: None,
            grabbed: [0.0; 2],
            clicked: Vec::new(),
            images,
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
        let middle = self.camera.framing(self.padding).target;
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
        // One moment for the camera and the puck, so a camera following the puck shows it
        // where it is drawn.
        let instant = Instant::now();
        let moving = self.motion.step(&mut self.camera, instant);
        let following = self.place_following(instant);
        self.map.update_camera(&mut self.camera);
        let transitioning =
            self.transitions
                .apply(&self.map.style, &mut self.shown, now, self.camera.zoom);
        let end = self.motion.end(&self.camera);
        let frame = self.map.frame(&self.camera, end.as_ref());
        let draws = self.map.draws(&frame);
        let puck = self.puck.as_ref().filter(|_| self.puck_shown).map(|puck| {
            puck_draw(
                puck,
                &self.puck_style,
                &self.shown,
                &self.map,
                &self.camera,
                instant,
            )
        });
        self.renderer.render(
            &self.shown,
            &self.camera,
            &draws,
            self.map.terrain(),
            puck.as_ref(),
        );
        let gliding = self.puck_shown && self.puck.as_ref().is_some_and(|p| p.moving(instant));
        Frame {
            stream: self.renderer.gpu().take(recycled),
            // Tiles left waiting are taken in by the next frames.
            animating: moving
                || following
                || gliding
                || transitioning
                || self.renderer.animating()
                || backlog,
            complete: frame.complete,
            tiles_drawn: frame.tiles_drawn(),
            loading: frame.loading,
            zoom: self.camera.zoom,
            bearing: self.camera.bearing,
            pitch: self.camera.pitch,
            following: self.follow.is_some(),
        }
    }

    /// While following the puck, put the camera where that has it at `now`. Returns
    /// whether it moves on from there: the puck is gliding, or the camera easing in.
    fn place_following(&mut self, now: Instant) -> bool {
        let (Some(follow), Some(puck)) = (&mut self.follow, &self.puck) else {
            return false;
        };
        let pose = puck.at(now);
        let target = Framing {
            target: pose.world,
            zoom: follow.zoom,
            bearing: follow.bearing.unwrap_or(pose.bearing),
            pitch: follow.pitch,
        };
        let mut framing = target;
        if let Some((from, start, duration)) = follow.entry {
            let t = now.saturating_duration_since(start).as_secs_f64() / duration;
            if t < 1.0 {
                framing = from.toward(&target, Curve::Ease.at(t));
            } else {
                follow.entry = None;
            }
        }
        self.camera.show(&framing, self.padding + follow.padding);
        follow.entry.is_some() || puck.moving(now)
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
    ///
    /// A drag ends following the puck once it has moved further than a click could, and
    /// not before: a press is a tap until then, and tapping a label or a place while
    /// navigating mustn't take the camera off the puck. The drag then starts from where
    /// the press went down, so the map doesn't lag behind the finger.
    pub fn pointer_move(&mut self, position: [f64; 2]) -> bool {
        let p = self.physical(position);
        let mut from = std::mem::replace(&mut self.cursor, p);
        if self.drag.is_some() && self.follow.is_some() {
            let down = self.grabbed;
            if (down[0] - p[0]).hypot(down[1] - p[1]) < CLICK_SLOP {
                return false;
            }
            self.let_go();
            from = down;
        }
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
    ///
    /// It lets go of the subject, as any touch of the map does, but goes on following
    /// the puck until the press drags (see [`Self::pointer_move`]): it may be a tap.
    pub fn pointer_down(&mut self, position: [f64; 2], button: Button, rotate: bool) -> bool {
        self.subject = None;
        self.cursor = self.physical(position);
        self.grabbed = self.cursor;
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
            && (p[0] - self.cursor[0]).hypot(p[1] - self.cursor[1]) < CLICK_SLOP
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
        self.let_go();
        self.cursor = self.physical(position);
        // Flutter reports a notch as about 50 logical pixels; half a zoom level per notch.
        let levels = -delta / 100.0;
        self.motion
            .zoom(&self.camera, levels, self.cursor, Instant::now());
    }

    /// A touchpad or pinch gesture started at `position`: stop anything moving, so it
    /// zooms from where things are.
    pub fn pan_zoom_start(&mut self, position: [f64; 2]) {
        self.let_go();
        self.cursor = self.physical(position);
        self.motion.grab(self.cursor, Instant::now());
    }

    /// A touchpad or pinch gesture at `position` moved by `pan` logical pixels, scaled by
    /// `scale` and turned clockwise by `rotation` degrees since its last update.
    pub fn pan_zoom(&mut self, position: [f64; 2], pan: [f64; 2], scale: f64, rotation: f64) {
        self.let_go();
        let p = self.physical(position);
        let ratio = self.camera.pixel_ratio;
        let to = [p[0] + pan[0] * ratio, p[1] + pan[1] * ratio];
        self.camera.drag(p, to);
        let levels = if scale > 0.0 { scale.log2() } else { 0.0 };
        if levels != 0.0 {
            self.camera.zoom_around(levels, to);
        }
        // Every update counts, so a pinch held still before letting go doesn't zoom on.
        self.motion.track_zoom(levels, to, Instant::now());
        if rotation != 0.0 {
            // Turning the map clockwise turns the view to the west.
            self.camera.rotate_around(-rotation, to);
        }
        self.cursor = to;
    }

    /// A touchpad or pinch gesture ended: if it was zooming quickly, zoom on as it slows.
    pub fn pan_zoom_end(&mut self) {
        self.motion.release_zoom(&self.camera, Instant::now());
    }

    /// Turn the map back to north up, keeping the middle of the view less the padding
    /// where it is and the zoom and tilt as they are. Stops following the puck.
    pub fn turn_north(&mut self) {
        self.follow = None;
        // The turn takes over the bearing, so the subject is fitted at whatever the
        // bearing is from now on rather than at one it no longer heads for.
        if let Some(subject) = &mut self.subject {
            subject.bearing = None;
        }
        self.motion
            .turn_north(&self.camera, self.padding, Instant::now());
    }

    /// Two fingers dragged up or down together by `dy` logical pixels: up tilts further.
    pub fn tilt(&mut self, dy: f64) {
        self.let_go();
        let camera = &mut self.camera;
        camera.set_pitch(camera.pitch - dy * 0.5);
    }

    /// A double tap at `position`: glide in a level around it.
    pub fn double_tap_zoom(&mut self, position: [f64; 2]) {
        self.let_go();
        self.cursor = self.physical(position);
        self.motion
            .zoom(&self.camera, 1.0, self.cursor, Instant::now());
    }

    /// A finger held down after a double tap at `position` dragged by `dy` logical pixels:
    /// down zooms in, up zooms out.
    pub fn quick_zoom(&mut self, position: [f64; 2], dy: f64) {
        self.let_go();
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
        self.let_go();
        true
    }

    /// Input moved the map: stop keeping the subject in view, and following the puck.
    /// Taps and clicks don't come here, so they leave the camera following.
    fn let_go(&mut self) {
        self.subject = None;
        self.follow = None;
    }

    /// Add `added` to the sprite images, by name; a name it has already keeps its image.
    /// Nothing already on the map moves or is laid out again.
    pub fn add_images(&mut self, added: &[(String, SpriteImage)]) {
        let atlas = Arc::new(
            self.images
                .as_deref()
                .cloned()
                .unwrap_or_default()
                .with_images(added),
        );
        self.renderer.set_images(&atlas);
        self.map.set_images(atlas.clone());
        self.images = Some(atlas);
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
        let [lng, lat] = to_lng_lat(self.camera.framing(self.padding).target);
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
        let middle = self.camera.framing(self.padding).target;
        self.padding = padding;
        self.motion.repad(padding);
        self.follow_padding(middle);
        true
    }

    /// After the padding or the viewport changed: keep the subject in view, or else
    /// `middle` (world coordinates) in the middle of the view less the padding.
    fn follow_padding(&mut self, middle: [f64; 2]) {
        if self.follow.is_some() {
            // The next frame puts the puck where the new padding has it.
            return;
        }
        if let Some(subject) = self.subject {
            let (center, zoom) = self.subject_view(subject);
            if !self.motion.retarget(center, zoom) {
                self.camera.center = center;
                self.camera.zoom = zoom;
            }
        } else if !self.motion.flying() && !self.motion.easing() {
            let framing = self.camera.aimed_at(middle);
            self.camera.show(&framing, self.padding);
        }
    }

    /// Fly to `[west, south, east, north]` and keep it in view as the padding changes,
    /// zoomed in no closer than `max_zoom`, with `margin` logical pixels clear around it,
    /// until input moves the map or [`MapView::clear_subject`]. With a `bearing` or
    /// `pitch`, the flight turns (the short way) and tilts there as it goes, and the
    /// bounds are fitted as they'll be seen at the end, so a tilted, turned map comes
    /// back flat and north up in one motion, say; without, it keeps the camera's.
    ///
    /// `clear` is kept clear too, edge by edge, on top of the padding and the margin, as
    /// the margin is: for what's drawn over the map at some edges and not others, which
    /// a margin the same all round would either let the bounds run under or waste room
    /// for.
    pub fn set_subject(
        &mut self,
        bounds: [f64; 4],
        max_zoom: f64,
        margin: f64,
        clear: Padding,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) {
        self.follow = None;
        let subject = Subject {
            bounds,
            max_zoom,
            margin,
            clear,
            bearing,
            pitch,
        };
        self.subject = Some(subject);
        let (center, zoom) = self.subject_view(subject);
        self.motion
            .fly_turning(&self.camera, center, zoom, bearing, pitch, Instant::now());
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

    /// The camera's center and zoom for showing `subject` with the current padding, at
    /// its bearing and pitch where it has them and the camera's otherwise.
    fn subject_view(&self, subject: Subject) -> ([f64; 2], f64) {
        // Fitted as seen from where the camera ends up: a tilt squashes the bounds and a
        // turn swaps their width and height, so fitting from the camera as it is now
        // would land too close or too far.
        let mut camera = self.camera.clone();
        if let Some(bearing) = subject.bearing {
            camera.set_bearing(bearing);
        }
        if let Some(pitch) = subject.pitch {
            camera.set_pitch(pitch);
        }
        let camera = &camera;
        let m = subject.margin;
        // The margin and what's kept clear count the same, edge by edge.
        let c = subject.clear;
        let all = Padding {
            top: c.top + m,
            right: c.right + m,
            bottom: c.bottom + m,
            left: c.left + m,
        };
        let fit = self.padding + all;
        let zoom = camera
            .fit_bounds(subject.bounds, fit)
            .zoom
            .min(subject.max_zoom);
        // However little of the view the padding leaves (none, under a full screen
        // sheet), don't zoom out more than SUBJECT_ZOOM_GIVE below how it shows with no
        // padding, only the margin and what's kept clear: past that it isn't worth seeing
        // all of. Easing towards the limit, as a
        // stiffening spring would, so the zoom never stops short.
        let open = camera
            .fit_bounds(subject.bounds, all)
            .zoom
            .min(subject.max_zoom);
        let zoom = if zoom < open {
            open - SUBJECT_ZOOM_GIVE * ((open - zoom) / SUBJECT_ZOOM_GIVE).tanh()
        } else {
            zoom
        };
        // In the middle of what `fit` leaves.
        let framing = camera.frame_bounds(subject.bounds, fit, zoom);
        (camera.center_showing(&framing, fit), zoom)
    }

    /// Fly to show `lng, lat` at `zoom` in the middle of the view less the padding,
    /// dropping any subject and following.
    pub fn fly_to(&mut self, lng: f64, lat: f64, zoom: f64) {
        self.let_go();
        let framing = Framing {
            zoom,
            ..self.camera.aimed_at(lng_lat_to_world(lng, lat))
        };
        let center = self.camera.center_showing(&framing, self.padding);
        self.motion.fly(&self.camera, center, zoom, Instant::now());
    }

    /// Ease to show `lng, lat` in the middle of the view less the padding (at any
    /// pitch), at `zoom`, `bearing` and `pitch` (as they are for `None`), over
    /// `duration` seconds along `curve`, dropping any subject and following. The bearing
    /// turns the short way, and the pitch stops at the camera's limit. Input that moves
    /// the map stops it.
    #[allow(clippy::too_many_arguments)]
    pub fn ease_to(
        &mut self,
        lng: f64,
        lat: f64,
        zoom: f64,
        bearing: Option<f64>,
        pitch: Option<f64>,
        duration: f64,
        curve: Curve,
    ) {
        self.let_go();
        let to = Framing {
            target: lng_lat_to_world(lng, lat),
            zoom,
            bearing: bearing.unwrap_or(self.camera.bearing),
            pitch: pitch.unwrap_or(self.camera.pitch),
        };
        self.motion.ease_to(
            &self.camera,
            to,
            duration,
            curve,
            self.padding,
            Instant::now(),
        );
    }

    /// Put the navigation puck at `lng, lat`, pointing `bearing` degrees clockwise from
    /// north: gliding there from where it's shown over `duration` seconds (at once for
    /// none, or the first time), linearly, and turning the short way. A new pose before
    /// the glide ends bends it from where it got to. Draw frames after.
    pub fn set_puck(&mut self, lng: f64, lat: f64, bearing: f64, duration: f64) {
        let now = Instant::now();
        let pose = Pose {
            world: lng_lat_to_world(lng, lat),
            bearing,
        };
        match &mut self.puck {
            Some(puck) => puck.glide_to(pose, duration, now),
            None => self.puck = Some(Puck::new(pose, now)),
        }
    }

    /// Show or hide the puck. A hidden puck still glides, and the camera can follow it.
    /// Returns whether a frame should be drawn.
    pub fn show_puck(&mut self, shown: bool) -> bool {
        let changed = shown != self.puck_shown;
        self.puck_shown = shown;
        changed && self.puck.is_some()
    }

    /// Draw the puck as `style` has it from now on. Fails if it names a layer to go
    /// under that the style doesn't have.
    pub fn set_puck_style(&mut self, style: PuckStyle) -> Result<()> {
        if let Some(id) = &style.before_layer
            && !self.shown.layers.iter().any(|l| l.id == *id)
        {
            bail!("the style has no layer {id:?} to draw the puck under");
        }
        self.puck_style = style;
        Ok(())
    }

    /// Follow the puck: keep it in the middle of the view less the padding and
    /// `padding` on top (logical pixels, to put it lower on screen, say), moving with
    /// it frame by frame, at `zoom` and `pitch`, with `bearing` fixed or (`None`)
    /// turning with the puck's course. It eases in from where the camera is over
    /// `duration` seconds; calling this again while following changes the zoom, pitch,
    /// bearing or padding the same way. Input that moves the map, or another camera
    /// move, stops it. Fails if the puck hasn't been put anywhere yet.
    pub fn follow(
        &mut self,
        zoom: f64,
        pitch: f64,
        bearing: Option<f64>,
        padding: Padding,
        duration: f64,
    ) -> Result<()> {
        if self.puck.is_none() {
            bail!("there's no puck to follow: set where it is first");
        }
        self.subject = None;
        self.motion.stop();
        let from = self.camera.framing(self.padding + padding);
        self.follow = Some(Follow {
            zoom,
            pitch,
            bearing,
            padding,
            entry: (duration > 0.0).then(|| (from, Instant::now(), duration)),
        });
        Ok(())
    }

    /// Stop following the puck, leaving the camera where it is.
    pub fn stop_following(&mut self) {
        self.follow = None;
    }

    /// Whether the camera is following the puck: [`MapView::follow`] started it, and
    /// neither input, another camera move nor [`MapView::stop_following`] has ended it.
    pub fn is_following(&self) -> bool {
        self.follow.is_some()
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

/// `puck` as drawn at `now` in `style` (as shown) over `map`, as `style` has it look.
fn puck_draw<'a>(
    puck: &Puck,
    look: &'a PuckStyle,
    style: &Style,
    map: &MapState<GpuTile>,
    camera: &Camera,
    now: Instant,
) -> PuckDraw<'a> {
    let pose = puck.at(now);
    let layers = &style.layers;
    let before = look
        .before_layer
        .as_deref()
        .and_then(|id| layers.iter().position(|l| l.id == id))
        .unwrap_or(layers.len());
    let height = map.terrain().map_or(0.0, |t| {
        let meters = t.tiles.elevation_at(pose.world).unwrap_or(0.0);
        f64::from(meters) * t.exaggeration * camera.pixels_per_meter()
    });
    PuckDraw {
        pose,
        style: look,
        before,
        height,
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
        v.set_subject(
            [-122.3493, 47.6205, -122.3493, 47.6205],
            14.0,
            48.0,
            Padding::default(),
            None,
            None,
        );
        settle(&mut v);
        let [lng, lat, zoom, ..] = v.camera();
        assert!((lng + 122.3493).abs() < 1e-6 && (lat - 47.6205).abs() < 1e-6);
        assert!((zoom - 14.0).abs() < 1e-9);

        // The sheet pulled up: still in the middle of what's left.
        v.set_padding(sheet(450.0));
        let [lng, lat, ..] = v.camera();
        assert!((lng + 122.3493).abs() < 1e-6 && (lat - 47.6205).abs() < 1e-6);

        // Bounds too big for zoom 14 in what's left zoom out to fit it, less the margin.
        v.set_subject(
            [-122.44, 47.50, -122.24, 47.73],
            14.0,
            48.0,
            Padding::default(),
            None,
            None,
        );
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
        v.set_subject(
            [-122.3493, 47.6205, -122.3493, 47.6205],
            14.0,
            48.0,
            Padding::default(),
            None,
            None,
        );
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
        v.set_subject(seattle, 14.0, 48.0, Padding::default(), None, None);
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

    /// The box `[west, south, east, north]` is drawn in as `v` shows it, in logical
    /// pixels: `[left, top, right, bottom]`.
    fn drawn(v: &MapView, [w, s, e, n]: [f64; 4]) -> [f64; 4] {
        let corners = [[w, s], [e, s], [e, n], [w, n]];
        let points = corners.map(|[lng, lat]| v.camera.world_to_screen(world(lng, lat)).unwrap());
        let ratio = v.camera.pixel_ratio;
        let lo = |i: usize| points.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min) / ratio;
        let hi = |i: usize| {
            points
                .iter()
                .map(|p| p[i])
                .fold(f64::NEG_INFINITY, f64::max)
                / ratio
        };
        [lo(0), lo(1), hi(0), hi(1)]
    }

    /// Whether `v` shows `bounds` as a subject with `margin` does from a flat, north up
    /// camera under a sheet `bottom` high: at the same center and zoom, and so drawn in
    /// the middle of the view less the sheet, fitted to it as subjects are fitted.
    fn fitted(v: &MapView, bounds: [f64; 4], bottom: f64, margin: f64) -> bool {
        let mut flat = view();
        flat.set_padding(sheet(bottom));
        flat.set_subject(bounds, 18.0, margin, Padding::default(), None, None);
        settle(&mut flat);
        let [left, top, right, low] = drawn(v, bounds);
        let middle = [(left + right) / 2.0, (top + low) / 2.0];
        let room = [400.0, (600.0 - bottom) / 2.0];
        let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-9;
        (middle[0] - room[0]).hypot(middle[1] - room[1]) < 0.5
            && near(v.camera.center, flat.camera.center)
            && (v.camera.zoom - flat.camera.zoom).abs() < 1e-9
    }

    const ROUTE: [f64; 4] = [-122.40, 47.58, -122.30, 47.66];

    #[test]
    fn a_subject_at_a_bearing_and_pitch_lands_there_fitted() {
        let mut v = view();
        v.set_padding(sheet(250.0));
        v.camera.set_bearing(-120.0);
        v.camera.set_pitch(45.0);
        v.set_subject(ROUTE, 18.0, 48.0, Padding::default(), Some(0.0), Some(0.0));
        v.frame(Stream::default());
        // On its way it's partly turned and tilted, not snapped flat.
        let [.., bearing, pitch] = v.camera();
        assert!(bearing > -120.0 && pitch > 0.0, "{bearing} {pitch}");
        settle(&mut v);
        let [.., bearing, pitch] = v.camera();
        assert_eq!((bearing, pitch), (0.0, 0.0));
        assert!(fitted(&v, ROUTE, 250.0, 48.0), "{:?}", drawn(&v, ROUTE));
        assert!(v.has_subject());
    }

    #[test]
    fn a_subject_keeps_its_bearing_and_pitch_as_the_padding_changes() {
        let mut v = view();
        v.set_padding(sheet(450.0));
        v.camera.set_bearing(80.0);
        v.camera.set_pitch(50.0);
        v.set_subject(ROUTE, 18.0, 48.0, Padding::default(), Some(0.0), Some(0.0));
        v.frame(Stream::default());
        // The sheet settles lower mid-flight.
        v.set_padding(sheet(200.0));
        settle(&mut v);
        let [.., bearing, pitch] = v.camera();
        assert_eq!((bearing, pitch), (0.0, 0.0));
        assert!(fitted(&v, ROUTE, 200.0, 48.0), "{:?}", drawn(&v, ROUTE));
        // And once landed, as it moves again.
        v.set_padding(sheet(300.0));
        let [.., bearing, pitch] = v.camera();
        assert_eq!((bearing, pitch), (0.0, 0.0));
        assert!(fitted(&v, ROUTE, 300.0, 48.0), "{:?}", drawn(&v, ROUTE));
    }

    #[test]
    fn a_subject_without_a_bearing_or_pitch_keeps_the_cameras() {
        let mut v = view();
        v.set_padding(sheet(250.0));
        v.camera.set_bearing(-120.0);
        v.camera.set_pitch(45.0);
        // Where the camera would go fitted at its own bearing and pitch, as before.
        let expected = v.subject_view(Subject {
            bounds: ROUTE,
            max_zoom: 18.0,
            margin: 48.0,
            clear: Padding::default(),
            bearing: None,
            pitch: None,
        });
        v.set_subject(ROUTE, 18.0, 48.0, Padding::default(), None, None);
        settle(&mut v);
        let [.., bearing, pitch] = v.camera();
        assert!((bearing + 120.0).abs() < 1e-9 && (pitch - 45.0).abs() < 1e-9);
        assert_eq!((v.camera.center, v.camera.zoom), expected);
        // Fitted as drawn at that tilt: in the middle of the view less the sheet, and
        // within the margin.
        let [left, top, right, low] = drawn(&v, ROUTE);
        assert!(((left + right) / 2.0 - 400.0).abs() < 0.5, "{left} {right}");
        assert!(((top + low) / 2.0 - 175.0).abs() < 0.5, "{top} {low}");
        assert!(left >= 47.5 && right <= 752.5, "{left} {right}");
    }

    #[test]
    fn a_subject_keeps_clear_of_what_is_drawn_over_the_map() {
        // The top 200 and the bottom 100 of the 800 × 600 view covered, as by the
        // navigation screen's banner and trip progress, the top 24 of it being the view's
        // own padding, for a status bar.
        let mut v = view();
        v.set_padding(Padding {
            top: 24.0,
            ..Padding::default()
        });
        let clear = Padding {
            top: 176.0,
            bottom: 100.0,
            ..Padding::default()
        };
        v.camera.set_pitch(45.0);
        v.set_subject(ROUTE, 18.0, 16.0, clear, Some(0.0), Some(0.0));
        settle(&mut v);
        // The route is taller than what's left is wide for it, so it fills the height
        // between 200 and 500, less the margin, and is in the middle across.
        let [left, top, right, low] = drawn(&v, ROUTE);
        assert!(
            (top - 216.0).abs() < 0.5 && (low - 484.0).abs() < 0.5,
            "{top} {low}"
        );
        assert!(left >= 16.0 && right <= 784.0, "{left} {right}");
        assert!(((left + right) / 2.0 - 400.0).abs() < 0.5, "{left} {right}");
    }

    fn world(lng: f64, lat: f64) -> [f64; 2] {
        lng_lat_to_world(lng, lat)
    }

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-9
    }

    #[test]
    fn following_holds_the_puck_still_on_screen_as_it_glides() {
        let mut v = view();
        v.set_padding(sheet(200.0));
        v.set_puck(-122.33, 47.6, 0.0, 0.0);
        // Room above it: the middle of what's left is at y = 200 + 200 / 2.
        let ahead = Padding {
            top: 200.0,
            ..Padding::default()
        };
        v.follow(16.0, 60.0, None, ahead, 0.0).unwrap();
        v.set_puck(-122.32, 47.61, 45.0, 1.0);
        // Just after the glide started.
        let t = Instant::now();
        for ms in [0, 100, 350, 700, 1000, 1500] {
            let now = t + Duration::from_millis(ms);
            let moving = v.place_following(now);
            assert_eq!(moving, ms < 1000, "{ms}");
            let pose = v.puck.as_ref().unwrap().at(now);
            let shown = v.camera.screen_to_world([400.0, 300.0]).unwrap();
            assert!(near(shown, pose.world), "{ms}: {shown:?} {:?}", pose.world);
            // Turning with the course.
            assert!((v.camera.bearing - pose.bearing).abs() < 1e-9);
            assert_eq!((v.camera.zoom, v.camera.pitch), (16.0, 60.0));
        }
        // At rest, nothing more to draw.
        settle(&mut v);
        assert!(v.is_following());
    }

    #[test]
    fn following_eases_in_and_keeps_a_fixed_bearing() {
        let mut v = view();
        v.set_puck(-122.30, 47.62, 120.0, 0.0);
        let before = v.camera();
        v.follow(15.0, 45.0, Some(0.0), Padding::default(), 0.5)
            .unwrap();
        let t = v.follow.unwrap().entry.unwrap().1;
        // At first, where the camera was.
        v.place_following(t);
        assert!(close(v.camera(), before), "{:?} {before:?}", v.camera());
        assert!(v.place_following(t + Duration::from_millis(250)));
        let partway = v.camera();
        assert!(partway[2] > 12.0 && partway[2] < 15.0, "{partway:?}");
        assert!(!v.place_following(t + Duration::from_millis(500)));
        let shown = v.camera.screen_to_world([400.0, 300.0]).unwrap();
        assert!(near(shown, world(-122.30, 47.62)));
        assert_eq!(
            (v.camera.zoom, v.camera.bearing, v.camera.pitch),
            (15.0, 0.0, 45.0)
        );
    }

    #[test]
    fn following_moves_with_the_sheet() {
        let mut v = view();
        v.set_puck(-122.33, 47.6, 0.0, 0.0);
        v.follow(16.0, 50.0, Some(0.0), Padding::default(), 0.0)
            .unwrap();
        v.frame(Stream::default());
        assert!(v.set_padding(sheet(300.0)));
        v.frame(Stream::default());
        let shown = v.camera.screen_to_world([400.0, 150.0]).unwrap();
        assert!(near(shown, world(-122.33, 47.6)), "{shown:?}");
    }

    #[test]
    fn input_and_other_camera_moves_end_following() {
        let mut v = view();
        assert!(
            v.follow(16.0, 0.0, None, Padding::default(), 0.0).is_err(),
            "no puck yet"
        );
        v.set_puck(-122.33, 47.6, 0.0, 0.0);
        let follow = |v: &mut MapView| {
            v.follow(16.0, 0.0, None, Padding::default(), 0.3).unwrap();
            assert!(v.frame(Stream::default()).following);
        };
        follow(&mut v);
        v.pointer_down([400.0, 300.0], Button::Primary, false);
        v.pointer_move([400.0, 340.0]);
        v.pointer_up([400.0, 340.0], Button::Primary, false);
        assert!(!v.is_following() && !v.frame(Stream::default()).following);
        follow(&mut v);
        v.pointer_down([400.0, 300.0], Button::Secondary, false);
        v.pointer_move([460.0, 300.0]);
        assert!(!v.is_following(), "rotating with the mouse");
        v.pointer_up([460.0, 300.0], Button::Secondary, false);
        follow(&mut v);
        v.scroll([400.0, 300.0], 50.0);
        assert!(!v.is_following());
        follow(&mut v);
        v.pan_zoom_start([400.0, 300.0]);
        assert!(!v.is_following());
        follow(&mut v);
        v.tilt(10.0);
        assert!(!v.is_following());
        follow(&mut v);
        v.double_tap_zoom([400.0, 300.0]);
        assert!(!v.is_following());
        follow(&mut v);
        v.quick_zoom([400.0, 300.0], 20.0);
        assert!(!v.is_following());
        follow(&mut v);
        v.fly_to(-122.3, 47.6, 12.0);
        assert!(!v.is_following());
        follow(&mut v);
        v.set_subject(
            [-122.35, 47.6, -122.3, 47.62],
            16.0,
            0.0,
            Padding::default(),
            None,
            None,
        );
        assert!(!v.is_following());
        follow(&mut v);
        v.camera.set_bearing(20.0);
        v.turn_north();
        assert!(!v.is_following());
        follow(&mut v);
        v.ease_to(-122.3, 47.6, 12.0, None, None, 0.3, Curve::Ease);
        assert!(!v.is_following());
        follow(&mut v);
        v.stop_following();
        assert!(!v.is_following());
    }

    #[test]
    fn an_ease_lands_its_target_in_the_middle_of_the_padding() {
        let mut v = view();
        v.set_padding(sheet(300.0));
        v.ease_to(
            -122.35,
            47.62,
            15.0,
            Some(200.0),
            Some(70.0),
            0.2,
            Curve::EaseOut,
        );
        assert!(v.frame(Stream::default()).animating);
        settle(&mut v);
        let shown = v.camera.screen_to_world([400.0, 150.0]).unwrap();
        assert!(near(shown, world(-122.35, 47.62)), "{shown:?}");
        assert_eq!(
            (v.camera.zoom, v.camera.bearing, v.camera.pitch),
            (15.0, -160.0, 70.0)
        );
    }

    #[test]
    fn a_shown_puck_draws_frames_only_while_it_glides() {
        let mut v = view();
        v.set_puck(-122.33, 47.6, 0.0, 0.0);
        assert!(!v.frame(Stream::default()).animating, "hidden and still");
        assert!(v.show_puck(true));
        v.set_puck(-122.32, 47.6, 90.0, 0.2);
        assert!(v.frame(Stream::default()).animating);
        settle(&mut v);
        assert!(v.set_puck_style(PuckStyle::default()).is_ok());
        let missing = PuckStyle {
            before_layer: Some("nope".into()),
            ..PuckStyle::default()
        };
        assert!(v.set_puck_style(missing).is_err());
    }

    /// What replacing a route's GeoJSON costs: `cargo test --release -p sparkle_flutter
    /// geojson_swap_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn geojson_swap_cost() {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("sparkle-swap-cost-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let style = dir.join("style.json");
        let line = |id: &str, width: f64| {
            serde_json::json!({"id": id, "type": "line", "source": "route",
                "layout": {"line-cap": "round", "line-join": "round"},
                "paint": {"line-color": "#1a73e8", "line-width": width}})
        };
        let style_json = serde_json::json!({"version": 8,
            "sources": {"route": {"type": "geojson",
                "data": {"type": "FeatureCollection", "features": []}}},
            "layers": [line("casing", 9.0), line("route", 6.0)]});
        std::fs::write(&style, style_json.to_string()).unwrap();
        let args = [
            "--style",
            style.to_str().unwrap(),
            "--view",
            "-122.33,47.6,15",
        ]
        .map(String::from)
        .to_vec();
        let info = GpuInfo {
            uniform_alignment: 256,
            sample_count: 1,
            half_float: false,
        };
        let mut v =
            MapView::new(args, info, [1080, 2400], 2.75, Arc::new(Mutex::new(None))).unwrap();
        // A winding route of 4000 points across about 20 km, split where the puck is
        // into the part travelled and the part ahead, as navigation updates it.
        let points: Vec<[f64; 2]> = (0..4000)
            .map(|i| {
                let t = f64::from(i) / 4000.0;
                [
                    -122.42 + 0.2 * t,
                    47.55 + 0.1 * t + 0.004 * (t * 300.0).sin(),
                ]
            })
            .collect();
        let route = |split: usize| {
            let feature = |coords: &[[f64; 2]], part: &str| {
                serde_json::json!({"type": "Feature", "properties": {"part": part},
                    "geometry": {"type": "LineString", "coordinates": coords}})
            };
            serde_json::json!({"type": "FeatureCollection", "features": [
                feature(&points[..=split], "travelled"), feature(&points[split..], "ahead")]})
            .to_string()
        };
        let swap = |v: &mut MapView, split: usize| {
            let text = route(split);
            let start = Instant::now();
            v.set_geojson("route", &text).unwrap();
            let set = start.elapsed();
            let mut frames = 0;
            let mut frame_time = Duration::ZERO;
            while v.map.swapping("route") {
                let f = Instant::now();
                v.frame(Stream::default());
                frame_time += f.elapsed();
                frames += 1;
                if v.map.swapping("route") {
                    std::thread::sleep(Duration::from_micros(200));
                }
                assert!(frames < 100_000);
            }
            (set, start.elapsed(), frames, frame_time, text.len())
        };
        // Warm up: the first data and the first tiles.
        swap(&mut v, 1000);
        settle(&mut v);
        let runs: Vec<_> = (0..20).map(|i| swap(&mut v, 1000 + i * 10)).collect();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut sets: Vec<f64> = runs.iter().map(|r| ms(r.0)).collect();
        let mut swaps: Vec<f64> = runs.iter().map(|r| ms(r.1)).collect();
        sets.sort_by(f64::total_cmp);
        swaps.sort_by(f64::total_cmp);
        let frames: usize = runs.iter().map(|r| r.2).sum();
        let frame_time: f64 = runs.iter().map(|r| ms(r.3)).sum();
        println!(
            "{} bytes of GeoJSON, {} tiles drawn: set_geojson (parse + index) median {:.2} ms, \
             max {:.2}; until swapped in median {:.2} ms, max {:.2}; {:.2} ms per frame \
             over {} frames",
            runs[0].4,
            v.frame(Stream::default()).tiles_drawn,
            sets[sets.len() / 2],
            sets[sets.len() - 1],
            swaps[swaps.len() / 2],
            swaps[swaps.len() - 1],
            frame_time / frames as f64,
            frames,
        );
    }

    #[test]
    fn touching_the_map_lets_go_of_the_subject() {
        let mut v = view();
        v.set_subject(
            [-122.3493, 47.6205, -122.3493, 47.6205],
            14.0,
            48.0,
            Padding::default(),
            None,
            None,
        );
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

    #[test]
    fn taps_and_clicks_keep_following_and_a_pan_ends_it() {
        let mut v = view();
        v.set_puck(-122.33, 47.6, 30.0, 0.0);
        v.follow(16.0, 60.0, None, Padding::default(), 0.0).unwrap();
        v.frame(Stream::default());
        let placed = v.camera();
        // A touch tap: down, up (touch says it's not a click yet), then the click once it
        // can't be a double tap.
        v.pointer_down([300.0, 200.0], Button::Primary, false);
        v.pointer_up([300.0, 200.0], Button::Primary, false);
        assert!(!v.click([300.0, 200.0]), "nothing to select there");
        assert!(v.frame(Stream::default()).following);
        // A mouse click that wobbles a little.
        v.pointer_down([300.0, 200.0], Button::Primary, false);
        assert!(!v.pointer_move([302.0, 201.0]), "no drag yet");
        v.pointer_up([302.0, 201.0], Button::Primary, true);
        // A right click.
        v.pointer_down([300.0, 200.0], Button::Secondary, false);
        v.pointer_up([300.0, 200.0], Button::Secondary, true);
        assert!(v.frame(Stream::default()).following);
        assert!(close(v.camera(), placed), "the camera never moved");

        // A pan: once past a click's reach it lets go, dragging from where it went
        // down, so the ground under the finger then is under it now.
        v.pointer_down([300.0, 200.0], Button::Primary, false);
        let grabbed = v.camera.screen_to_world([300.0, 200.0]).unwrap();
        v.pointer_move([302.0, 202.0]);
        assert!(v.is_following());
        v.pointer_move([330.0, 260.0]);
        assert!(!v.is_following());
        let under = v.camera.screen_to_world([330.0, 260.0]).unwrap();
        assert!(near(under, grabbed), "{under:?} {grabbed:?}");
        v.pointer_up([330.0, 260.0], Button::Primary, false);
        assert!(!v.frame(Stream::default()).following);
    }

    /// A sheet, a little more on the left than the right, and some room at the top: the
    /// middle of what it leaves of the 800 by 600 view is at (455, 195).
    fn lopsided() -> Padding {
        Padding {
            top: 40.0,
            right: 10.0,
            bottom: 250.0,
            left: 120.0,
        }
    }

    const MIDDLE: [f64; 2] = [455.0, 195.0];

    fn shows(v: &MapView, lng: f64, lat: f64) -> bool {
        let shown = v.camera.screen_to_world(MIDDLE).unwrap();
        (shown[0] - world(lng, lat)[0]).hypot(shown[1] - world(lng, lat)[1]) < 1e-9
    }

    #[test]
    fn tilted_or_not_the_camera_is_the_ground_in_the_middle_of_the_padding() {
        for pitch in [0.0, 60.0] {
            let mut v = view();
            v.camera.set_bearing(-25.0);
            v.camera.set_pitch(pitch);
            let before = v.camera();
            assert!(v.set_padding(lopsided()));
            assert!(
                close(v.camera(), before),
                "{pitch}: {:?} {before:?}",
                v.camera()
            );
            let [lng, lat, ..] = before;
            assert!(shows(&v, lng, lat), "{pitch}");
            // Easing to where the camera is leaves it there, as does showing it again.
            let center = v.camera.center;
            v.ease_to(lng, lat, before[2], None, None, 0.2, Curve::Ease);
            settle(&mut v);
            assert!(
                close(v.camera(), before),
                "{pitch}: {:?} {before:?}",
                v.camera()
            );
            assert!(near(v.camera.center, center), "{pitch}");
        }
    }

    #[test]
    fn tilted_or_not_a_flight_lands_in_the_middle_of_the_padding() {
        for pitch in [0.0, 60.0] {
            let mut v = view();
            v.camera.set_bearing(40.0);
            v.camera.set_pitch(pitch);
            v.set_padding(lopsided());
            v.fly_to(-122.30, 47.65, 15.0);
            settle(&mut v);
            assert!(shows(&v, -122.30, 47.65), "{pitch}");
            let [lng, lat, zoom, bearing, p] = v.camera();
            assert!((lng + 122.30).abs() < 1e-6 && (lat - 47.65).abs() < 1e-6);
            assert_eq!((zoom, bearing, p), (15.0, 40.0, pitch));
        }
    }

    #[test]
    fn tilted_or_not_a_subject_shows_in_the_middle_of_the_padding() {
        for pitch in [0.0, 60.0] {
            let mut v = view();
            v.camera.set_pitch(pitch);
            v.set_padding(lopsided());
            v.set_subject(
                [-122.3493, 47.6205, -122.3493, 47.6205],
                14.0,
                48.0,
                Padding::default(),
                None,
                None,
            );
            settle(&mut v);
            assert!(shows(&v, -122.3493, 47.6205), "{pitch}");
            // The sheet moves: still there.
            v.set_padding(Padding {
                bottom: 100.0,
                ..lopsided()
            });
            let shown = v.camera.screen_to_world([455.0, 270.0]).unwrap();
            assert!(near(shown, world(-122.3493, 47.6205)), "{pitch}");
            // Bounds are drawn in the middle of what the padding leaves. (They don't
            // quite fit it: padding zooms out less than it would take, see
            // SUBJECT_ZOOM_GIVE, and the camera's tests check the fit itself.)
            v.set_padding(lopsided());
            v.set_subject(
                [-122.36, 47.60, -122.30, 47.63],
                18.0,
                0.0,
                Padding::default(),
                None,
                None,
            );
            settle(&mut v);
            let corners = [
                [-122.36, 47.63],
                [-122.30, 47.63],
                [-122.30, 47.60],
                [-122.36, 47.60],
            ]
            .map(|[lng, lat]| v.camera.world_to_screen(world(lng, lat)).unwrap());
            let (xs, ys) = (corners.map(|c| c[0]), corners.map(|c| c[1]));
            let min = |a: [f64; 4]| a.into_iter().fold(f64::INFINITY, f64::min);
            let max = |a: [f64; 4]| a.into_iter().fold(f64::NEG_INFINITY, f64::max);
            assert!(
                ((min(xs) + max(xs)) / 2.0 - MIDDLE[0]).abs() < 1e-3,
                "{pitch}"
            );
            assert!(
                ((min(ys) + max(ys)) / 2.0 - MIDDLE[1]).abs() < 1e-3,
                "{pitch}"
            );
        }
    }

    #[test]
    fn tilted_or_not_turning_north_keeps_the_middle_of_the_padding() {
        for pitch in [0.0, 60.0] {
            let mut v = view();
            v.camera.set_bearing(-130.0);
            v.camera.set_pitch(pitch);
            v.set_padding(lopsided());
            let [lng, lat, zoom, ..] = v.camera();
            v.turn_north();
            v.frame(Stream::default());
            settle(&mut v);
            assert!(shows(&v, lng, lat), "{pitch}");
            assert_eq!(
                (v.camera.zoom, v.camera.bearing, v.camera.pitch),
                (zoom, 0.0, pitch)
            );
        }
    }
}
