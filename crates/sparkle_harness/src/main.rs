//! A map of a tile server's tiles or local MBTiles and GeoJSON files, drawn with wgpu from
//! geometry built by `sparkle`.
//!
//! See [`args::USAGE`] for the command line. With `--headless`, it renders one frame
//! offscreen once every visible tile has loaded, saves it and exits, without opening a
//! window.

mod render;

pub use sparkle_view::{args, camera, loader, map, mbtiles, motion, setup};

use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use sparkle::Style;
use sparkle::sprite::ImageAtlas;
use sparkle::style::StyleDiff;
use sparkle::transition::Transitions;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::args::Args;
use crate::camera::Camera;
use crate::loader::{LoaderConfig, SourceConfig, TileLoader, TileMessage};
use crate::map::MapSource;
use crate::mbtiles::Metadata;
use crate::motion::Motion;
use crate::render::{GpuTile, Renderer};
use crate::setup::{
    Marked, initial_camera, initial_feature_state, load_style, mark_feature, sources, sprites,
};

type MapState = crate::map::MapState<GpuTile>;
const DEFAULT_SIZE: [u32; 2] = [1024, 768];
/// How long headless rendering waits for tiles before saving what it has.
const HEADLESS_TIMEOUT: Duration = Duration::from_secs(180);
/// How often a `--style` file is checked for changes.
const STYLE_POLL: Duration = Duration::from_millis(500);

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"),
    )
    .init();
    let args = args::parse(std::env::args().skip(1))?;
    let style = Arc::new(load_style(args.style.as_deref(), &args)?);
    let (configs, map_sources, metadata) = sources(&args, &style)?;
    if let Some(fonts) = &args.fonts {
        log::info!("fonts: {}", fonts.display());
    }
    if args.headless {
        run_headless(args, style, configs, map_sources, metadata)
    } else {
        run_windowed(args, style, configs, map_sources, metadata)
    }
}

fn spawn_loader(
    args: &Args,
    style: &Arc<Style>,
    sources: Vec<SourceConfig>,
    images: Option<Arc<ImageAtlas>>,
    deliver: impl Fn(TileMessage) + Send + Sync + 'static,
) -> Result<TileLoader> {
    TileLoader::spawn(
        LoaderConfig {
            style: style.clone(),
            sources,
            images,
            fonts: args.fonts.clone(),
        },
        deliver,
    )
}

