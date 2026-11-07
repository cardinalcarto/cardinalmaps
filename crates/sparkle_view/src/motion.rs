//! Easing for window input: wheel zooms that glide to their level, and flings that carry a
//! pan on after the mouse lets go; and flights the app starts, which zoom out and back in
//! to get somewhere. All advance once per frame through [`Motion::step`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::camera::{Camera, MAX_ZOOM, TILE_SIZE};

/// The zoom still to go shrinks by a factor of e every this many seconds.
const ZOOM_TIME: f64 = 0.06;
/// Zooms closer than this to their target finish at once.
const ZOOM_DONE: f64 = 1e-3;
/// Only the drag's last this much counts toward a fling's velocity.
const FLING_WINDOW: Duration = Duration::from_millis(100);
/// A drag held still this long before letting go doesn't fling.
const FLING_PAUSE: Duration = Duration::from_millis(50);
/// A fling's speed shrinks by a factor of e every this many seconds, so it carries on for
/// its speed times this.
const FLING_TIME: f64 = 0.3;
/// Fling speeds in logical pixels per second: faster ones are capped, slower ones stop.
const MAX_SPEED: f64 = 2000.0;
const MIN_SPEED: f64 = 20.0;
/// The longest time one step advances, so a stalled frame doesn't jump.
const MAX_STEP: f64 = 0.05;
/// How far a flight zooms out on the way, as MapLibre's `flyTo` `curve`.
const FLY_CURVE: f64 = 1.42;
/// A flight's average speed, in screenfuls per second along its path (MapLibre's
/// `speed`).
const FLY_SPEED: f64 = 1.2;
/// Flights take at least and at most this many seconds.
const FLY_MIN_TIME: f64 = 0.3;
const FLY_MAX_TIME: f64 = 2.5;

#[derive(Default)]
pub struct Motion {
    /// Zoom levels still to go, and the point (physical pixels) they zoom around.
    zoom: f64,
    zoom_anchor: [f64; 2],
    /// The fling's velocity in physical pixels per second, and the point it drags.
    velocity: [f64; 2],
    fling_anchor: [f64; 2],
    /// Recent pan positions, oldest first.
    samples: VecDeque<(Instant, [f64; 2])>,
    /// When the last step ran, while anything is moving.
    last: Option<Instant>,
    flight: Option<Flight>,
}

/// A flight from one view to another along van Wijk and Nuttall's path, as MapLibre's
/// `flyTo`: zooming out while panning, so the view keeps moving at a steady pace on
/// screen.
struct Flight {
    start: Instant,
    /// Seconds.
    duration: f64,
    from: [f64; 2],
    from_zoom: f64,
    /// The viewport's logical size when the flight started.
    view: [f64; 2],
    /// Where the center goes; its `x` may be off by a world so the flight goes the short
    /// way around.
    to: [f64; 2],
    /// The path's length, in the units of [`Flight::at`].
    length: f64,
    path: Path,
}

enum Path {
    /// Zooming out and back in on the way.
    Curve { r0: f64, w0: f64, u1: f64 },
    /// Zooming in or out in place: the view is scaled by `e^(direction ρ s)`.
    Zoom { direction: f64 },
}

impl Flight {
    fn new(camera: &Camera, to: [f64; 2], zoom: f64, now: Instant) -> Self {
        let view = camera.logical_size();
        let (to, length, path) = Self::path(camera.center, camera.zoom, view, to, zoom);
        Self {
            start: now,
            duration: (length / FLY_SPEED).clamp(FLY_MIN_TIME, FLY_MAX_TIME),
            from: camera.center,
            from_zoom: camera.zoom,
            view,
            to,
            length,
            path,
        }
    }

    /// Head for `to` at `zoom` instead, keeping where and when the flight started and
    /// how long it takes, so a small change bends the rest of the path without a jump.
    fn retarget(&mut self, to: [f64; 2], zoom: f64) {
        (self.to, self.length, self.path) =
            Self::path(self.from, self.from_zoom, self.view, to, zoom);
    }

