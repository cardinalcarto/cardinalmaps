//! The navigation puck: a chevron at the device's position, pointing along its course,
//! drawn by the frontend over the map rather than as a GeoJSON layer, so it can move every
//! frame. Fixes arrive about once a second; between them the puck glides from where it is
//! shown to the newest fix, so it never jumps, and the follow camera reads the same pose
//! at the same moment, so the puck holds still on screen while the map moves under it.

use std::time::Instant;

/// Where the puck is and which way it points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// World coordinates (`0..1`, `y` down).
    pub world: [f64; 2],
    /// Degrees clockwise from north.
    pub bearing: f64,
}

/// How the puck looks. The app picks the colors and size; these are a neutral default.
#[derive(Debug, Clone, PartialEq)]
pub struct PuckStyle {
    /// Straight (not premultiplied) RGBA, `0..1`.
    pub fill: [f32; 4],
    pub outline: [f32; 4],
    /// Logical pixels, inside the chevron's edge.
    pub outline_width: f32,
    /// The chevron's length from tip to tail, in logical pixels on screen, wherever it
    /// is: like other navigation pucks it keeps its size at any distance, though a tilt
    /// still foreshortens it, as it lies flat on the map.
    pub size: f32,
    /// The style layer it's drawn under; `None` (or a layer the style no longer has) is
    /// over every layer. Over everything by default, because "under the labels" would
    /// put it under more than labels: styles such as Headway draw 3D buildings after
    /// their first symbol layers, so from zoom 14 up, where navigation is, buildings
    /// would tint and hide it, and a road label could sit on it. The location dot is on
    /// top for the same reason.
    pub before_layer: Option<String>,
}

impl Default for PuckStyle {
    fn default() -> Self {
        Self {
            fill: [0.2, 0.2, 0.2, 1.0],
            outline: [1.0; 4],
            outline_width: 2.0,
            size: 28.0,
            before_layer: None,
        }
    }
}

/// The puck's pose over time: gliding from one pose to the next, linearly in time for
/// the position and the short way round for the bearing.
#[derive(Debug, Clone)]
pub struct Puck {
    from: Pose,
    to: Pose,
    start: Instant,
    /// Seconds.
    duration: f64,
}

impl Puck {
    /// A puck resting at `pose`.
    pub fn new(pose: Pose, now: Instant) -> Self {
        Self {
            from: pose,
            to: pose,
            start: now,
            duration: 0.0,
        }
    }

    /// Glide from where the puck is shown at `now` to `to` over `duration` seconds
    /// (at once for none), as a new fix arrives: one that comes before the last glide
    /// ended bends it from where it had got to, rather than jumping.
    pub fn glide_to(&mut self, to: Pose, duration: f64, now: Instant) {
        let from = self.at(now);
        // Going the short way around the world, and turning the short way round.
        let mut world = to.world;
        world[0] += (from.world[0] - world[0]).round();
        let turn = (to.bearing - from.bearing + 180.0).rem_euclid(360.0) - 180.0;
        *self = Self {
            from,
            to: Pose {
                world,
                bearing: from.bearing + turn,
            },
            start: now,
            duration: duration.max(0.0),
        };
    }

    /// The pose shown at `now`.
    pub fn at(&self, now: Instant) -> Pose {
        let t = self.progress(now);
        let lerp = |a: f64, b: f64| a + (b - a) * t;
        let (from, to) = (&self.from, &self.to);
        Pose {
            world: [
                lerp(from.world[0], to.world[0]).rem_euclid(1.0),
                lerp(from.world[1], to.world[1]),
            ],
            bearing: (lerp(from.bearing, to.bearing) + 180.0).rem_euclid(360.0) - 180.0,
        }
    }

    /// Whether the pose is still changing at `now`.
    pub fn moving(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }

    /// The share of the glide gone at `now`.
    fn progress(&self, now: Instant) -> f64 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        (now.saturating_duration_since(self.start).as_secs_f64() / self.duration).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn pose(x: f64, y: f64, bearing: f64) -> Pose {
        Pose {
            world: [x, y],
            bearing,
        }
    }

    fn close(a: Pose, b: Pose) -> bool {
        (a.world[0] - b.world[0]).abs() < 1e-12
            && (a.world[1] - b.world[1]).abs() < 1e-12
            && (a.bearing - b.bearing).abs() < 1e-9
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn it_glides_evenly_and_rests_at_the_fix() {
        let t = Instant::now();
        let mut puck = Puck::new(pose(0.2, 0.3, 0.0), t);
        puck.glide_to(pose(0.4, 0.5, 90.0), 1.0, t);
        assert!(close(puck.at(t + ms(250)), pose(0.25, 0.35, 22.5)));
        assert!(puck.moving(t + ms(999)));
        assert!(!puck.moving(t + ms(1000)));
        assert!(close(puck.at(t + ms(5000)), pose(0.4, 0.5, 90.0)));
    }

    #[test]
    fn a_fix_mid_glide_bends_it_from_where_it_is_shown() {
        let t = Instant::now();
        let mut puck = Puck::new(pose(0.2, 0.3, 0.0), t);
        puck.glide_to(pose(0.4, 0.3, 0.0), 1.0, t);
        let then = t + ms(500);
        let shown = puck.at(then);
        puck.glide_to(pose(0.4, 0.5, 0.0), 1.0, then);
        assert!(close(puck.at(then), shown), "no jump");
        assert!(close(puck.at(then + ms(500)), pose(0.35, 0.4, 0.0)));
    }

    #[test]
    fn it_turns_the_short_way_and_crosses_the_antimeridian() {
        let t = Instant::now();
        let mut puck = Puck::new(pose(0.99, 0.5, 170.0), t);
        puck.glide_to(pose(0.01, 0.5, -170.0), 1.0, t);
        let half = puck.at(t + ms(500));
        assert!(half.world[0].abs() < 1e-12 || (half.world[0] - 1.0).abs() < 1e-12);
        assert!(
            (half.bearing.abs() - 180.0).abs() < 1e-9,
            "{}",
            half.bearing
        );
        assert!(close(puck.at(t + ms(1000)), pose(0.01, 0.5, -170.0)));
    }

    #[test]
    fn a_fix_of_no_duration_jumps() {
        let t = Instant::now();
        let mut puck = Puck::new(pose(0.2, 0.3, 0.0), t);
        puck.glide_to(pose(0.4, 0.5, 45.0), 0.0, t);
        assert!(!puck.moving(t));
        assert!(close(puck.at(t), pose(0.4, 0.5, 45.0)));
    }
}
