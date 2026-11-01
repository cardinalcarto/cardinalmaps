//! What Dart calls: a map that draws itself as commands for Flutter GPU, and the input
//! it reacts to. All but [`SparkleMap::wake_stream`] run synchronously on the calling
//! isolate's thread.

use std::sync::{Arc, Mutex, MutexGuard};

use flutter_rust_bridge::frb;

use crate::app::{Button, MapView, Wake};
use crate::frb_generated::StreamSink;
use crate::gpu::{GpuInfo, Stream};

/// What the device can do, as Flutter GPU reports it.
pub struct DeviceInfo {
    /// Offsets of uniform blocks in a buffer must be multiples of this.
    pub uniform_alignment: u32,
    /// 4 with multisampling, else 1.
    pub sample_count: u32,
    /// Whether `r16g16b16a16Float` textures can be rendered to and blended.
    pub half_float: bool,
}

/// One frame: commands for `executor.dart`, and what the map is doing.
///
/// The commands and their data stay in Rust's memory, where Dart reads them in place
/// (they're megabytes when tiles arrive): each is the address and length of an array,
/// valid until the next call of [`SparkleMap::frame`].
pub struct FrameOutput {
    /// `u32` words.
    pub commands: Address,
    /// The draws' uniform blocks, in bytes.
    pub uniforms: Address,
    /// Vertices draws bind, in bytes.
    pub data: Address,
    /// Contents of new buffers and textures, in bytes.
    pub uploads: Address,
    /// Draw another frame soon: something is moving or fading, or tiles are waiting.
    pub animating: bool,
    /// Every visible tile has loaded, so nothing is a stand-in.
    pub complete: bool,
    pub tiles_drawn: u32,
    pub loading: u32,
    pub zoom: f64,
    pub bearing: f64,
    pub pitch: f64,
}

/// Where an array is in Rust's memory, and how many elements it has.
pub struct Address {
    pub address: i64,
    pub length: i64,
}

impl Address {
    fn of<T>(values: &[T]) -> Self {
        Self {
            address: values.as_ptr() as i64,
            length: values.len() as i64,
        }
    }
}

#[frb(opaque)]
pub struct SparkleMap {
    view: Mutex<MapView>,
    wake: Wake,
    /// The last frame's commands, which Dart reads in place until the next frame.
    shown: Mutex<Stream>,
}

impl SparkleMap {
    /// A map set up from command line `args` (as `sparkle_harness` takes them), `width`
    /// by `height` physical pixels with `pixel_ratio` of them per logical pixel.
    #[frb(sync)]
    pub fn new(
        args: Vec<String>,
        device: DeviceInfo,
        width: u32,
        height: u32,
        pixel_ratio: f64,
    ) -> anyhow::Result<SparkleMap> {
        let wake: Wake = Arc::new(Mutex::new(None));
        let info = GpuInfo {
            uniform_alignment: device.uniform_alignment,
            sample_count: device.sample_count,
            half_float: device.half_float,
        };
        let view = MapView::new(args, info, [width, height], pixel_ratio, wake.clone())?;
        Ok(SparkleMap {
            view: Mutex::new(view),
            wake,
            shown: Mutex::new(Stream::default()),
        })
    }

