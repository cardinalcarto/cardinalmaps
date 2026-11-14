//! Easing for window input: wheel zooms that glide to their level, flings that carry a
//! pan on after the mouse lets go and a pinch zoom on after the fingers lift; and moves
//! the app starts: flights, which zoom out and back in to get somewhere, eases straight
//! to a zoom, bearing and pitch, and turns back to north up. All advance once per frame
//! through [`Motion::step`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::camera::{Camera, Framing, MAX_PITCH, MAX_ZOOM, Padding, TILE_SIZE};

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
/// A zoom fling's speed shrinks by a factor of e every this many seconds, so it zooms on
/// for its speed times this.
const ZOOM_FLING_TIME: f64 = 0.25;
/// Zoom fling speeds in levels per second: faster ones are capped, and slower ones don't
/// start, so a slow, careful pinch stays where it was let go. Once going, one stops when
/// it slows to [`ZOOM_FLING_STOP`].
const MAX_ZOOM_SPEED: f64 = 9.0;
const MIN_ZOOM_SPEED: f64 = 1.5;
const ZOOM_FLING_STOP: f64 = 0.1;
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
/// Turning north up takes this long for the least turn, and up to twice as long for a
/// half turn.
const TURN_TIME: f64 = 0.3;

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
    /// The zoom fling's speed in levels per second, and the point it zooms around.
    zoom_velocity: f64,
    zoom_fling_anchor: [f64; 2],
    /// Recent pinch zooms, oldest first: the levels zoomed since the pinch started, and
    /// the point it zoomed around.
    zoom_samples: VecDeque<(Instant, f64, [f64; 2])>,
    /// When the last step ran, while anything is moving.
    last: Option<Instant>,
    flight: Option<Flight>,
    turn: Option<Turn>,
    ease: Option<Ease>,
}

/// How an ease's progress follows time, as CSS's timing functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    Linear,
    /// `cubic-bezier(0.25, 0.1, 0.25, 1)`, as MapLibre eases.
    #[default]
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Curve {
    /// How far along the ease is (0 to 1) at `t`, the share of its time gone.
    pub fn at(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::Ease => cubic_bezier([0.25, 0.1, 0.25, 1.0], t),
            Self::EaseIn => cubic_bezier([0.42, 0.0, 1.0, 1.0], t),
            Self::EaseOut => cubic_bezier([0.0, 0.0, 0.58, 1.0], t),
            Self::EaseInOut => cubic_bezier([0.42, 0.0, 0.58, 1.0], t),
        }
    }
}

/// Easing from one framing to another along a [`Curve`], keeping the target in the
/// middle of the view less `padding` on the way.
struct Ease {
    start: Instant,
    /// Seconds.
    duration: f64,
    from: Framing,
    to: Framing,
    curve: Curve,
    padding: Padding,
}

impl Ease {
    /// Move `camera` to where the ease is at `now`. Returns whether it has further to go.
    fn step(&self, camera: &mut Camera, now: Instant) -> bool {
        let t = if self.duration > 0.0 {
            ((now - self.start).as_secs_f64() / self.duration).min(1.0)
        } else {
            1.0
        };
        let framing = if t < 1.0 {
            self.from.toward(&self.to, self.curve.at(t))
        } else {
            self.to
        };
        camera.show(&framing, self.padding);
        t < 1.0
    }
}

/// Turning the bearing to north up, keeping the middle of the view less `padding` where
/// it is.
struct Turn {
    start: Instant,
    /// Seconds.
    duration: f64,
    from: f64,
    padding: Padding,
}

impl Turn {
    /// Turn `camera` to where it is at `now`. Returns whether it has further to go.
    fn step(&self, camera: &mut Camera, now: Instant) -> bool {
        let t = ((now - self.start).as_secs_f64() / self.duration).min(1.0);
        let framing = Framing {
            bearing: self.from * (1.0 - ease(t)),
            ..camera.framing(self.padding)
        };
        camera.show(&framing, self.padding);
        t < 1.0
    }
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
    /// The bearing it starts from and how far it turns (the short way), for a flight
    /// that turns as it goes; `None` leaves the bearing to whatever else moves it.
    turn: Option<(f64, f64)>,
    /// The pitch it starts from and the one it ends at, for a flight that tilts as it
    /// goes; `None` leaves the pitch alone.
    tilt: Option<(f64, f64)>,
}

