//! Easing for window input: wheel zooms that glide to their level, and flings that carry a
//! pan on after the mouse lets go. Both advance once per frame through [`Motion::step`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::camera::{Camera, MAX_ZOOM};

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
}

impl Motion {
    pub fn active(&self) -> bool {
        self.zoom != 0.0 || self.velocity != [0.0; 2]
    }

    /// Glide `levels` further than any zoom in progress, around `anchor`.
    pub fn zoom(&mut self, camera: &Camera, levels: f64, anchor: [f64; 2], now: Instant) {
        self.wake(now);
        let target = (camera.zoom + self.zoom + levels).clamp(0.0, MAX_ZOOM);
        self.zoom = target - camera.zoom;
        self.zoom_anchor = anchor;
    }

    /// A pan starts at `p`: stop any fling.
    pub fn grab(&mut self, p: [f64; 2], now: Instant) {
        self.velocity = [0.0; 2];
        self.samples.clear();
        self.samples.push_back((now, p));
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