fn run_headless(
    args: Args,
    style: Arc<Style>,
    configs: Vec<SourceConfig>,
    map_sources: Vec<MapSource>,
    metadata: Option<Metadata>,
) -> Result<()> {
    let path = args.screenshot.clone().expect("checked by args::parse");
    let [width, height] = args.size.unwrap_or(DEFAULT_SIZE);
    let pixel_ratio = args.pixel_ratio.unwrap_or(1.0);
    let atlas = sprites(&args, &style, pixel_ratio)?.map(Arc::new);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = pollster::block_on(Renderer::headless(&instance, width, height))?;
    if let Some(atlas) = &atlas {
        renderer.set_images(atlas);
    }
    let (tx, rx) = mpsc::channel();
    let loader = spawn_loader(&args, &style, configs, atlas, move |m| drop(tx.send(m)))?;
    let mut map = MapState::new(style.clone(), map_sources, loader, args.tile_memory);
    let mut camera = Camera::new([f64::from(width), f64::from(height)], pixel_ratio);
    initial_camera(&mut camera, &args, &style, metadata.as_ref());
    initial_feature_state(&mut map, &mut renderer, &args);

    let deadline = Instant::now() + HEADLESS_TIMEOUT;
    let wait = |map: &mut MapState, renderer: &mut Renderer, camera: &mut Camera| -> Result<()> {
        loop {
            map.update_camera(camera);
            let frame = map.frame(camera);
            if frame.complete {
                return Ok(());
            }
            if Instant::now() >= deadline {
                log::warn!("gave up waiting for {} tiles", frame.loading);
                return Ok(());
            }
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(message) => {
                    map.receive(renderer, message);
                    while let Ok(message) = rx.try_recv() {
                        map.receive(renderer, message);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("the tile loader stopped"),
            }
        }
    };
    wait(&mut map, &mut renderer, &mut camera)?;

    if let Some(point) = args.hover {
        mark_feature(
            &mut map,
            &mut renderer,
            &camera,
            point,
            "hover",
            None,
            false,
        );
    }
    // The style as drawn: the current one, with transitions applied.
    let mut shown = (*map.style).clone();
    let mut transitions = Transitions::new();
    if let Some(next) = &args.next_style {
        let next = load_style(Some(next), &args)?;
        let diff = map.style.diff(&next);
        transitions.start(&shown, &next, 0.0);
        map.set_style(&mut renderer, Arc::new(next), &diff);
        wait(&mut map, &mut renderer, &mut camera)?;
        shown = (*map.style).clone();
        transitions.apply(&map.style, &mut shown, args.transition_ms, camera.zoom);
    }

    // Let tiles finish fading in.
    while renderer.animating() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    map.update_camera(&mut camera);
    let frame = map.frame(&camera);
    renderer.screenshot(&shown, &camera, &map.draws(&frame), map.terrain(), &path)?;
    log::info!(
        "saved {} ({} tiles drawn)",
        path.display(),
        frame.tiles_drawn()
    );
    Ok(())
}

fn run_windowed(
    args: Args,
    style: Arc<Style>,
    configs: Vec<SourceConfig>,
    map_sources: Vec<MapSource>,
    metadata: Option<Metadata>,
) -> Result<()> {
    let event_loop = EventLoop::<TileMessage>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
        Box::new(event_loop.owned_display_handle()),
    ));
    let mut app = App {
        instance,
        style_mtime: args.style.as_deref().and_then(modified),
        shown: (*style).clone(),
        args,
        style,
        metadata,
        pending: Some((configs, map_sources, proxy)),
        map: None,
        view: None,
        exit: false,
        cursor: [0.0; 2],
        drag: None,
        motion: Motion::default(),
        press: None,
        modifiers: ModifiersState::empty(),
        error: None,
        transitions: Transitions::new(),
        clock: Instant::now(),
        next_poll: Instant::now() + STYLE_POLL,
        hovered: None,
        selected: None,
    };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What a mouse drag does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Pan,
    Rotate,
}

struct View {
    window: Arc<Window>,
    renderer: Renderer,
    camera: Camera,
}

type Pending = (
    Vec<SourceConfig>,
    Vec<MapSource>,
    winit::event_loop::EventLoopProxy<TileMessage>,
);

struct App {
    instance: wgpu::Instance,
    args: Args,
    style: Arc<Style>,
    metadata: Option<Metadata>,
    /// What's needed to start loading once the window (and its pixel ratio) exists.
    pending: Option<Pending>,
    map: Option<MapState>,
    view: Option<View>,
    exit: bool,
    cursor: [f64; 2],
    drag: Option<Drag>,
    /// Wheel zooms and flings still in progress.
    motion: Motion,
    /// Where the left button went down, to tell clicks from drags.
    press: Option<[f64; 2]>,
    modifiers: ModifiersState,
    error: Option<anyhow::Error>,
    /// The style as drawn, with paint transitions applied to the map's style.
    shown: Style,
    transitions: Transitions,
    /// Transitions are timed in milliseconds since this.
    clock: Instant,
    style_mtime: Option<SystemTime>,
    next_poll: Instant,
    hovered: Option<Marked>,
    selected: Option<Marked>,
}

