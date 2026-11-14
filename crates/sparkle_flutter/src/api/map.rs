//! What Dart calls: a map that draws itself as commands for Flutter GPU, and the input
//! it reacts to. All but [`SparkleMap::wake_stream`] run synchronously on the calling
//! isolate's thread.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use flutter_rust_bridge::frb;

use sparkle::sprite::{ImageMeta, SpriteImage};
use sparkle_view::camera::Padding;
use sparkle_view::motion::Curve;
use sparkle_view::puck::PuckStyle;

use crate::app::{Button, MapView, Wake};
use crate::frb_generated::StreamSink;
use crate::gpu::{GpuInfo, Stream};

/// An image to add to the map's sprite: see [`SparkleMap::add_images`].
pub struct SpriteImageInput {
    pub name: String,
    /// RGBA, 8 bits per channel, premultiplied alpha, rows top down.
    pub pixels: Vec<u8>,
    /// In physical pixels.
    pub width: u32,
    pub height: u32,
    /// Physical pixels per logical pixel.
    pub pixel_ratio: f32,
}

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
    /// The camera is following the puck: see [`SparkleMap::follow`]. Input that moves
    /// the map ends it, which the next frame tells.
    pub following: bool,
}

/// How the navigation puck, a chevron on a disk, looks: see
/// [`SparkleMap::set_puck_style`].
pub struct PuckStyleInput {
    /// The chevron's fill, `0xAARRGGBB` as Dart's `Color.toARGB32`, as are the other
    /// colors.
    pub fill: u32,
    pub outline: u32,
    /// In logical pixels, inside the chevron's edge; 0 for none.
    pub outline_width: f64,
    /// The chevron's length from tip to tail, in logical pixels.
    pub size: f64,
    /// The disk under the chevron, which hides the route line just behind it (see
    /// `sparkle_view::puck::PuckStyle::disk_fill` for why).
    pub disk_fill: u32,
    pub disk_outline: u32,
    /// In logical pixels, inside the disk's edge; 0 for none.
    pub disk_outline_width: f64,
    /// In logical pixels on screen, like `size`; 0 for no disk.
    pub disk_radius: f64,
    /// The id of the style layer it's drawn under; `None` for over every layer (see
    /// `sparkle_view::puck::PuckStyle::before_layer` for why that's the default).
    pub before_layer: Option<String>,
}

impl From<PuckStyleInput> for PuckStyle {
    fn from(p: PuckStyleInput) -> Self {
        let rgba = |argb: u32| {
            let [a, r, g, b] = argb.to_be_bytes().map(|c| f32::from(c) / 255.0);
            [r, g, b, a]
        };
        PuckStyle {
            fill: rgba(p.fill),
            outline: rgba(p.outline),
            outline_width: p.outline_width.max(0.0) as f32,
            size: p.size.max(0.0) as f32,
            disk_fill: rgba(p.disk_fill),
            disk_outline: rgba(p.disk_outline),
            disk_outline_width: p.disk_outline_width.max(0.0) as f32,
            disk_radius: p.disk_radius.max(0.0) as f32,
            before_layer: p.before_layer,
        }
    }
}

/// How an ease's progress follows time, as CSS's timing functions of the same names.
pub enum EaseCurve {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl From<EaseCurve> for Curve {
    fn from(c: EaseCurve) -> Self {
        match c {
            EaseCurve::Linear => Curve::Linear,
            EaseCurve::Ease => Curve::Ease,
            EaseCurve::EaseIn => Curve::EaseIn,
            EaseCurve::EaseOut => Curve::EaseOut,
            EaseCurve::EaseInOut => Curve::EaseInOut,
        }
    }
}

fn properties_text(
    properties: &std::collections::BTreeMap<String, maplibre_expr::Value>,
) -> HashMap<String, String> {
    properties
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), property_text(v)?)))
        .collect()
}

