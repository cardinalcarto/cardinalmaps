//! Costs, travel modes, and the constants shared by every costing
//! (mainline's `sif/costconstants.h` and the tables at the top of `sif/dynamiccost.h`).

use core::ops::{Add, AddAssign, Mul, MulAssign, Sub, SubAssign};

/// The cost of traversing something, and the time it takes (mainline's `sif::Cost`).
///
/// `cost` is what path search minimizes;
/// `secs` is the elapsed time, which costings estimate separately
/// (penalties add cost but no time).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    /// The cost, in units comparable to seconds.
    pub cost: f32,
    /// The elapsed time, in seconds.
    pub secs: f32,
}

impl Cost {
    /// No cost and no time (mainline's `kNoCost`).
    pub const ZERO: Cost = Cost::new(0.0, 0.0);

    /// A cost and an elapsed time.
    #[inline]
    pub const fn new(cost: f32, secs: f32) -> Self {
        Self { cost, secs }
    }
}

impl Add for Cost {
    type Output = Cost;

    #[inline]
    fn add(self, other: Cost) -> Cost {
        Cost::new(self.cost + other.cost, self.secs + other.secs)
    }
}

impl AddAssign for Cost {
    #[inline]
    fn add_assign(&mut self, other: Cost) {
        self.cost += other.cost;
        self.secs += other.secs;
    }
}

impl Sub for Cost {
    type Output = Cost;

    #[inline]
    fn sub(self, other: Cost) -> Cost {
        Cost::new(self.cost - other.cost, self.secs - other.secs)
    }
}

impl SubAssign for Cost {
    #[inline]
    fn sub_assign(&mut self, other: Cost) {
        self.cost -= other.cost;
        self.secs -= other.secs;
    }
}

impl Mul<f32> for Cost {
    type Output = Cost;

    #[inline]
    fn mul(self, factor: f32) -> Cost {
        Cost::new(self.cost * factor, self.secs * factor)
    }
}

impl MulAssign<f32> for Cost {
    #[inline]
    fn mul_assign(&mut self, factor: f32) {
        self.cost *= factor;
        self.secs *= factor;
    }
}

/// How a costing travels (mainline's `sif::TravelMode`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum TravelMode {
    #[default]
    Drive = 0,
    Pedestrian = 1,
    Bicycle = 2,
    PublicTransit = 3,
}

/// A turn onto a short edge inside an intersection (mainline's `sif::InternalTurn`).
///
/// Thor records this on a label so that a U-turn made across two short internal edges
/// can be penalized like one made at a single node.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum InternalTurn {
    #[default]
    NoTurn = 0,
    LeftTurn = 1,
    RightTurn = 2,
}

/// Internal edges at most this long (in meters) count as short (`kShortInternalLength`).
pub const SHORT_INTERNAL_LENGTH: u32 = 8;

/// Seconds per hour (`midgard::kSecPerHour`).
pub(crate) const SEC_PER_HOUR: f32 = 3600.0;

/// The fastest speed mainline allows, in km/h (`kMaxSpeedKph`).
pub(crate) const MAX_SPEED_KPH: u32 = 252;

/// The fastest ferry speed, in km/h (`kMaxFerrySpeedKph`).
pub(crate) const MAX_FERRY_SPEED_KPH: u32 = 40;

/// A `fixed_speed` of 0 means no fixed speed (`kDisableFixedSpeed`).
pub(crate) const DISABLE_FIXED_SPEED: u32 = 0;

/// The largest weighted grade (`kMaxGradeFactor`).
pub(crate) const MAX_GRADE_FACTOR: usize = 15;

/// Seconds per meter at each speed in km/h (`kSpeedFactor`).
///
/// A speed of 0 gets an hour per meter (mainline's "TODO - what to make speed=0?").
pub(crate) static SPEED_FACTOR: [f32; 253] = {
    let mut factors = [0.0; 253];
    factors[0] = SEC_PER_HOUR;
    let mut s = 1;
    while s < factors.len() {
        #[expect(clippy::cast_precision_loss, reason = "speeds are at most 252")]
        let speed = s as f32;
        factors[s] = (SEC_PER_HOUR * 0.001) / speed;
        s += 1;
    }
    factors
};

/// Seconds per meter at a speed, saturating at the table's largest speed.
///
/// Mainline indexes `kSpeedFactor` without a check;
/// speeds come from tiles and options that keep them in range.
#[inline]
pub(crate) fn speed_factor(kph: u32) -> f32 {
    let index =
        usize::try_from(kph).map_or(SPEED_FACTOR.len() - 1, |i| i.min(SPEED_FACTOR.len() - 1));
    SPEED_FACTOR[index]
}

/// Edge cost factors by road density (`kDensityFactor`).
pub(crate) static DENSITY_FACTOR: [f32; 16] = {
    let mut factors = [0.0; 16];
    let mut d = 0;
    while d < factors.len() {
        #[expect(clippy::cast_precision_loss, reason = "densities are at most 15")]
        let density = d as f32;
        factors[d] = 0.85 + density * 0.025;
        d += 1;
    }
    factors
};

/// Transition cost factors by node density (`kTransDensityFactor`).
pub(crate) const TRANS_DENSITY_FACTOR: [f32; 16] = [
    1.0, 1.0, 1.0, 1.0, 1.0, 1.1, 1.2, 1.3, 1.4, 1.6, 1.9, 2.2, 2.5, 2.8, 3.1, 3.5,
];

/// Converts length into time-like units for `use_distance` (`kInvMedianSpeed`).
pub(crate) const INV_MEDIAN_SPEED: f32 = 1.0 / 16.0;

/// The ferry penalty at `use_ferry` 0 (`kMaxFerryPenalty`).
pub(crate) const MAX_FERRY_PENALTY: f32 = 6.0 * SEC_PER_HOUR;

/// U-turn penalties (`kTCUnfavorablePencilPointUturn` and friends).
pub(crate) const TC_UNFAVORABLE_PENCIL_POINT_UTURN: f32 = 15.0;
pub(crate) const TC_UNFAVORABLE_UTURN: f32 = 600.0;
pub(crate) const TC_NAME_INCONSISTENT_UTURN: f32 = 10.0;

/// The largest highway avoidance factor (`kMaxHighwayBiasFactor`).
pub(crate) const MAX_HIGHWAY_BIAS_FACTOR: f32 = 8.0;

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "the tables must match mainline exactly")]
mod tests {
    use super::*;

    #[test]
    fn tables_match_mainline() {
        // Spot checks against values printed by mainline's sif helper.
        assert_eq!(SPEED_FACTOR[0], 3600.0);
        assert_eq!(SPEED_FACTOR[1], 3600.0 * 0.001);
        assert_eq!(SPEED_FACTOR[36], (3600.0f32 * 0.001) / 36.0);
        assert_eq!(speed_factor(1000), SPEED_FACTOR[252]);
        assert_eq!(DENSITY_FACTOR[0], 0.85);
        assert_eq!(DENSITY_FACTOR[15], 0.85 + 15.0 * 0.025);
    }

    #[test]
    fn cost_arithmetic() {
        let mut c = Cost::new(1.0, 2.0) + Cost::new(3.0, 4.0);
        assert_eq!(c, Cost::new(4.0, 6.0));
        c *= 0.5;
        assert_eq!(c, Cost::new(2.0, 3.0));
        c -= Cost::new(1.0, 1.0);
        assert_eq!(c - Cost::new(1.0, 2.0), Cost::ZERO);
        assert_eq!(Cost::new(1.0, 1.0) * 3.0, Cost::new(3.0, 3.0));
    }
}