impl App {
    fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64() * 1000.0
    }

    fn redraw(&mut self) {
        let now = self.now();
        let (Some(view), Some(map)) = (&mut self.view, &mut self.map) else {
            return;
        };
        if self.motion.step(&mut view.camera, Instant::now()) {
            view.window.request_redraw();
        }
        map.update_camera(&mut view.camera);
        let camera = &view.camera;
        if self
            .transitions
            .apply(&map.style, &mut self.shown, now, camera.zoom)
        {
            view.window.request_redraw();
        }
        let frame = map.frame(camera);
        let draws = map.draws(&frame);
        view.renderer
            .render(&self.shown, camera, &draws, map.terrain());
        if view.renderer.animating() {
            view.window.request_redraw();
        }
        if let Some(path) = &self.args.screenshot
            && frame.complete
        {
            match view
                .renderer
                .screenshot(&self.shown, camera, &draws, map.terrain(), path)
            {
                Ok(()) => log::info!("saved {}", path.display()),
                Err(err) => self.error = Some(err),
            }
            self.exit = true;
        }
        view.window.set_title(&format!(
            "sparkle — zoom {:.2}, bearing {:.0}°, pitch {:.0}° — {} tiles drawn, {} loading",
            camera.zoom,
            camera.bearing,
            camera.pitch,
            frame.tiles_drawn(),
            frame.loading,
        ));
    }

    /// Reload the `--style` file if it changed, transitioning to it.
    fn poll_style(&mut self) {
        let Some(path) = self.args.style.clone() else {
            return;
        };
        let mtime = modified(&path);
        if mtime == self.style_mtime {
            return;
        }
        self.style_mtime = mtime;
        let new = match load_style(Some(&path), &self.args) {
            Ok(style) => style,
            Err(err) => {
                log::warn!("not reloading the style: {err:#}");
                return;
            }
        };
        let now = self.now();
        let (Some(view), Some(map)) = (&mut self.view, &mut self.map) else {
            return;
        };
        let diff: StyleDiff = map.style.diff(&new);
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
        self.transitions.start(&self.shown, &new, now);
        let new = Arc::new(new);
        map.set_style(&mut view.renderer, new.clone(), &diff);
        self.shown = (*new).clone();
        self.style = new;
        view.window.request_redraw();
    }

    /// Hover the feature under the cursor, or with `click`, toggle its selection.
    fn mark(&mut self, click: bool) {
        let (Some(view), Some(map)) = (&mut self.view, &mut self.map) else {
            return;
        };
        let (key, slot) = if click {
            ("selected", &mut self.selected)
        } else {
            ("hover", &mut self.hovered)
        };
        let (marked, changed) = mark_feature(
            map,
            &mut view.renderer,
            &view.camera,
            self.cursor,
            key,
            slot.as_ref(),
            click,
        );
        *slot = marked;
        if changed {
            view.window.request_redraw();
        }
    }

    fn create_view(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let mut attributes = Window::default_attributes().with_title("sparkle");
        attributes = match self.args.size {
            Some([w, h]) => attributes.with_inner_size(winit::dpi::PhysicalSize::new(w, h)),
            None => attributes.with_inner_size(winit::dpi::LogicalSize::new(
                DEFAULT_SIZE[0],
                DEFAULT_SIZE[1],
            )),
        };
        let window = Arc::new(event_loop.create_window(attributes)?);
        let renderer = pollster::block_on(Renderer::windowed(&self.instance, window.clone()))?;
        let size = window.inner_size();
        let pixel_ratio = self.args.pixel_ratio.unwrap_or(window.scale_factor());
        let mut camera = Camera::new([f64::from(size.width), f64::from(size.height)], pixel_ratio);
        initial_camera(&mut camera, &self.args, &self.style, self.metadata.as_ref());
        let mut view = View {
            window,
            renderer,
            camera,
        };

        if let Some((configs, map_sources, proxy)) = self.pending.take() {
            let atlas = sprites(&self.args, &self.style, pixel_ratio)?.map(Arc::new);
            if let Some(atlas) = &atlas {
                view.renderer.set_images(atlas);
            }
            // Sending only fails once the event loop has exited, when nobody needs the tile.
            let loader = spawn_loader(&self.args, &self.style, configs, atlas, move |m| {
                drop(proxy.send_event(m))
            })?;
            let mut map = MapState::new(
                self.style.clone(),
                map_sources,
                loader,
                self.args.tile_memory,
            );
            initial_feature_state(&mut map, &mut view.renderer, &self.args);
            self.map = Some(map);
        }
        self.view = Some(view);
        Ok(())
    }
}