fn property_text(value: &maplibre_expr::Value) -> Option<String> {
    use maplibre_expr::Value;
    match value {
        Value::String(s) => Some(s.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A feature of a label that was clicked or tapped.
pub struct ClickedFeature {
    /// The style's source it's from.
    pub source: String,
    /// The layer of its source's tiles it's in.
    pub source_layer: Option<String>,
    /// The tile it's in, for looking up what else is there; see
    /// [`SparkleMap::tile_feature`].
    pub tile_z: u8,
    pub tile_x: i64,
    pub tile_y: u32,
    pub id: u64,
    /// Where the label is anchored.
    pub lng: f64,
    pub lat: f64,
    /// Its properties that are strings, numbers or booleans, as text.
    pub properties: HashMap<String, String>,
}

/// Where the camera is.
pub struct CameraState {
    pub lng: f64,
    pub lat: f64,
    pub zoom: f64,
    /// Degrees clockwise from north.
    pub bearing: f64,
    /// Degrees from looking straight down.
    pub pitch: f64,
}

pub struct LngLat {
    pub lng: f64,
    pub lat: f64,
}

/// Space at the edges of the view to keep clear, such as under a sheet, in logical
/// pixels.
pub struct MapPadding {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl From<MapPadding> for Padding {
    fn from(p: MapPadding) -> Self {
        Padding {
            top: p.top,
            right: p.right,
            bottom: p.bottom,
            left: p.left,
        }
    }
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
    ///
    /// Not sync: setting up reads files and fetches the tile servers' TileJSON, which
    /// takes the network's time, and would hold the app's UI thread until it was done.
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
            following: frame.following,
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

    /// The features of the labels the last click landed on, nearest first. Later calls
    /// return nothing until the next click.
    #[frb(sync)]
    pub fn take_clicked(&self) -> Vec<ClickedFeature> {
        self.view()
            .take_clicked()
            .into_iter()
            .map(|f| ClickedFeature {
                source: f.source,
                source_layer: f.source_layer,
                tile_z: f.tile.z,
                tile_x: f.tile.x,
                tile_y: f.tile.y,
                id: f.id,
                lng: f.lng,
                lat: f.lat,
                properties: properties_text(&f.properties),
            })
            .collect()
    }

    /// The properties of the feature with `id` in `source_layer` of a tile, as for
    /// [`ClickedFeature::properties`], if the tile is still loaded and has one. Read it
    /// right after the click, as tiles come and go.
    #[frb(sync)]
    pub fn tile_feature(
        &self,
        source: String,
        source_layer: String,
        tile_z: u8,
        tile_x: i64,
        tile_y: u32,
        id: u64,
    ) -> Option<HashMap<String, String>> {
        let tile = sparkle_view::camera::Placement {
            z: tile_z,
            x: tile_x,
            y: tile_y,
        };
        let properties = self
            .view()
            .tile_feature(&source, tile, Some(&source_layer), id)?;
        Some(properties_text(&properties))
    }

    /// A mouse wheel turned by `delta` logical pixels (positive scrolls down).
    #[frb(sync)]
    pub fn scroll(&self, x: f64, y: f64, delta: f64) {
        self.view().scroll([x, y], delta);
    }

    /// A touchpad or pinch gesture started at `x, y`: stop anything moving.
    #[frb(sync)]
    pub fn pan_zoom_start(&self, x: f64, y: f64) {
        self.view().pan_zoom_start([x, y]);
    }

    /// A touchpad or pinch gesture moved by `dx, dy`, scaled by `scale` and turned
    /// clockwise by `rotation` degrees since its last update.
    #[frb(sync)]
    pub fn pan_zoom(&self, x: f64, y: f64, dx: f64, dy: f64, scale: f64, rotation: f64) {
        self.view().pan_zoom([x, y], [dx, dy], scale, rotation);
    }

    /// A touchpad or pinch gesture ended (its fingers lifted): if it was zooming quickly,
    /// it zooms on as it slows. Draw frames after.
    #[frb(sync)]
    pub fn pan_zoom_end(&self) {
        self.view().pan_zoom_end();
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

    /// Add `images` to the sprite, for icons to use from now on, by name; a name it has
    /// already keeps its image. Draw a frame after. A layer showing one of them for the
    /// first time needs its data set after this.
    #[frb(sync)]
    pub fn add_images(&self, images: Vec<SpriteImageInput>) {
        let images: Vec<_> = images
            .into_iter()
            .map(|image| {
                (
                    image.name,
                    SpriteImage {
                        pixels: image.pixels,
                        width: image.width,
                        height: image.height,
                        meta: ImageMeta {
                            pixel_ratio: image.pixel_ratio,
                            ..ImageMeta::default()
                        },
                    },
                )
            })
            .collect();
        self.view().add_images(&images);
    }

    /// Replace the data of the style's GeoJSON source `source` with `geojson`, a GeoJSON
    /// object as text. Draw a frame after.
    #[frb(sync)]
    pub fn set_geojson(&self, source: String, geojson: String) -> anyhow::Result<()> {
        self.view().set_geojson(&source, &geojson)
    }

    /// Where the camera is: its center is the middle of the view less the padding.
    #[frb(sync)]
    pub fn camera(&self) -> CameraState {
        let [lng, lat, zoom, bearing, pitch] = self.view().camera();
        CameraState {
            lng,
            lat,
            zoom,
            bearing,
            pitch,
        }
    }

    /// The ground at `x, y` (logical pixels), if the view shows the ground there.
    #[frb(sync)]
    pub fn screen_to_lng_lat(&self, x: f64, y: f64) -> Option<LngLat> {
        let [lng, lat] = self.view().screen_to_lng_lat([x, y])?;
        Some(LngLat { lng, lat })
    }

    /// Cover `padding` at the edges of the view, as a sheet over the map does. The map
    /// is still drawn behind it, but the camera centers things in the rest: it keeps the
    /// subject in view, or else keeps the middle of the rest where it is on the map, as
    /// well as the zoom. Returns whether a frame should be drawn.
    #[frb(sync)]
    pub fn set_padding(&self, padding: MapPadding) -> bool {
        self.view().set_padding(padding.into())
    }

    /// Fly to the bounds (`west > east` crosses the antimeridian), zoomed in no closer
    /// than `max_zoom`, with `margin` logical pixels clear around them, and keep them in
    /// view as the padding changes until input moves the map or [`Self::clear_subject`].
    /// Point-like bounds show at `max_zoom`.
    ///
    /// With a `bearing` or `pitch`, the camera turns (the short way) and tilts there on
    /// the same flight, and the bounds are fitted as seen from there: to come back flat
    /// and north up from a tilted, turned view in one motion, say. `None` keeps the
    /// camera's own. Draw a frame after.
    #[frb(sync)]
    #[allow(clippy::too_many_arguments)]
    pub fn set_subject(
        &self,
        west: f64,
        south: f64,
        east: f64,
        north: f64,
        max_zoom: f64,
        margin: f64,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) {
        self.view()
            .set_subject([west, south, east, north], max_zoom, margin, bearing, pitch);
    }

    /// Stop keeping the subject in view, leaving the camera where it is.
    #[frb(sync)]
    pub fn clear_subject(&self) {
        self.view().clear_subject();
    }

    /// Whether the camera is keeping a subject in view: one was set, and neither input
    /// that moves the map nor [`Self::clear_subject`] has let go of it since.
    #[frb(sync)]
    pub fn has_subject(&self) -> bool {
        self.view().has_subject()
    }

    /// Fly to show `lng, lat` at `zoom` in the middle of the view less the padding,
    /// zooming out on the way if it's far, and dropping any subject. Input that moves
    /// the map stops it. Draw a frame after.
    #[frb(sync)]
    pub fn fly_to(&self, lng: f64, lat: f64, zoom: f64) {
        self.view().fly_to(lng, lat, zoom);
    }

    /// Turn back to north up, keeping the middle of the view less the padding where it
    /// is and the zoom and tilt as they are. Touching the map stops it. Draw frames after.
    #[frb(sync)]
    pub fn turn_north(&self) {
        self.view().turn_north();
    }

    /// Ease to show `lng, lat` in the middle of the view less the padding (at any pitch),
    /// at `zoom`, and at `bearing` and `pitch` (as they are for `None`), over `duration`
    /// seconds along `curve`, dropping any subject and following. The bearing turns the
    /// short way; the pitch stops at the camera's limit (85°). Input that moves the map
    /// stops it. Draw frames after.
    #[frb(sync)]
    #[allow(clippy::too_many_arguments)]
    pub fn ease_to(
        &self,
        lng: f64,
        lat: f64,
        zoom: f64,
        bearing: Option<f64>,
        pitch: Option<f64>,
        duration: f64,
        curve: EaseCurve,
    ) {
        self.view()
            .ease_to(lng, lat, zoom, bearing, pitch, duration, curve.into());
    }

    /// Put the navigation puck at `lng, lat`, pointing `bearing` degrees clockwise from
    /// north, gliding there from where it's shown over `duration` seconds: linearly, and
    /// turning the short way. The first pose, or one with no duration, is taken at once.
    /// A pose that comes before the glide ends bends it from where it got to. Draw
    /// frames after.
    #[frb(sync)]
    pub fn set_puck(&self, lng: f64, lat: f64, bearing: f64, duration: f64) {
        self.view().set_puck(lng, lat, bearing, duration);
    }

    /// Show or hide the puck (hidden until shown, or set by `--puck`). Returns whether
    /// a frame should be drawn.
    #[frb(sync)]
    pub fn show_puck(&self, shown: bool) -> bool {
        self.view().show_puck(shown)
    }

    /// Draw the puck as `style` has it. Fails if it names a layer the style lacks. Draw
    /// a frame after.
    #[frb(sync)]
    pub fn set_puck_style(&self, style: PuckStyleInput) -> anyhow::Result<()> {
        self.view().set_puck_style(style.into())
    }

    /// Follow the puck, frame by frame: keep it in the middle of the view less the
    /// padding and `padding` on top of it (to put it lower on screen, say), at `zoom` and
    /// `pitch`, with `bearing` fixed or (`None`) turning with the puck. It eases in from
    /// where the camera is over `duration` seconds; calling it again while following
    /// changes the settings the same way. Input that moves the map, [`Self::fly_to`],
    /// [`Self::ease_to`], [`Self::set_subject`] and [`Self::turn_north`] end it. Fails if
    /// the puck hasn't been put anywhere yet. Draw frames after.
    #[frb(sync)]
    pub fn follow(
        &self,
        zoom: f64,
        pitch: f64,
        bearing: Option<f64>,
        padding: MapPadding,
        duration: f64,
    ) -> anyhow::Result<()> {
        self.view()
            .follow(zoom, pitch, bearing, padding.into(), duration)
    }

    /// Stop following the puck, leaving the camera where it is.
    #[frb(sync)]
    pub fn stop_following(&self) {
        self.view().stop_following();
    }

    /// Whether the camera is following the puck: [`Self::follow`] started it, and nothing
    /// has ended it since.
    #[frb(sync)]
    pub fn is_following(&self) -> bool {
        self.view().is_following()
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