    fn view(&self) -> MutexGuard<'_, MapView> {
        self.view.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An event whenever a tile has loaded, so a frame should be drawn.
    pub fn wake_stream(&self, sink: StreamSink<i32>) {
        *self.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(move || {
            let _ = sink.add(0);
        }));
    }

    #[frb(sync)]
    pub fn resize(&self, width: u32, height: u32, pixel_ratio: f64) {
        self.view().resize([width, height], pixel_ratio);
    }

    /// Draw a frame. Dart must be done with the previous frame's commands.
    #[frb(sync)]
    pub fn frame(&self) -> FrameOutput {
        let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        let done = std::mem::take(&mut *shown);
        let frame = self.view().frame(done);
        *shown = frame.stream;
        FrameOutput {
            commands: Address::of(&shown.words),
            uniforms: Address::of(&shown.uniforms),
            data: Address::of(&shown.data),
            uploads: Address::of(&shown.uploads),
            animating: frame.animating,
            complete: frame.complete,
            tiles_drawn: frame.tiles_drawn as u32,
            loading: frame.loading as u32,
            zoom: frame.zoom,
            bearing: frame.bearing,
            pitch: frame.pitch,
        }
    }

    /// Reload the `--style` file if it changed. Returns whether a frame should be drawn.
    #[frb(sync)]
    pub fn poll_style(&self) -> bool {
        self.view().poll_style()
    }

    /// Where `--screenshot` wants the first complete frame saved, if anywhere.
    #[frb(sync)]
    pub fn screenshot_path(&self) -> Option<String> {
        self.view()
            .screenshot_path()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// With `--hover x,y`, hover that point. Returns whether a frame should be drawn.
    #[frb(sync)]
    pub fn hover_argument(&self) -> bool {
        self.view().hover_argument()
    }

    /// Positions are in logical pixels. Returns whether a frame should be drawn.
    #[frb(sync)]
    pub fn pointer_move(&self, x: f64, y: f64) -> bool {
        self.view().pointer_move([x, y])
    }

    /// `button`: 0 primary, 1 secondary, else another. `rotate`: a primary drag rotates.
    #[frb(sync)]
    pub fn pointer_down(&self, x: f64, y: f64, button: u32, rotate: bool) -> bool {
        self.view().pointer_down([x, y], to_button(button), rotate)
    }

    /// `click`: a release where the button went down is a click; touch taps say no, and
    /// click once they can't be double taps.
    #[frb(sync)]
    pub fn pointer_up(&self, x: f64, y: f64, button: u32, click: bool) -> bool {
        self.view().pointer_up([x, y], to_button(button), click)
    }

    /// A click or tap at `x, y`. Returns whether a frame should be drawn.
    #[frb(sync)]
    pub fn click(&self, x: f64, y: f64) -> bool {
        self.view().click([x, y])
    }

    /// A mouse wheel turned by `delta` logical pixels (positive scrolls down).
    #[frb(sync)]
    pub fn scroll(&self, x: f64, y: f64, delta: f64) {
        self.view().scroll([x, y], delta);
    }

    /// A touchpad or pinch gesture moved by `dx, dy`, scaled by `scale` and turned
    /// clockwise by `rotation` degrees since its last update.
    #[frb(sync)]
    pub fn pan_zoom(&self, x: f64, y: f64, dx: f64, dy: f64, scale: f64, rotation: f64) {
        self.view().pan_zoom([x, y], [dx, dy], scale, rotation);
    }

    /// Two fingers dragged up or down together by `dy` logical pixels: up tilts further.
    #[frb(sync)]
    pub fn tilt(&self, dy: f64) {
        self.view().tilt(dy);
    }

    /// A double tap: glide in a level around it.
    #[frb(sync)]
    pub fn double_tap_zoom(&self, x: f64, y: f64) {
        self.view().double_tap_zoom([x, y]);
    }

    /// A finger held down after a double tap at `x, y` dragged by `dy` logical pixels:
    /// down zooms in, up zooms out.
    #[frb(sync)]
    pub fn quick_zoom(&self, x: f64, y: f64, dy: f64) {
        self.view().quick_zoom([x, y], dy);
    }

    /// `key`: a character, or a named key such as `ArrowLeft`.
    #[frb(sync)]
    pub fn key(&self, key: String, shift: bool) -> bool {
        self.view().key(&key, shift)
    }
}

fn to_button(button: u32) -> Button {
    match button {
        0 => Button::Primary,
        1 => Button::Secondary,
        _ => Button::Other,
    }
}

#[frb(init)]
pub fn init_app() {
    // As `setup_default_user_utils`, but logging `info` and up: its `Trace` costs time on
    // every frame on Android, where log lines go through logd.
    flutter_rust_bridge::setup_log_to_console(log::LevelFilter::Info);
    flutter_rust_bridge::setup_backtrace();
    // Platforms the bridge doesn't log on.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
}