impl ApplicationHandler<TileMessage> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.view.is_none()
            && let Err(err) = self.create_view(event_loop)
        {
            self.error = Some(err);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, message: TileMessage) {
        let (Some(view), Some(map)) = (&mut self.view, &mut self.map) else {
            return;
        };
        if map.receive(&mut view.renderer, message) {
            view.window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.args.style.is_none() {
            return;
        }
        if Instant::now() >= self.next_poll {
            self.poll_style();
            self.next_poll = Instant::now() + STYLE_POLL;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_poll));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(view) = &mut self.view else { return };
        let camera = &mut view.camera;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                self.redraw();
                if self.exit {
                    event_loop.exit();
                }
                return;
            }
            WindowEvent::Resized(size) => {
                view.renderer.resize(size.width, size.height);
                camera.viewport = [f64::from(size.width), f64::from(size.height)];
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if self.args.pixel_ratio.is_none() {
                    camera.pixel_ratio = scale_factor;
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                return;
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = [position.x, position.y];
                let from = std::mem::replace(&mut self.cursor, p);
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
                    None => {
                        self.mark(false);
                        return;
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                if button == MouseButton::Left {
                    let press = std::mem::replace(&mut self.press, pressed.then_some(self.cursor));
                    // A release where the button went down is a click.
                    if let (false, Some(p)) = (pressed, press)
                        && (p[0] - self.cursor[0]).hypot(p[1] - self.cursor[1]) < 4.0
                    {
                        self.drag = None;
                        self.mark(true);
                        return;
                    }
                }
                if !pressed && self.drag == Some(Drag::Pan) {
                    self.motion.release(camera, Instant::now());
                }
                self.drag = match (pressed, button) {
                    (false, _) => None,
                    (true, MouseButton::Right) => Some(Drag::Rotate),
                    (true, MouseButton::Left) if self.modifiers.control_key() => Some(Drag::Rotate),
                    (true, MouseButton::Left) => {
                        self.motion.grab(self.cursor, Instant::now());
                        Some(Drag::Pan)
                    }
                    (true, _) => self.drag,
                };
                return;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                match delta {
                    // Wheel notches glide; touchpads already scroll smoothly.
                    MouseScrollDelta::LineDelta(_, y) => {
                        let levels = f64::from(y) * 0.5;
                        self.motion
                            .zoom(camera, levels, self.cursor, Instant::now());
                    }
                    MouseScrollDelta::PixelDelta(p) => camera.zoom_around(p.y / 200.0, self.cursor),
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let center = [camera.viewport[0] / 2.0, camera.viewport[1] / 2.0];
                let step = 100.0 * camera.pixel_ratio;
                let shift = self.modifiers.shift_key();
                match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character("+" | "=") => {
                        self.motion.zoom(camera, 1.0, center, Instant::now())
                    }
                    Key::Character("-") => self.motion.zoom(camera, -1.0, center, Instant::now()),
                    Key::Character("n" | "N") => {
                        camera.set_bearing(0.0);
                        camera.set_pitch(0.0);
                    }
                    Key::Named(NamedKey::ArrowLeft) if shift => {
                        camera.set_bearing(camera.bearing - 15.0)
                    }
                    Key::Named(NamedKey::ArrowRight) if shift => {
                        camera.set_bearing(camera.bearing + 15.0)
                    }
                    Key::Named(NamedKey::ArrowUp) if shift => camera.set_pitch(camera.pitch + 10.0),
                    Key::Named(NamedKey::ArrowDown) if shift => {
                        camera.set_pitch(camera.pitch - 10.0)
                    }
                    Key::Named(NamedKey::ArrowLeft) => camera.pan(step, 0.0),
                    Key::Named(NamedKey::ArrowRight) => camera.pan(-step, 0.0),
                    Key::Named(NamedKey::ArrowUp) => camera.pan(0.0, step),
                    Key::Named(NamedKey::ArrowDown) => camera.pan(0.0, -step),
                    _ => return,
                }
            }
            _ => return,
        }
        view.window.request_redraw();
    }
}
