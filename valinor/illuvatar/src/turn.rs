//! Turn degrees and turn types, as in mainline's `midgard::GetTurnDegree` and `baldr::Turn`.
//!
//! Headings come from the trip as `u32` values in `0..360`,
//! but nothing guarantees that, so the arithmetic wraps exactly as mainline's
//! unsigned 32-bit arithmetic does instead of assuming the range.

/// The turn degree from one heading to another: 90 is a perfect right turn.
///
/// This is `(((to - from) + 360) % 360)` in wrapping `u32` arithmetic,
/// which is what mainline computes.
#[inline]
pub(crate) const fn turn_degree(from_heading: u32, to_heading: u32) -> u32 {
    to_heading.wrapping_sub(from_heading).wrapping_add(360) % 360
}

/// A coarse classification of a turn degree (`baldr::Turn::Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnType {
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
    /// The turn type of a turn degree (`baldr::Turn::GetType`).
    #[inline]
    pub(crate) const fn of(turn_degree: u32) -> Self {
        match turn_degree % 360 {
            0..=10 | 350.. => Self::Straight,
            11..=44 => Self::SlightRight,
            45..=135 => Self::Right,
            136..=159 => Self::SharpRight,
            160..=200 => Self::Reverse,
            201..=224 => Self::SharpLeft,
            225..=315 => Self::Left,
            316..=349 => Self::SlightLeft,
        }
    }
}

/// Whether an intersecting edge turns within `threshold` degrees of the path,
/// on the given side (`odin::IsSimilarTurnDegree`).
#[inline]
pub(crate) const fn is_similar_turn_degree(
    path_turn_degree: u32,
    intersecting_turn_degree: u32,
    is_right: bool,
    threshold: u32,
) -> bool {
    let delta = if is_right {
        intersecting_turn_degree
            .wrapping_sub(path_turn_degree)
            .wrapping_add(360)
            % 360
    } else {
        path_turn_degree
            .wrapping_sub(intersecting_turn_degree)
            .wrapping_add(360)
            % 360
    };
    delta <= threshold
}

/// Mainline's default similarity threshold for [`is_similar_turn_degree`].
pub(crate) const SIMILAR_TURN_DEGREE_THRESHOLD: u32 = 40;

/// Whether a turn degree is within the fork-forward cone.
#[inline]
pub(crate) const fn is_fork_forward(turn_degree: u32) -> bool {
    turn_degree > 339 || turn_degree < 21
}

/// Whether a turn degree is "relatively straight".
#[inline]
pub(crate) const fn is_relative_straight(turn_degree: u32) -> bool {
    turn_degree > 329 || turn_degree < 31
}

/// Whether a turn degree is within the forward cone.
#[inline]
pub(crate) const fn is_forward(turn_degree: u32) -> bool {
    turn_degree > 314 || turn_degree < 46
}

/// Whether a turn degree is within the wider forward cone.
#[inline]
pub(crate) const fn is_wider_forward(turn_degree: u32) -> bool {
    turn_degree > 304 || turn_degree < 56
}

/// The absolute difference between two turn degrees, folded into `0..=180`.
///
/// Mainline converts to `int` before subtracting;
/// degrees above `i32::MAX` cannot come from a valid heading,
/// so we compute in `i64` to stay exact for every `u32`.
#[inline]
pub(crate) fn turn_degree_delta(path_turn_degree: u32, xedge_turn_degree: u32) -> u32 {
    let delta = (i64::from(path_turn_degree) - i64::from(xedge_turn_degree)).unsigned_abs();
    let delta = if delta > 180 {
        360_u64.wrapping_sub(delta)
    } else {
        delta
    };
    // Both inputs are turn degrees (below 360), so this fits;
    // for out-of-range inputs mainline's `int` arithmetic is not meaningful anyway.
    u32::try_from(delta).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_degree_wraps_like_mainline() {
        assert_eq!(turn_degree(0, 90), 90);
        assert_eq!(turn_degree(90, 0), 270);
        assert_eq!(turn_degree(359, 1), 2);
        assert_eq!(turn_degree(10, 10), 0);
        // Out-of-range headings: (5 - 400 + 360) wraps around 2^32, then % 360.
        assert_eq!(
            turn_degree(400, 5),
            5_u32.wrapping_sub(400).wrapping_add(360) % 360
        );
    }

    #[test]
    fn turn_types_match_the_mainline_table() {
        let expected = [
            (0, TurnType::Straight),
            (10, TurnType::Straight),
            (11, TurnType::SlightRight),
            (44, TurnType::SlightRight),
            (45, TurnType::Right),
            (135, TurnType::Right),
            (136, TurnType::SharpRight),
            (159, TurnType::SharpRight),
            (160, TurnType::Reverse),
            (200, TurnType::Reverse),
            (201, TurnType::SharpLeft),
            (224, TurnType::SharpLeft),
            (225, TurnType::Left),
            (315, TurnType::Left),
            (316, TurnType::SlightLeft),
            (349, TurnType::SlightLeft),
            (350, TurnType::Straight),
            (359, TurnType::Straight),
            (360, TurnType::Straight),
            (405, TurnType::Right),
        ];
        for (degree, turn_type) in expected {
            assert_eq!(TurnType::of(degree), turn_type, "turn degree {degree}");
        }
    }

    #[test]
    fn similar_turn_degrees() {
        assert!(is_similar_turn_degree(90, 120, true, 40));
        assert!(!is_similar_turn_degree(90, 131, true, 40));
        // Left of the path is not similar on the right.
        assert!(!is_similar_turn_degree(90, 80, true, 40));
        assert!(is_similar_turn_degree(90, 80, false, 40));
        assert!(is_similar_turn_degree(10, 350, false, 40));
    }

    #[test]
    fn turn_degree_deltas_fold() {
        assert_eq!(turn_degree_delta(10, 350), 20);
        assert_eq!(turn_degree_delta(350, 10), 20);
        assert_eq!(turn_degree_delta(90, 270), 180);
        assert_eq!(turn_degree_delta(0, 0), 0);
    }
}