enum Path {
    /// Zooming out and back in on the way.
    Curve { r0: f64, w0: f64, u1: f64 },
    /// Zooming in or out in place: the view is scaled by `e^(direction ρ s)`.
    Zoom { direction: f64 },
}

impl Flight {
    /// A flight from where `camera` is to `to` at `zoom`, turning to `bearing` and
    /// tilting to `pitch` on the way where they're given.
    fn new(
        camera: &Camera,
        to: [f64; 2],
        zoom: f64,
        bearing: Option<f64>,
        pitch: Option<f64>,
        now: Instant,
    ) -> Self {
        let view = camera.logical_size();
        let (to, length, path) = Self::path(camera.center, camera.zoom, view, to, zoom);
        let turn = bearing.map(|b| {
            let by = (b - camera.bearing + 180.0).rem_euclid(360.0) - 180.0;
            (camera.bearing, by)
        });
        let tilt = pitch.map(|p| (camera.pitch, p.clamp(0.0, MAX_PITCH)));
        // A flight that barely moves still takes long enough to turn and tilt without a
        // lurch: as long as turning north up would for the turn, and likewise for
        // tilting by a share of the furthest tilt.
        let turning = turn.map_or(0.0, |(_, by)| TURN_TIME * (1.0 + by.abs() / 180.0));
        let tilting = tilt.map_or(0.0, |(from, to)| {
            TURN_TIME * (1.0 + (to - from).abs() / MAX_PITCH)
        });
        let duration = (length / FLY_SPEED).max(turning).max(tilting);
        Self {
            start: now,
            duration: duration.clamp(FLY_MIN_TIME, FLY_MAX_TIME),
            from: camera.center,
            from_zoom: camera.zoom,
            view,
            to,
            length,
            path,
            turn,
            tilt,
        }
    }

    /// Head for `to` at `zoom` instead, keeping where and when the flight started, how
    /// long it takes and any bearing and pitch it's turning to, so a small change bends
    /// the rest of the path without a jump.
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
        let k = ease(t);
        let (center, zoom) = self.at(k * self.length);
        camera.center = [center[0].rem_euclid(1.0), center[1].clamp(0.0, 1.0)];
        camera.zoom = zoom.clamp(0.0, MAX_ZOOM);
        // Along the same curve as the path, so the turn and tilt land with it. The center
        // was worked out for the end's bearing and pitch, so the view comes out right
        // only once they're there too.
        if let Some((from, by)) = self.turn {
            camera.set_bearing(from + by * k);
        }
        if let Some((from, to)) = self.tilt {
            camera.set_pitch(from + (to - from) * k);
        }
        t < 1.0
    }
}

/// CSS's `ease` timing curve, `cubic-bezier(0.25, 0.1, 0.25, 1)`, as MapLibre eases
/// flights.
fn ease(t: f64) -> f64 {
    Curve::Ease.at(t)
}

