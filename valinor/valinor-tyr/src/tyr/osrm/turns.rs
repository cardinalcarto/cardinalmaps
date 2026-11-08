//! Turn geometry helpers: `midgard::GetTurnDegree`, `baldr::Turn::GetType`,
//! turn lane directions (`odin::turn_lane_direction` and `lane_indications`),
//! and the OSRM modifier strings (`tyr::osrmconstants`).

pub(super) const MODIFIER_STRAIGHT: &str = "straight";
pub(super) const MODIFIER_UTURN: &str = "uturn";
pub(super) const MODIFIER_SLIGHT_LEFT: &str = "slight left";
pub(super) const MODIFIER_LEFT: &str = "left";
pub(super) const MODIFIER_SHARP_LEFT: &str = "sharp left";
pub(super) const MODIFIER_SLIGHT_RIGHT: &str = "slight right";
pub(super) const MODIFIER_RIGHT: &str = "right";
pub(super) const MODIFIER_SHARP_RIGHT: &str = "sharp right";

/// `baldr::Turn::Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnType {
    Straight,
    SlightRight,
    Right,
    SharpRight,
    Reverse,
    SharpLeft,
    Left,
    SlightLeft,
}

impl TurnType {
    /// `Turn::GetType`.
    pub(super) fn from_degree(turn_degree: u32) -> Self {
        match turn_degree % 360 {
            0..=10 | 350.. => Self::Straight,
            11..=44 => Self::SlightRight,
            45..=135 => Self::Right,
            136..=159 => Self::SharpRight,
            160..=200 => Self::Reverse,
            201..=224 => Self::SharpLeft,
            225..=315 => Self::Left,
            _ => Self::SlightLeft,
        }
    }

    /// The OSRM modifier for this turn (`turn_modifier(in_brg, out_brg)`).
    pub(super) fn modifier(self) -> &'static str {
        match self {
            Self::Straight => MODIFIER_STRAIGHT,
            Self::SlightRight => MODIFIER_SLIGHT_RIGHT,
            Self::Right => MODIFIER_RIGHT,
            Self::SharpRight => MODIFIER_SHARP_RIGHT,
            Self::Reverse => MODIFIER_UTURN,
            Self::SharpLeft => MODIFIER_SHARP_LEFT,
            Self::Left => MODIFIER_LEFT,
            Self::SlightLeft => MODIFIER_SLIGHT_LEFT,
        }
    }
}

/// `GetTurnDegree`, with mainline's unsigned wraparound.
pub(super) fn turn_degree(from_heading: u32, to_heading: u32) -> u32 {
    to_heading.wrapping_sub(from_heading).wrapping_add(360) % 360
}

/// The turn type from an incoming to an outgoing heading.
pub(super) fn turn_type(in_bearing: u32, out_bearing: u32) -> TurnType {
    TurnType::from_degree(turn_degree(in_bearing, out_bearing))
}

// `baldr::kTurnLane*` direction bits.
pub(super) const TURN_LANE_EMPTY: u32 = 0;
pub(super) const TURN_LANE_NONE: u32 = 1 << 0;
const TURN_LANE_THROUGH: u32 = 1 << 1;
const TURN_LANE_SHARP_LEFT: u32 = 1 << 2;
const TURN_LANE_LEFT: u32 = 1 << 3;
const TURN_LANE_SLIGHT_LEFT: u32 = 1 << 4;
const TURN_LANE_SLIGHT_RIGHT: u32 = 1 << 5;
const TURN_LANE_RIGHT: u32 = 1 << 6;
const TURN_LANE_SHARP_RIGHT: u32 = 1 << 7;
const TURN_LANE_REVERSE: u32 = 1 << 8;