    /// The path from `from` at `from_zoom` to `to` at `zoom` for a viewport of `view`
    /// logical pixels: the end (the short way around), the path's length, and its shape.
    fn path(
        from: [f64; 2],
        from_zoom: f64,
        view: [f64; 2],
        to: [f64; 2],
        zoom: f64,
    ) -> ([f64; 2], f64, Path) {
        let mut to = to;
        to[0] += (from[0] - to[0]).round();
        let rho = FLY_CURVE;
        let rho2 = rho * rho;
        // Sizes in logical pixels at the starting zoom.
        let w0 = view[0].max(view[1]);
        let w1 = w0 / (zoom - from_zoom).exp2();
        let size = TILE_SIZE * from_zoom.exp2();
        let u1 = (to[0] - from[0]).hypot(to[1] - from[1]) * size;
        let r = |end: bool| {
            let (w, sign) = if end { (w1, -1.0) } else { (w0, 1.0) };
            let b = (w1 * w1 - w0 * w0 + sign * rho2 * rho2 * u1 * u1) / (2.0 * w * rho2 * u1);
            ((b * b + 1.0).sqrt() - b).ln()
        };
        let r0 = r(false);
        let curve = (r(true) - r0) / rho;
        if u1 > 1e-6 && curve.is_finite() {
            (to, curve, Path::Curve { r0, w0, u1 })
        } else {
            let direction = if w1 < w0 { -1.0 } else { 1.0 };
            (to, (w1 / w0).ln().abs() / rho, Path::Zoom { direction })
        }
    }

    /// The center and zoom `s` along the path.
    fn at(&self, s: f64) -> ([f64; 2], f64) {
        let rho = FLY_CURVE;
        let (u, w) = match self.path {
            Path::Curve { r0, w0, u1 } => (
                w0 * (r0.cosh() * (r0 + rho * s).tanh() - r0.sinh()) / (rho * rho) / u1,
                r0.cosh() / (r0 + rho * s).cosh(),
            ),
            Path::Zoom { direction } => (0.0, (direction * rho * s).exp()),
        };
        let center = [
            self.from[0] + (self.to[0] - self.from[0]) * u,
            self.from[1] + (self.to[1] - self.from[1]) * u,
        ];
        (center, self.from_zoom - w.log2())
    }

    /// Move `camera` to where the flight is at `now`. Returns whether it has further to go.
    fn step(&self, camera: &mut Camera, now: Instant) -> bool {
        let t = ((now - self.start).as_secs_f64() / self.duration).min(1.0);
        let (center, zoom) = self.at(ease(t) * self.length);
        camera.center = [center[0].rem_euclid(1.0), center[1].clamp(0.0, 1.0)];
        camera.zoom = zoom.clamp(0.0, MAX_ZOOM);
        t < 1.0
    }
}