/// CSS's `cubic-bezier(x1, y1, x2, y2)` at `t`.
fn cubic_bezier([x1, y1, x2, y2]: [f64; 4], t: f64) -> f64 {
    let bezier = |a: f64, b: f64, x: f64| {
        let m = 1.0 - x;
        3.0 * m * m * x * a + 3.0 * m * x * x * b + x * x * x
    };
    // Find where the curve's x is t, then take its y there.
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if bezier(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(y1, y2, (lo + hi) / 2.0)
}

impl Motion {
    pub fn active(&self) -> bool {
        self.zoom != 0.0
            || self.velocity != [0.0; 2]
            || self.zoom_velocity != 0.0
            || self.flight.is_some()
            || self.turn.is_some()
            || self.ease.is_some()
    }

    /// Where `camera` comes to rest if the motion under way runs its course, or `None`
    /// if nothing is moving: so that what's needed there can load on the way.
    pub fn end(&self, camera: &Camera) -> Option<Camera> {
        if !self.active() {
            return None;
        }
        let mut end = camera.clone();
        if let Some(flight) = &self.flight {
            flight.step(
                &mut end,
                flight.start + Duration::from_secs_f64(flight.duration),
            );
        }
        if let Some(turn) = &self.turn {
            turn.step(
                &mut end,
                turn.start + Duration::from_secs_f64(turn.duration),
            );
        }
        if let Some(ease) = &self.ease {
            end.show(&ease.to, ease.padding);
        }
        if self.zoom != 0.0 {
            end.zoom_around(self.zoom, self.zoom_anchor);
        }
        // A decaying speed carries on for its time constant times the speed it loses
        // before it stops.
        let speed = self.velocity[0].hypot(self.velocity[1]);
        if speed > 0.0 {
            let lost = (1.0 - MIN_SPEED * camera.pixel_ratio / speed).max(0.0);
            let from = self.fling_anchor;
            let travel = self.velocity.map(|v| v * FLING_TIME * lost);
            end.drag(from, [from[0] + travel[0], from[1] + travel[1]]);
        }
        if self.zoom_velocity != 0.0 {
            let lost = (self.zoom_velocity.abs() - ZOOM_FLING_STOP).max(0.0);
            let levels = self.zoom_velocity.signum() * ZOOM_FLING_TIME * lost;
            end.zoom_around(levels, self.zoom_fling_anchor);
        }
        Some(end)
    }

    /// Turn to north up the short way, keeping the middle of the view less `padding`
    /// where it is and changing nothing else.
    pub fn turn_north(&mut self, camera: &Camera, padding: Padding, now: Instant) {
        if camera.bearing == 0.0 {
            self.turn = None;
            return;
        }
        // An ease would turn the bearing too, as would a flight turning as it goes: that
        // one flies on but leaves the bearing to the turn.
        self.ease = None;
        if let Some(flight) = &mut self.flight {
            flight.turn = None;
        }
        self.wake(now);
        // The camera keeps its bearing within ±180°, so heading for 0 is the short way.
        self.turn = Some(Turn {
            start: now,
            duration: TURN_TIME * (1.0 + camera.bearing.abs() / 180.0),
            from: camera.bearing,
            padding,
        });
    }

    /// The view's padding changed: keep the new middle where it is through any turn, and
    /// any ease heading for it.
    pub fn repad(&mut self, padding: Padding) {
        if let Some(turn) = &mut self.turn {
            turn.padding = padding;
        }
        if let Some(ease) = &mut self.ease {
            ease.padding = padding;
        }
    }

    /// Ease to `to` over `duration` seconds along `curve`, keeping the target in the
    /// middle of the view less `padding` (as [`Motion::repad`] changes it), ending
    /// anything else in progress. The bearing turns the short way, and the pitch stays
    /// within [`MAX_PITCH`].
    pub fn ease_to(
        &mut self,
        camera: &Camera,
        to: Framing,
        duration: f64,
        curve: Curve,
        padding: Padding,
        now: Instant,
    ) {
        self.stop();
        let to = Framing {
            zoom: to.zoom.clamp(0.0, MAX_ZOOM),
            pitch: to.pitch.clamp(0.0, MAX_PITCH),
            ..to
        };
        self.ease = Some(Ease {
            start: now,
            duration: duration.max(0.0),
            from: camera.framing(padding),
            to,
            curve,
            padding,
        });
        self.last = Some(now);
    }

    /// Whether an ease is under way.
    pub fn easing(&self) -> bool {
        self.ease.is_some()
    }

    /// End everything in progress, leaving the camera where it is: something else, such
    /// as following the puck, moves it from now on.
    pub fn stop(&mut self) {
        self.zoom = 0.0;
        self.velocity = [0.0; 2];
        self.zoom_velocity = 0.0;
        self.samples.clear();
        self.zoom_samples.clear();
        self.flight = None;
        self.turn = None;
        self.ease = None;
        self.last = None;
    }

    /// Fly to `center` (world coordinates) at `zoom`, ending anything else in progress
    /// but a turn north up.
    pub fn fly(&mut self, camera: &Camera, center: [f64; 2], zoom: f64, now: Instant) {
        self.fly_turning(camera, center, zoom, None, None, now);
    }

    /// Fly to `center` (world coordinates) at `zoom` as [`Motion::fly`] does, turning to
    /// `bearing` (the short way) and tilting to `pitch` (within [`MAX_PITCH`]) on the way
    /// where they're given, all in one motion. `center` should be where the camera's
    /// center goes at that bearing and pitch. A flight that turns ends any turn north up,
    /// which would fight it for the bearing.
    pub fn fly_turning(
        &mut self,
        camera: &Camera,
        center: [f64; 2],
        zoom: f64,
        bearing: Option<f64>,
        pitch: Option<f64>,
        now: Instant,
    ) {
        if bearing.is_some() {
            self.turn = None;
        }
        self.zoom = 0.0;
        self.velocity = [0.0; 2];
        self.zoom_velocity = 0.0;
        self.samples.clear();
        self.zoom_samples.clear();
        self.ease = None;
        let zoom = zoom.clamp(0.0, MAX_ZOOM);
        self.flight = Some(Flight::new(camera, center, zoom, bearing, pitch, now));
        self.last = Some(now);
    }

    /// Glide `levels` further than any zoom in progress, around `anchor`.
    pub fn zoom(&mut self, camera: &Camera, levels: f64, anchor: [f64; 2], now: Instant) {
        self.flight = None;
        self.ease = None;
        self.wake(now);
        let target = (camera.zoom + self.zoom + levels).clamp(0.0, MAX_ZOOM);
        self.zoom = target - camera.zoom;
        self.zoom_anchor = anchor;
    }

    /// Whether a flight is under way.
    pub fn flying(&self) -> bool {
        self.flight.is_some()
    }

    /// Have the flight under way head for `center` (world coordinates) at `zoom` instead,
    /// still turning and tilting to any bearing and pitch it was. Returns false if
    /// there's none.
    pub fn retarget(&mut self, center: [f64; 2], zoom: f64) -> bool {
        let Some(flight) = &mut self.flight else {
            return false;
        };
        flight.retarget(center, zoom.clamp(0.0, MAX_ZOOM));
        true
    }

    /// A pan or pinch starts at `p`: stop any fling, flight, turn or ease. Returns
    /// whether there was one.
    pub fn grab(&mut self, p: [f64; 2], now: Instant) -> bool {
        let stopped = self.velocity != [0.0; 2]
            || self.zoom_velocity != 0.0
            || self.flight.is_some()
            || self.turn.is_some()
            || self.ease.is_some();
        self.velocity = [0.0; 2];
        self.zoom_velocity = 0.0;
        self.flight = None;
        self.turn = None;
        self.ease = None;
        self.samples.clear();
        self.samples.push_back((now, p));
        self.zoom_samples.clear();
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

    /// A pinch zoomed by `levels` (none when it only panned or turned) around `anchor`.
    pub fn track_zoom(&mut self, levels: f64, anchor: [f64; 2], now: Instant) {
        let total = self.zoom_samples.back().map_or(0.0, |&(_, z, _)| z) + levels;
        self.zoom_samples.push_back((now, total, anchor));
        while self
            .zoom_samples
            .front()
            .is_some_and(|&(t, _, _)| now - t > FLING_WINDOW)
        {
            self.zoom_samples.pop_front();
        }
    }

    /// The pinch let go: zoom on at its recent speed, if it was still zooming quickly.
    pub fn release_zoom(&mut self, camera: &Camera, now: Instant) {
        let samples = std::mem::take(&mut self.zoom_samples);
        let (Some(&(t0, z0, _)), Some(&(t1, z1, anchor))) = (samples.front(), samples.back())
        else {
            return;
        };
        let duration = (t1 - t0).as_secs_f64();
        if now - t1 > FLING_PAUSE || duration <= 0.0 {
            return;
        }
        let speed = (z1 - z0) / duration;
        // Already at the limit it's heading for.
        let limited = if speed > 0.0 {
            camera.zoom >= MAX_ZOOM
        } else {
            camera.zoom <= 0.0
        };
        if speed.abs() < MIN_ZOOM_SPEED || limited {
            return;
        }
        self.wake(now);
        self.zoom_velocity = speed.clamp(-MAX_ZOOM_SPEED, MAX_ZOOM_SPEED);
        self.zoom_fling_anchor = anchor;
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

        if let Some(turn) = &self.turn
            && !turn.step(camera, now)
        {
            self.turn = None;
        }

        if let Some(ease) = &self.ease
            && !ease.step(camera, now)
        {
            self.ease = None;
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

        if self.zoom_velocity != 0.0 {
            let decay = (-dt / ZOOM_FLING_TIME).exp();
            let levels = self.zoom_velocity * ZOOM_FLING_TIME * (1.0 - decay);
            camera.zoom_around(levels, self.zoom_fling_anchor);
            self.zoom_velocity *= decay;
            let limited = camera.zoom <= 0.0 || camera.zoom >= MAX_ZOOM;
            if self.zoom_velocity.abs() < ZOOM_FLING_STOP || limited {
                self.zoom_velocity = 0.0;
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
        assert!(
            (c.zoom - before.1).abs() < 5e-3,
            "{} -> {}",
            before.1,
            c.zoom
        );
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
    fn a_turning_flight_lands_flat_and_north_up_the_short_way() {
        let mut c = camera();
        c.set_bearing(-150.0);
        c.set_pitch(45.0);
        let mut m = Motion::default();
        let t = Instant::now();
        let to = crate::camera::lng_lat_to_world(-122.3, 47.62);
        m.fly_turning(&c, to, 13.0, Some(0.0), Some(0.0), t);
        let mut now = t;
        let mut bearings = Vec::new();
        while m.step(&mut c, {
            now += Duration::from_millis(16);
            now
        }) {
            bearings.push(c.bearing);
        }
        // From -150° to 0° the short way never passes ±180°.
        assert!(
            bearings.iter().all(|b| (-150.0..=0.0).contains(b)),
            "{bearings:?}"
        );
        assert_eq!((c.bearing, c.pitch), (0.0, 0.0));
        assert!((c.zoom - 13.0).abs() < 1e-9);
        assert!((c.center[0] - to[0]).abs() < 1e-12 && (c.center[1] - to[1]).abs() < 1e-12);
    }

    #[test]
    fn a_turning_flight_in_place_takes_time_to_turn() {
        let mut c = camera();
        c.set_bearing(170.0);
        let center = c.center;
        let mut m = Motion::default();
        let t = Instant::now();
        m.fly_turning(&c, center, c.zoom, Some(0.0), None, t);
        let frames = settle(&mut m, &mut c, t);
        // Turning north up takes about twice TURN_TIME for nearly a half turn.
        assert!(frames as f64 * 0.016 > 1.8 * TURN_TIME, "{frames}");
        assert_eq!(c.bearing, 0.0);
    }

    #[test]
    fn a_retargeted_flight_keeps_turning_and_tilting() {
        let mut c = camera();
        c.set_bearing(90.0);
        c.set_pitch(60.0);
        let mut m = Motion::default();
        let t = Instant::now();
        let to = crate::camera::lng_lat_to_world(-122.35, 47.62);
        m.fly_turning(&c, to, 14.0, Some(0.0), Some(0.0), t);
        let mut now = t;
        for _ in 0..10 {
            now += Duration::from_millis(16);
            m.step(&mut c, now);
        }
        let before = (c.bearing, c.pitch);
        assert!(m.retarget([to[0], to[1] + 1e-6], 14.0));
        m.step(&mut c, now);
        // The same moment on the new path: the turn and tilt go on from where they were.
        assert!((c.bearing - before.0).abs() < 1e-9 && (c.pitch - before.1).abs() < 1e-9);
        settle(&mut m, &mut c, now);
        assert_eq!((c.bearing, c.pitch), (0.0, 0.0));
    }

    #[test]
    fn a_flight_without_targets_leaves_the_bearing_and_pitch() {
        let mut c = camera();
        c.set_bearing(30.0);
        c.set_pitch(40.0);
        let mut m = Motion::default();
        let t = Instant::now();
        m.fly(&c, crate::camera::lng_lat_to_world(-122.2, 47.7), 15.0, t);
        settle(&mut m, &mut c, t);
        assert_eq!((c.bearing, c.pitch), (30.0, 40.0));
    }

    #[test]
    fn turning_north_takes_the_bearing_from_a_turning_flight() {
        let mut c = camera();
        c.set_bearing(60.0);
        c.set_pitch(30.0);
        let mut m = Motion::default();
        let t = Instant::now();
        let to = crate::camera::lng_lat_to_world(-122.3, 47.62);
        m.fly_turning(&c, to, 13.0, Some(120.0), Some(0.0), t);
        m.step(&mut c, t + Duration::from_millis(16));
        m.turn_north(&c, Padding::default(), t + Duration::from_millis(16));
        settle(&mut m, &mut c, t + Duration::from_millis(16));
        assert!(c.bearing.abs() < 1e-9, "{}", c.bearing);
        assert_eq!(c.pitch, 0.0);
    }

    /// Pinch at `levels_per_frame` every 16 ms for `frames` frames from `t`, returning when
    /// the last update came.
    fn pinch(m: &mut Motion, t: Instant, frames: u32, levels_per_frame: f64) -> Instant {
        m.grab([400.0, 300.0], t);
        let mut now = t;
        for _ in 0..frames {
            now += Duration::from_millis(16);
            m.track_zoom(levels_per_frame, [400.0, 300.0], now);
        }
        now
    }

    #[test]
    fn a_quick_pinch_zooms_on() {
        let mut c = camera();
        let mut m = Motion::default();
        // About 3 levels a second.
        let end = pinch(&mut m, Instant::now(), 10, 0.05);
        m.release_zoom(&c, end);
        assert!(m.active());
        settle(&mut m, &mut c, end);
        // It carries on by about its speed times ZOOM_FLING_TIME.
        let carried = c.zoom - 12.0;
        assert!(carried > 0.5 && carried < 0.8, "{carried}");
    }

    #[test]
    fn a_quick_pinch_out_zooms_out() {
        let mut c = camera();
        let mut m = Motion::default();
        let end = pinch(&mut m, Instant::now(), 10, -0.05);
        m.release_zoom(&c, end);
        settle(&mut m, &mut c, end);
        assert!(c.zoom < 11.5, "{}", c.zoom);
    }

    #[test]
    fn a_slow_pinch_stops_where_it_lets_go() {
        let c = camera();
        let mut m = Motion::default();
        // Half a level a second.
        let end = pinch(&mut m, Instant::now(), 10, 0.008);
        m.release_zoom(&c, end);
        assert!(!m.active());
    }

    #[test]
    fn a_pinch_held_still_does_not_zoom_on() {
        let c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        let mut now = pinch(&mut m, t, 10, 0.05);
        // Held apart, still moving the fingers a little but not zooming.
        for _ in 0..10 {
            now += Duration::from_millis(16);
            m.track_zoom(0.0, [400.0, 300.0], now);
        }
        m.release_zoom(&c, now);
        assert!(!m.active());
    }

    #[test]
    fn a_zoom_fling_stops_at_the_limit() {
        let mut c = camera();
        c.zoom = MAX_ZOOM - 0.1;
        let mut m = Motion::default();
        let end = pinch(&mut m, Instant::now(), 10, 0.2);
        m.release_zoom(&c, end);
        let frames = settle(&mut m, &mut c, end);
        assert_eq!(c.zoom, MAX_ZOOM);
        assert!(frames < 5, "{frames} frames");
    }

    #[test]
    fn grabbing_stops_a_zoom_fling() {
        let c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        let end = pinch(&mut m, t, 10, 0.05);
        m.release_zoom(&c, end);
        assert!(m.grab([400.0, 300.0], end + Duration::from_millis(10)));
        assert!(!m.active());
    }

    #[test]
    fn turning_north_changes_only_the_bearing() {
        for pitch in [0.0, 60.0] {
            let mut c = camera();
            c.set_bearing(-150.0);
            c.set_pitch(pitch);
            let padding = Padding {
                top: 50.0,
                right: 20.0,
                bottom: 300.0,
                left: 130.0,
            };
            let middle = c.framing(padding).target;
            let mut m = Motion::default();
            let t = Instant::now();
            m.turn_north(&c, padding, t);
            m.step(&mut c, t + Duration::from_millis(100));
            // Partway, it's turning the short way, straight back toward 0, around the
            // middle of the padding.
            assert!(c.bearing > -150.0 && c.bearing < 0.0, "{}", c.bearing);
            let partway = c.framing(padding).target;
            assert!((partway[0] - middle[0]).abs() < 1e-12, "{pitch}");
            assert!((partway[1] - middle[1]).abs() < 1e-12, "{pitch}");
            let frames = settle(&mut m, &mut c, t + Duration::from_millis(100));
            assert!(frames < 40, "{frames} frames");
            assert_eq!(c.bearing, 0.0);
            assert_eq!((c.zoom, c.pitch), (12.0, pitch));
            let after = c.framing(padding).target;
            assert!((after[0] - middle[0]).abs() < 1e-12 && (after[1] - middle[1]).abs() < 1e-12);
        }
    }

    #[test]
    fn turning_north_when_north_up_does_nothing() {
        let c = camera();
        let mut m = Motion::default();
        m.turn_north(&c, Padding::default(), Instant::now());
        assert!(!m.active());
    }

    #[test]
    fn grabbing_stops_a_turn() {
        let mut c = camera();
        c.set_bearing(90.0);
        let mut m = Motion::default();
        let t = Instant::now();
        m.turn_north(&c, Padding::default(), t);
        assert!(m.grab([400.0, 300.0], t + Duration::from_millis(10)));
        assert!(!m.active());
    }

    /// `end` foresaw where `camera` came to rest, as of `then`.
    fn assert_foreseen(end: &Camera, camera: &Camera) {
        let moved = (end.center[0] - camera.center[0]).hypot(end.center[1] - camera.center[1]);
        // Within a pixel or so at the view's zoom.
        let pixel = 1.0 / (TILE_SIZE * camera.zoom.exp2());
        assert!(moved < 2.0 * pixel, "{} pixels off", moved / pixel);
        assert!(
            (end.zoom - camera.zoom).abs() < 0.01,
            "{} vs {}",
            end.zoom,
            camera.zoom
        );
        assert!((end.bearing - camera.bearing).abs() < 1e-6);
    }

    #[test]
    fn nothing_moving_has_no_end() {
        assert!(Motion::default().end(&camera()).is_none());
    }

    #[test]
    fn a_fling_ends_where_it_was_foreseen() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.grab([400.0, 300.0], t);
        for i in 1..=5 {
            let p = [400.0 - 30.0 * f64::from(i), 300.0 + 20.0 * f64::from(i)];
            m.track(p, t + Duration::from_millis(10 * i as u64));
        }
        let end = t + Duration::from_millis(50);
        m.release(&c, end);
        let foreseen = m.end(&c).unwrap();
        // Partway, it still foresees the same end.
        m.step(&mut c, end + Duration::from_millis(16));
        m.step(&mut c, end + Duration::from_millis(32));
        assert_foreseen(&m.end(&c).unwrap(), &foreseen);
        settle(&mut m, &mut c, end + Duration::from_millis(32));
        assert_foreseen(&foreseen, &c);
    }

    #[test]
    fn a_wheel_zoom_ends_where_it_was_foreseen() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.zoom(&c, 1.5, [100.0, 500.0], t);
        let foreseen = m.end(&c).unwrap();
        settle(&mut m, &mut c, t);
        assert_foreseen(&foreseen, &c);
    }

    #[test]
    fn a_zoom_fling_ends_where_it_was_foreseen() {
        let mut c = camera();
        let mut m = Motion::default();
        let end = pinch(&mut m, Instant::now(), 10, -0.05);
        m.release_zoom(&c, end);
        let foreseen = m.end(&c).unwrap();
        settle(&mut m, &mut c, end);
        assert_foreseen(&foreseen, &c);
    }

    #[test]
    fn a_flight_ends_where_it_was_foreseen() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.fly(&c, crate::camera::lng_lat_to_world(-122.2, 47.7), 15.0, t);
        let foreseen = m.end(&c).unwrap();
        settle(&mut m, &mut c, t);
        assert_foreseen(&foreseen, &c);
    }

    fn framing(lng: f64, lat: f64, zoom: f64, bearing: f64, pitch: f64) -> Framing {
        Framing {
            target: crate::camera::lng_lat_to_world(lng, lat),
            zoom,
            bearing,
            pitch,
        }
    }

    #[test]
    fn an_ease_ends_at_its_framing_turning_the_short_way() {
        let mut c = camera();
        c.set_bearing(170.0);
        let padding = Padding {
            bottom: 200.0,
            ..Padding::default()
        };
        let to = framing(-122.3, 47.62, 15.0, -160.0, 50.0);
        let mut m = Motion::default();
        let t = Instant::now();
        m.ease_to(&c, to, 0.5, Curve::Linear, padding, t);
        m.step(&mut c, t + Duration::from_millis(250));
        // Halfway, linearly: 15° on from 170 through 180.
        assert!((c.bearing - -175.0).abs() < 1e-9, "{}", c.bearing);
        assert!((c.zoom - 13.5).abs() < 1e-9 && (c.pitch - 25.0).abs() < 1e-9);
        let frames = settle(&mut m, &mut c, t + Duration::from_millis(250));
        assert!(frames <= 17, "{frames} frames");
        let shown = c.framing(padding);
        assert!((shown.target[0] - to.target[0]).abs() < 1e-12);
        assert!((shown.target[1] - to.target[1]).abs() < 1e-12);
        assert_eq!((c.zoom, c.bearing, c.pitch), (15.0, -160.0, 50.0));
    }

    #[test]
    fn an_ease_keeps_the_pitch_within_the_cap() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        let to = framing(-122.33, 47.6, 12.0, 0.0, 120.0);
        m.ease_to(&c, to, 0.2, Curve::Ease, Padding::default(), t);
        let foreseen = m.end(&c).unwrap();
        settle(&mut m, &mut c, t);
        assert_eq!(c.pitch, MAX_PITCH);
        assert_foreseen(&foreseen, &c);
    }

    #[test]
    fn an_ease_of_no_time_lands_on_the_next_step() {
        let mut c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        m.ease_to(
            &c,
            framing(-122.2, 47.7, 14.0, 10.0, 0.0),
            0.0,
            Curve::Ease,
            Padding::default(),
            t,
        );
        assert!(!m.step(&mut c, t));
        assert_eq!((c.zoom, c.bearing), (14.0, 10.0));
    }

    #[test]
    fn grabbing_or_zooming_stops_an_ease() {
        let c = camera();
        let mut m = Motion::default();
        let t = Instant::now();
        let to = framing(-122.2, 47.7, 14.0, 10.0, 0.0);
        m.ease_to(&c, to, 1.0, Curve::Ease, Padding::default(), t);
        assert!(m.grab([400.0, 300.0], t + Duration::from_millis(10)));
        assert!(!m.active());
        m.ease_to(&c, to, 1.0, Curve::Ease, Padding::default(), t);
        m.zoom(&c, 1.0, [400.0, 300.0], t);
        assert!(!m.easing());
    }

    #[test]
    fn curves_start_and_end_in_place() {
        for curve in [
            Curve::Linear,
            Curve::Ease,
            Curve::EaseIn,
            Curve::EaseOut,
            Curve::EaseInOut,
        ] {
            assert!(curve.at(0.0).abs() < 1e-6 && (curve.at(1.0) - 1.0).abs() < 1e-6);
            let mut last = 0.0;
            for i in 1..=20 {
                let k = curve.at(f64::from(i) / 20.0);
                assert!(k >= last, "{curve:?} goes back");
                last = k;
            }
        }
        assert!(Curve::EaseIn.at(0.25) < 0.25 && Curve::EaseOut.at(0.25) > 0.25);
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