/// `odin::turn_lane_direction`: the modifier for a single direction bit, or `""`.
///
/// Mainline takes the direction as a `uint16_t`, so only the low 16 bits count.
pub(super) fn turn_lane_direction(direction: u32) -> &'static str {
    match direction & 0xFFFF {
        TURN_LANE_REVERSE => MODIFIER_UTURN,
        TURN_LANE_SHARP_LEFT => MODIFIER_SHARP_LEFT,
        TURN_LANE_LEFT => MODIFIER_LEFT,
        TURN_LANE_SLIGHT_LEFT => MODIFIER_SLIGHT_LEFT,
        TURN_LANE_THROUGH => MODIFIER_STRAIGHT,
        TURN_LANE_SLIGHT_RIGHT => MODIFIER_SLIGHT_RIGHT,
        TURN_LANE_RIGHT => MODIFIER_RIGHT,
        TURN_LANE_SHARP_RIGHT => MODIFIER_SHARP_RIGHT,
        _ => "",
    }
}

/// `lane_indications`: a lane's directions from left to right,
/// with the U-turn on the side traffic turns around on.
pub(super) fn lane_indications(drive_on_right: bool, mask: u32) -> Vec<&'static str> {
    let mut indications = Vec::new();
    if mask & TURN_LANE_REVERSE != 0 && drive_on_right {
        indications.push(MODIFIER_UTURN);
    }
    for (bit, modifier) in [
        (TURN_LANE_SHARP_LEFT, MODIFIER_SHARP_LEFT),
        (TURN_LANE_LEFT, MODIFIER_LEFT),
        (TURN_LANE_SLIGHT_LEFT, MODIFIER_SLIGHT_LEFT),
        (TURN_LANE_THROUGH, MODIFIER_STRAIGHT),
        (TURN_LANE_SLIGHT_RIGHT, MODIFIER_SLIGHT_RIGHT),
        (TURN_LANE_RIGHT, MODIFIER_RIGHT),
        (TURN_LANE_SHARP_RIGHT, MODIFIER_SHARP_RIGHT),
    ] {
        if mask & bit != 0 {
            indications.push(modifier);
        }
    }
    if mask & TURN_LANE_REVERSE != 0 && !drive_on_right {
        indications.push(MODIFIER_UTURN);
    }
    indications
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrees_and_types() {
        assert_eq!(turn_degree(20, 10), 350);
        assert_eq!(turn_degree(10, 100), 90);
        assert_eq!(turn_degree(0, 0), 0);
        assert_eq!(TurnType::from_degree(10), TurnType::Straight);
        assert_eq!(TurnType::from_degree(11), TurnType::SlightRight);
        assert_eq!(TurnType::from_degree(135), TurnType::Right);
        assert_eq!(TurnType::from_degree(159), TurnType::SharpRight);
        assert_eq!(TurnType::from_degree(200), TurnType::Reverse);
        assert_eq!(TurnType::from_degree(224), TurnType::SharpLeft);
        assert_eq!(TurnType::from_degree(315), TurnType::Left);
        assert_eq!(TurnType::from_degree(349), TurnType::SlightLeft);
        assert_eq!(TurnType::from_degree(350), TurnType::Straight);
        assert_eq!(TurnType::from_degree(450), TurnType::Right);
    }

    /// The cases of mainline's `testlaneIndications`.
    #[test]
    fn indications() {
        assert_eq!(
            lane_indications(true, TURN_LANE_REVERSE | TURN_LANE_LEFT | TURN_LANE_THROUGH),
            vec!["uturn", "left", "straight"]
        );
        assert_eq!(
            lane_indications(false, TURN_LANE_REVERSE | TURN_LANE_RIGHT),
            vec!["right", "uturn"]
        );
        assert!(lane_indications(true, TURN_LANE_EMPTY).is_empty());
        assert_eq!(turn_lane_direction(TURN_LANE_SLIGHT_RIGHT), "slight right");
        assert_eq!(turn_lane_direction(TURN_LANE_NONE), "");
        assert_eq!(turn_lane_direction(0x1_0000 | TURN_LANE_LEFT), "left");
    }
}