/// CSS's `ease` timing curve, `cubic-bezier(0.25, 0.1, 0.25, 1)`, as MapLibre eases
/// flights.
fn ease(t: f64) -> f64 {
    let bezier = |a: f64, b: f64, x: f64| {
        let m = 1.0 - x;
        3.0 * m * m * x * a + 3.0 * m * x * x * b + x * x * x
    };
    // Find where the curve's x is t, then take its y there.
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if bezier(0.25, 0.25, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(0.1, 1.0, (lo + hi) / 2.0)
}

impl Motion {
    pub fn active(&self) -> bool {
        self.zoom != 0.0 || self.velocity != [0.0; 2] || self.flight.is_some()
    }

    /// Fly to `center` (world coordinates) at `zoom`, ending anything else in progress.
    pub fn fly(&mut self, camera: &Camera, center: [f64; 2], zoom: f64, now: Instant) {
        self.zoom = 0.0;
        self.velocity = [0.0; 2];
        self.samples.clear();
        self.flight = Some(Flight::new(camera, center, zoom.clamp(0.0, MAX_ZOOM), now));
        self.last = Some(now);
    }

    /// Glide `levels` further than any zoom in progress, around `anchor`.
    pub fn zoom(&mut self, camera: &Camera, levels: f64, anchor: [f64; 2], now: Instant) {
        self.flight = None;
        self.wake(now);
        let target = (camera.zoom + self.zoom + levels).clamp(0.0, MAX_ZOOM);
        self.zoom = target - camera.zoom;
        self.zoom_anchor = anchor;
    }

    /// Whether a flight is under way.
    pub fn flying(&self) -> bool {
        self.flight.is_some()
    }

    /// Have the flight under way head for `center` (world coordinates) at `zoom` instead.
    /// Returns false if there's none.
    pub fn retarget(&mut self, center: [f64; 2], zoom: f64) -> bool {
        let Some(flight) = &mut self.flight else {
            return false;
        };
        flight.retarget(center, zoom.clamp(0.0, MAX_ZOOM));
        true
    }

    /// A pan starts at `p`: stop any fling or flight. Returns whether there was one.
    pub fn grab(&mut self, p: [f64; 2], now: Instant) -> bool {
        let stopped = self.velocity != [0.0; 2] || self.flight.is_some();
        self.velocity = [0.0; 2];
        self.flight = None;
        self.samples.clear();
        self.samples.push_back((now, p));
        stopped
    }

    /// The pan moved to `p`.
    pub fn track(&mut self, p: [f64; 2], now: Instant) {
        self.samples.push_back((now, p));
        while self
            .samples
            .front()
            .is_some_and(|&(t, _)| now - t > FLING_WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// The pan let go: fling on at its recent velocity, if it was still moving.
    pub fn release(&mut self, camera: &Camera, now: Instant) {
        let samples = std::mem::take(&mut self.samples);
        let (Some(&(t0, p0)), Some(&(t1, p1))) = (samples.front(), samples.back()) else {
            return;
        };
        let duration = (t1 - t0).as_secs_f64();
        if now - t1 > FLING_PAUSE || duration <= 0.0 {
            return;
        }
        let mut velocity = [(p1[0] - p0[0]) / duration, (p1[1] - p0[1]) / duration];
        let speed = velocity[0].hypot(velocity[1]);
        let ratio = camera.pixel_ratio;
        if speed < MIN_SPEED * ratio {
            return;
        }
        let scale = (MAX_SPEED * ratio / speed).min(1.0);
        velocity = velocity.map(|v| v * scale);
        self.wake(now);
        self.velocity = velocity;
        self.fling_anchor = p1;
    }

    /// Advance to `now`. Returns whether anything is still moving.
    pub fn step(&mut self, camera: &mut Camera, now: Instant) -> bool {
        let Some(last) = self.last else {
            return false;
        };
        let dt = (now - last).as_secs_f64().min(MAX_STEP);
        self.last = Some(now);

        if let Some(flight) = &self.flight
            && !flight.step(camera, now)
        {
            self.flight = None;
        }

        if self.zoom != 0.0 {
            let mut delta = self.zoom * (1.0 - (-dt / ZOOM_TIME).exp());
            if (self.zoom - delta).abs() < ZOOM_DONE {
                delta = self.zoom;
            }
            camera.zoom_around(delta, self.zoom_anchor);
            self.zoom -= delta;
        }

        if self.velocity != [0.0; 2] {
            // How far the decaying velocity carries over `dt`.
            let decay = (-dt / FLING_TIME).exp();
            let travel = FLING_TIME * (1.0 - decay);
            let from = self.fling_anchor;
            let to = [
                from[0] + self.velocity[0] * travel,
                from[1] + self.velocity[1] * travel,
            ];
            camera.drag(from, to);
            self.velocity = self.velocity.map(|v| v * decay);
            if self.velocity[0].hypot(self.velocity[1]) < MIN_SPEED * camera.pixel_ratio {
                self.velocity = [0.0; 2];
            }
        }

        if !self.active() {
            self.last = None;
        }
        self.active()
    }

    /// Start timing steps from `now` unless something is already moving.
    fn wake(&mut self, now: Instant) {
        if !self.active() {
            self.last = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        let mut c = Camera::new([800.0, 600.0], 1.0);
        c.look_at(-122.33, 47.6, 12.0);
        c
    }

    /// Step at 60 fps until nothing moves, returning the frame count.
    fn settle(motion: &mut Motion, camera: &mut Camera, mut now: Instant) -> usize {
        let mut frames = 0;
        loop {
            now += Duration::from_millis(16);
            frames += 1;
            if !motion.step(camera, now) {
                return frames;
            }
            assert!(frames < 1000, "never settled");
        }
    }

    #[test]
    fn wheel_zoom_glides_to_its_target() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.zoom(&c, 0.5, [400.0, 300.0], t);
        m.zoom(&c, 0.5, [400.0, 300.0], t);
        assert!(m.step(&mut c, t + Duration::from_millis(16)));
        assert!(c.zoom > 12.0 && c.zoom < 13.0);
        let frames = settle(&mut m, &mut c, t + Duration::from_millis(16));
        assert!(frames < 30, "{frames} frames");
        assert!((c.zoom - 13.0).abs() < 1e-9);
    }

    #[test]
    fn wheel_zoom_stops_at_the_limits() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.zoom(&c, -20.0, [400.0, 300.0], t);
        settle(&mut m, &mut c, t);
        assert_eq!(c.zoom, 0.0);
    }

    #[test]
    fn a_quick_drag_flings_on() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.grab([400.0, 300.0], t);
        for i in 1..=5 {
            m.track(
                [400.0 + 10.0 * f64::from(i), 300.0],
                t + Duration::from_millis(10 * i as u64),
            );
        }
        let end = t + Duration::from_millis(50);
        // Panning to the right moves the center west.
        let before = c.center;
        m.release(&c, end);
        assert!(m.active());
        settle(&mut m, &mut c, end);
        assert!(c.center[0] < before[0]);
        assert!((c.center[1] - before[1]).abs() < 1e-9);
    }

    #[test]
    fn grabbing_says_whether_it_stopped_a_fling() {
        let c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        assert!(!m.grab([400.0, 300.0], t));
        m.track([500.0, 300.0], t + Duration::from_millis(50));
        m.release(&c, t + Duration::from_millis(50));
        assert!(m.active());
        assert!(m.grab([400.0, 300.0], t + Duration::from_millis(60)));
        assert!(!m.active());
    }

    #[test]
    fn a_drag_held_still_does_not_fling() {
        let c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.grab([400.0, 300.0], t);
        m.track([450.0, 300.0], t + Duration::from_millis(20));
        m.release(&c, t + Duration::from_millis(200));
        assert!(!m.active());
    }

    #[test]
    fn a_flight_ends_where_it_was_headed() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        let to = crate::camera::lng_lat_to_world(8.54, 47.37);
        m.fly(&c, to, 14.0, t);
        // Partway, it has zoomed out to see both ends.
        m.step(&mut c, t + Duration::from_millis(16));
        let mut lowest = c.zoom;
        let mut now = t + Duration::from_millis(16);
        while m.step(&mut c, now) {
            lowest = lowest.min(c.zoom);
            now += Duration::from_millis(16);
        }
        assert!(now - t <= Duration::from_secs_f64(FLY_MAX_TIME + 0.1));
        assert!(lowest < 6.0, "{lowest}");
        assert!((c.zoom - 14.0).abs() < 1e-9);
        assert!((c.center[0] - to[0]).abs() < 1e-9 && (c.center[1] - to[1]).abs() < 1e-9);
    }

    #[test]
    fn a_flight_in_place_just_zooms() {
        let mut c = camera();
        let center = c.center;
        let mut m = Motion::default();
        let t = Instant::now();
        m.fly(&c, center, 15.0, t);
        settle(&mut m, &mut c, t);
        assert!((c.zoom - 15.0).abs() < 1e-9);
        assert_eq!(c.center, center);
    }

    #[test]
    fn a_retargeted_flight_ends_at_its_new_target_without_jumping() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        let to = crate::camera::lng_lat_to_world(-122.35, 47.62);
        m.fly(&c, to, 14.0, t);
        let mut now = t;
        for _ in 0..10 {
            now += Duration::from_millis(16);
            m.step(&mut c, now);
        }
        let before = (c.center, c.zoom);
        // A few pixels further south, as when the sheet covering the bottom moves on a
        // frame: the path bends by about as much.
        let moved = [to[0], to[1] + 1e-6];
        assert!(m.retarget(moved, 14.0));
        // The same moment, on the new path.
        m.step(&mut c, now);
        let jump = (c.center[0] - before.0[0]).hypot(c.center[1] - before.0[1]);
        assert!(jump < 1e-5, "{jump}");
        assert!((c.zoom - before.1).abs() < 5e-3, "{} -> {}", before.1, c.zoom);
        settle(&mut m, &mut c, now);
        assert!((c.center[1] - moved[1]).abs() < 1e-12 && (c.zoom - 14.0).abs() < 1e-9);
        assert!(!m.retarget(to, 14.0), "no flight to retarget once landed");
    }

    #[test]
    fn grabbing_stops_a_flight() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.fly(&c, [0.2, 0.3], 10.0, t);
        m.step(&mut c, t + Duration::from_millis(16));
        m.grab([400.0, 300.0], t + Duration::from_millis(20));
        assert!(!m.active());
    }

    #[test]
    fn grabbing_stops_a_fling() {
        let c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.grab([400.0, 300.0], t);
        m.track([500.0, 300.0], t + Duration::from_millis(20));
        m.release(&c, t + Duration::from_millis(20));
        assert!(m.active());
        m.grab([400.0, 300.0], t + Duration::from_millis(30));
        assert!(!m.active());
    }
}
