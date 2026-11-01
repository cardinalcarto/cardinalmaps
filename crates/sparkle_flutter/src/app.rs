//! A map on screen and what input does to it: a port of `sparkle_harness`'s window, with
//! Flutter's events (forwarded by the widget) in place of winit's.
//!
//! Positions come in logical pixels, as Flutter has them, and are turned into the physical
//! pixels the camera works in.

use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Instant, SystemTime};

use anyhow::{Result, bail};
use sparkle::Style;
use sparkle::style::StyleDiff;
use sparkle::transition::Transitions;
use sparkle_view::args::Args;
use sparkle_view::camera::Camera;
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
    /// The pointer, in physical pixels.
    cursor: [f64; 2],
    drag: Option<Drag>,
    /// Where the primary button went down, to tell clicks from drags.
    press: Option<[f64; 2]>,
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
        let mut map = MapState::new(style.clone(), map_sources, loader);
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
            cursor: [0.0; 2],
            drag: None,
            press: None,
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
        self.renderer.resize(size);
        self.camera.viewport = size.map(f64::from);
        if self.args.pixel_ratio.is_none() {
            self.camera.pixel_ratio = pixel_ratio;
        }
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

    /// Hover the feature under the pointer, or with `click`, toggle its selection.
    /// Returns whether anything changed on screen.
    fn mark(&mut self, click: bool) -> bool {
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
    /// with ctrl held). Returns whether a frame should be drawn.
    pub fn pointer_down(&mut self, position: [f64; 2], button: Button, rotate: bool) -> bool {
        self.cursor = self.physical(position);
        if button == Button::Primary {
            self.press = Some(self.cursor);
        }
        self.drag = match button {
            Button::Secondary => Some(Drag::Rotate),
            Button::Primary if rotate => Some(Drag::Rotate),
            Button::Primary => {
                self.motion.grab(self.cursor, Instant::now());
                Some(Drag::Pan)
            }
            Button::Other => self.drag,
        };
        false
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
        self.cursor = self.physical(position);
        // Flutter reports a notch as about 50 logical pixels; half a zoom level per notch.
        let levels = -delta / 100.0;
        self.motion
            .zoom(&self.camera, levels, self.cursor, Instant::now());
    }

    /// A touchpad or pinch gesture at `position` moved by `pan` logical pixels, scaled by
    /// `scale` and turned clockwise by `rotation` degrees since its last update.
    pub fn pan_zoom(&mut self, position: [f64; 2], pan: [f64; 2], scale: f64, rotation: f64) {
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
        let camera = &mut self.camera;
        camera.set_pitch(camera.pitch - dy * 0.5);
    }

    /// A double tap at `position`: glide in a level around it.
    pub fn double_tap_zoom(&mut self, position: [f64; 2]) {
        self.cursor = self.physical(position);
        self.motion
            .zoom(&self.camera, 1.0, self.cursor, Instant::now());
    }

    /// A finger held down after a double tap at `position` dragged by `dy` logical pixels:
    /// down zooms in, up zooms out.
    pub fn quick_zoom(&mut self, position: [f64; 2], dy: f64) {
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
        true
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

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
