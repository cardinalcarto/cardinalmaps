//! # Predicted speeds
//!
//! Speeds that vary over the week, and how to pick the speed of an edge at a given time.
//!
//! A Valhalla tile can carry a speed profile for each directed edge:
//! the average speed in every 5-minute bucket of the week (2016 buckets).
//! That's a lot of data, so the profile is compressed by truncating its DCT-II
//! to the first 200 coefficients,
//! and decoded one bucket at a time with the inverse transform (DCT-III).
//!
//! The term "predicted speed" is Valhalla's.
//! It's perhaps more accurate to think of these as historical average speeds,
//! which are useful for prediction as long as there isn't a special event,
//! temporary lane closure, etc.
//!
//! ## Time of week
//!
//! Times are seconds from the start of the week, Sunday 00:00, in the edge's *local* time.
//! (Mainline's `GraphTile::GetSpeed` docs say Monday,
//! but `DateTime::second_of_week`, which produces these values, counts from Sunday.)
//! Like `GetSpeed`, everything here wraps times modulo a week,
//! so a time of exactly one week (which mainline's `TimeInfo::forward` can produce)
//! is Sunday 00:00 again.
//!
//! ## Matching mainline
//!
//! Decoding and encoding follow mainline's `predictedspeeds.cc` operation for operation:
//! the same table of `cosf` values (computed in `f32` the same way),
//! the same summation order, normalization applied in the same places,
//! and no fused multiply-adds.
//! On Linux with glibc, the results are bit for bit identical to mainline 3.9.1
//! (the tests check this against values from mainline's own Python bindings).
//!
//! Elsewhere, results can differ from Linux mainline in the last bits of an `f32`,
//! exactly as mainline compiled for that platform would:
//! Rust's `f32::cos` calls the platform's `cosf`, and those differ by an ulp here and there
//! (about 1% of table entries differ from a correctly rounded cosine on glibc, for example).
//! Mainline built with floating-point contraction (such as clang on aarch64)
//! can also differ, since it may fuse the multiply-adds that Rust never fuses.
//! Either way, the effect on a routing speed is a change of one kph
//! when a decoded speed lies within a few millionths of a rounding boundary.
//!
//! ## Speed selection
//!
//! [`select_speed`] picks the speed for an edge at a time the way mainline's
//! `GraphTile::GetSpeed` does, given which sources a costing model allows.
//!
//! This code is ported from [Valhalla](https://github.com/valhalla/valhalla)'s baldr module
//! (`predictedspeeds.cc`, `predictedspeeds.h`, and `graphtile.h`).
//! The original code is available under the MIT license.
use crate::graph_tile::DirectedEdge;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use enumset::{EnumSet, EnumSetType};
use std::sync::LazyLock;
use thiserror::Error;
use zerocopy::{I16, LE, U32};

/// The size of each speed bucket, in minutes.
///
/// The week is broken into fixed-size buckets.
/// 5 minutes is hard-coded in Valhalla.
pub const SPEED_BUCKET_SIZE_MINUTES: u32 = 5;
/// The size of each speed bucket, in seconds.
pub const SPEED_BUCKET_SIZE_SECONDS: u32 = SPEED_BUCKET_SIZE_MINUTES * 60;
/// The number of seconds in a week.
pub const SECONDS_PER_WEEK: u32 = 7 * 24 * 60 * 60;
/// The number of speed buckets in a week (2016).
pub const BUCKETS_PER_WEEK: usize = (SECONDS_PER_WEEK / SPEED_BUCKET_SIZE_SECONDS) as usize;
/// [`BUCKETS_PER_WEEK`] as the type bucket indices are stored in.
const BUCKETS_PER_WEEK_U16: u16 = 2016;
const _: () = assert!(BUCKETS_PER_WEEK_U16 as usize == BUCKETS_PER_WEEK);

/// The number of DCT coefficients kept for each speed profile.
pub const COEFFICIENT_COUNT: usize = 200;

/// The number of bytes in a decoded base64 speed profile.
/// Each coefficient (`i16`) is encoded as 2 bytes in big-endian order.
const DECODED_SPEED_SIZE: usize = 2 * COEFFICIENT_COUNT;

/// Mainline's sentinel for "no time" (`kInvalidSecondsOfWeek`).
///
/// It's the largest value of the 20-bit field mainline stores times of week in.
/// [`select_speed`] treats it the same as no time at all,
/// since that's what mainline does with it.
pub const INVALID_SECONDS_OF_WEEK: u64 = (1 << 20) - 1;

/// Mainline's `kMinValidSpeedKph`.
/// Free and constrained flow speeds below this mean "no data".
const MIN_VALID_SPEED_KPH: u8 = 1;

/// Constrained flow speeds apply strictly between 07:00 and 19:00 local time
/// (mainline's `GetSpeed`).
const DAYTIME_START_SECONDS: u64 = 7 * 60 * 60;
const DAYTIME_END_SECONDS: u64 = 19 * 60 * 60;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

// The DCT constants from mainline's predictedspeeds.cc.
// Mainline spells these as decimal literals;
// the tests check that each of these is the same f32.

/// `k1OverSqrt2` (`0.707106781f`).
const ONE_OVER_SQRT_2: f32 = core::f32::consts::FRAC_1_SQRT_2;
/// `kPiBucketConstant` (`3.14159265f / 2016.0f`).
const PI_BUCKET_CONSTANT: f32 = core::f32::consts::PI / 2016.0;
/// `kSpeedNormalization` (`0.031497039f`, which is `sqrt(2 / 2016)`).
#[expect(
    clippy::excessive_precision,
    reason = "spelled as mainline spells it, to make the correspondence obvious"
)]
const SPEED_NORMALIZATION: f32 = 0.031_497_039;

/// The cosine table, one row of [`COEFFICIENT_COUNT`] values per bucket of the week.
///
/// This is mainline's `BucketCosTable`: entry `[bucket][c]` is
/// `cosf(kPiBucketConstant * (bucket + 0.5f) * c)`, evaluated in `f32` in that order.
/// It takes about 1.6 MB, like mainline's, and is built on first use.
static COS_TABLE: LazyLock<Box<[[f32; COEFFICIENT_COUNT]]>> = LazyLock::new(|| {
    (0..BUCKETS_PER_WEEK_U16)
        .map(|bucket| {
            let center = f32::from(bucket) + 0.5;
            let mut row = [0f32; COEFFICIENT_COUNT];
            for (c, value) in (0u8..).zip(row.iter_mut()) {
                *value = (PI_BUCKET_CONSTANT * center * f32::from(c)).cos();
            }
            row
        })
        .collect()
});

/// Gets the row of the cosine table for a bucket.
#[inline]
fn cos_row(bucket: SpeedBucket) -> &'static [f32; COEFFICIENT_COUNT] {
    // A SpeedBucket is always less than BUCKETS_PER_WEEK,
    // and the table has exactly BUCKETS_PER_WEEK rows, so this can't go out of bounds.
    &COS_TABLE[bucket.index()]
}

/// A 5-minute bucket of the week.
///
/// Bucket 0 starts on Sunday at 00:00 local time.
/// This is always less than [`BUCKETS_PER_WEEK`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpeedBucket(u16);

impl SpeedBucket {
    /// Gets the bucket with the given index,
    /// or `None` if the index is not less than [`BUCKETS_PER_WEEK`].
    #[inline]
    pub fn new(index: usize) -> Option<Self> {
        u16::try_from(index)
            .ok()
            .filter(|&index| index < BUCKETS_PER_WEEK_U16)
            .map(Self)
    }

    /// Gets the bucket containing a time of week, in seconds from Sunday 00:00 local time.
    ///
    /// Times of a week or more wrap around, as in mainline's `GraphTile::GetSpeed`.
    #[inline]
    pub fn from_second_of_week(seconds: u64) -> Self {
        let bucket = (seconds % u64::from(SECONDS_PER_WEEK)) / u64::from(SPEED_BUCKET_SIZE_SECONDS);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the bucket is less than BUCKETS_PER_WEEK, which fits in a u16"
        )]
        Self(bucket as u16)
    }

    /// The index of this bucket in the week (0 to 2015).
    #[inline]
    pub fn index(self) -> usize {
        usize::from(self.0)
    }

    /// Every bucket of the week, in order.
    #[inline]
    pub fn all() -> impl Iterator<Item = Self> {
        (0..BUCKETS_PER_WEEK_U16).map(Self)
    }
}

/// An error decoding a base64 speed profile.
#[derive(Debug, Error)]
pub enum PredictedSpeedCodecError {
    #[error("decode base64 speed coefficients: {0}")]
    Base64DecodeError(#[from] base64::DecodeError),
    #[error("decode speed coefficients: found {count} bytes; expected {DECODED_SPEED_SIZE}")]
    IncorrectByteCount { count: usize },
}

/// An error reading a speed profile from a tile.
///
/// Either means the tile is corrupt or the caller asked for an edge the tile doesn't have.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PredictedSpeedLookupError {
    #[error(
        "look up the speed profile of directed edge {index}: the tile has {edge_count} directed edges"
    )]
    EdgeIndexOutOfRange { index: usize, edge_count: usize },
    #[error(
        "read the speed profile at coefficient {offset}: the tile has only {available} profile coefficients"
    )]
    ProfileOutOfRange { offset: u32, available: usize },
}

/// Compresses a full week of speed buckets by truncating its DCT-II.
///
/// Speeds are in kilometers per hour, one per [`SpeedBucket`].
/// This is mainline's `compress_speed_buckets`.
///
/// Coefficients are rounded half away from zero.
/// Mainline leaves a coefficient outside the range of `i16` undefined
/// (which takes speeds far above any real road, around 700 kph sustained over the week);
/// here such a coefficient saturates, and a NaN speed makes coefficients zero.
///
/// # Performance
///
/// This does 403,200 multiply-adds.
/// The first call also builds the 1.6 MB cosine table.
#[inline]
pub fn compress_speed_buckets(speeds: &[f32; BUCKETS_PER_WEEK]) -> [i16; COEFFICIENT_COUNT] {
    let mut coefficients = [0f32; COEFFICIENT_COUNT];
    // DCT-II, accumulated bucket by bucket as mainline does.
    for (row, &speed) in COS_TABLE.iter().zip(speeds) {
        for (coefficient, &cos) in coefficients.iter_mut().zip(row) {
            *coefficient += cos * speed;
        }
    }
    let [dc, ..] = &mut coefficients;
    *dc *= ONE_OVER_SQRT_2;

    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is rounded, and float to int casts saturate"
    )]
    coefficients.map(|coefficient| (SPEED_NORMALIZATION * coefficient).round() as i16)
}

/// Recovers one bucket's speed from a compressed profile (DCT-III).
///
/// Returns a speed in kilometers per hour.
/// This is mainline's `decompress_speed_bucket`.
/// The result can be slightly negative, or 0, for slow or erratic profiles;
/// [`select_speed`] rounds it the way routing uses it.
///
/// # Performance
///
/// This is 200 dependent multiply-adds.
/// They aren't vectorized, since reassociating the sum would change the result
/// relative to mainline.
#[inline]
pub fn decompress_speed_bucket(
    coefficients: &[i16; COEFFICIENT_COUNT],
    bucket: SpeedBucket,
) -> f32 {
    dct_iii(coefficients, bucket)
}

/// The DCT-III behind [`decompress_speed_bucket`],
/// generic so it can read little-endian tile data in place.
#[inline]
fn dct_iii<T: Copy + Into<i16>>(coefficients: &[T; COEFFICIENT_COUNT], bucket: SpeedBucket) -> f32 {
    let [first, rest @ ..] = coefficients;
    let [_, cos_rest @ ..] = cos_row(bucket);
    let mut speed = f32::from((*first).into()) * ONE_OVER_SQRT_2;
    for (&coefficient, &cos) in rest.iter().zip(cos_rest) {
        speed += f32::from(coefficient.into()) * cos;
    }
    speed * SPEED_NORMALIZATION
}

/// Packs compressed speed coefficients into a base64 string.
///
/// Each coefficient is serialized big-endian, as mainline's `encode_compressed_speeds` does.
#[inline]
pub fn encode_compressed_speeds(coefficients: &[i16; COEFFICIENT_COUNT]) -> String {
    let mut raw = [0u8; DECODED_SPEED_SIZE];
    for (bytes, coefficient) in raw.as_chunks_mut::<2>().0.iter_mut().zip(coefficients) {
        *bytes = coefficient.to_be_bytes();
    }
    STANDARD.encode(raw)
}

/// Decodes a base64 string into compressed speed coefficients.
///
/// This is mainline's `decode_compressed_speeds`.
///
/// # Errors
///
/// Fails if the string isn't valid base64 (with padding)
/// or doesn't decode to exactly 400 bytes.
#[inline]
pub fn decode_base64_speed_coefficients(
    encoded: &str,
) -> Result<[i16; COEFFICIENT_COUNT], PredictedSpeedCodecError> {
    let raw = STANDARD.decode(encoded.as_bytes())?;
    if raw.len() != DECODED_SPEED_SIZE {
        return Err(PredictedSpeedCodecError::IncorrectByteCount { count: raw.len() });
    }
    let mut coefficients = [0i16; COEFFICIENT_COUNT];
    for (coefficient, bytes) in coefficients.iter_mut().zip(raw.as_chunks::<2>().0) {
        *coefficient = i16::from_be_bytes(*bytes);
    }
    Ok(coefficients)
}

/// Averages speed profiles directly in the compressed domain.
///
/// Returns `None` if there are no profiles to average.
/// This is a Valinor addition; mainline has no equivalent.
///
/// # Performance
///
/// There is no intermediate decode step:
/// the DCT is linear, so the mean of the coefficients is the coefficients of the mean.
///
/// # Accuracy
///
/// The dominant source of error in Valhalla predicted traffic is the DCT-II compression itself,
/// which lossily compresses 2016 speed buckets into 200 low-frequency coefficients and therefore
/// smooths sharp or sudden changes.
///
/// This function does not introduce any additional structural loss beyond that compression.
/// It computes the coefficient-space mean of the already-quantized profiles and performs a
/// single final quantization step.
///
/// Compared to averaging fully decoded profiles and then recompressing them, this approach
/// is at least as accurate and typically more accurate, since it avoids an extra
/// compress–decompress cycle.
///
/// The only additional error is from integer division (truncating toward zero)
/// when computing the mean coefficients;
/// this is less than 1 least-significant unit per coefficient.
/// In practice, this translates to sub-kph differences in reconstructed speeds.
#[inline]
pub fn average_compressed_speeds(
    profiles: &[[i16; COEFFICIENT_COUNT]],
) -> Option<[i16; COEFFICIENT_COUNT]> {
    // An i64 sum can't overflow: that would take more than 2^48 profiles,
    // which is far more than fit in memory.
    let count = i64::try_from(profiles.len())
        .ok()
        .filter(|&count| count > 0)?;
    let mut sums = [0i64; COEFFICIENT_COUNT];
    for profile in profiles {
        for (sum, &coefficient) in sums.iter_mut().zip(profile) {
            *sum += i64::from(coefficient);
        }
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the mean of i16 values, truncated toward zero, is an i16"
    )]
    Some(sums.map(|sum| (sum / count) as i16))
}

/// Read access to the speed profiles stored in a tile.
///
/// The tile stores one offset per directed edge,
/// counted in coefficients (not bytes) from the start of the profiles,
/// and the profiles themselves, [`COEFFICIENT_COUNT`] coefficients each.
/// Only edges whose [`DirectedEdge::has_predicted_speed`] is set have a meaningful offset;
/// the others are typically 0, so reading them gives *some* profile, not an error.
/// Callers must check the flag (as [`select_speed`] does).
///
/// Lookups never panic: an edge index or offset beyond the data is an error.
#[derive(Debug, Clone)]
pub(crate) struct PredictedSpeeds<'a> {
    /// The coefficient offset of each directed edge's profile in `profiles`.
    offsets: &'a [U32<LE>],
    /// The profile coefficients, back to back.
    profiles: &'a [I16<LE>],
}

impl<'a> PredictedSpeeds<'a> {
    pub fn new(offsets: &'a [U32<LE>], profiles: &'a [I16<LE>]) -> Self {
        Self { offsets, profiles }
    }

    /// Gets the compressed profile of a directed edge.
    ///
    /// Like mainline, this reads [`COEFFICIENT_COUNT`] coefficients from the edge's offset,
    /// which needn't be a multiple of [`COEFFICIENT_COUNT`].
    ///
    /// # Errors
    ///
    /// Fails if the edge index has no offset
    /// or the offset leaves fewer than [`COEFFICIENT_COUNT`] coefficients.
    #[inline]
    pub fn coefficients(
        &self,
        directed_edge_index: usize,
    ) -> Result<&'a [I16<LE>; COEFFICIENT_COUNT], PredictedSpeedLookupError> {
        let offset = self
            .offsets
            .get(directed_edge_index)
            .ok_or(PredictedSpeedLookupError::EdgeIndexOutOfRange {
                index: directed_edge_index,
                edge_count: self.offsets.len(),
            })?
            .get();
        usize::try_from(offset)
            .ok()
            .and_then(|start| self.profiles.get(start..))
            .and_then(<[I16<LE>]>::first_chunk::<COEFFICIENT_COUNT>)
            .ok_or(PredictedSpeedLookupError::ProfileOutOfRange {
                offset,
                available: self.profiles.len(),
            })
    }

    /// Gets the predicted speed (kph) of a directed edge at a time of week.
    ///
    /// `seconds_of_week` counts from Sunday 00:00 local time and wraps modulo a week.
    /// This is mainline's `PredictedSpeeds::speed`, with the wrapping `GetSpeed` applies first.
    ///
    /// # Errors
    ///
    /// See [`PredictedSpeeds::coefficients`].
    #[inline]
    pub fn try_speed(
        &self,
        directed_edge_index: usize,
        seconds_of_week: u64,
    ) -> Result<f32, PredictedSpeedLookupError> {
        let coefficients = self.coefficients(directed_edge_index)?;
        Ok(dct_iii(
            coefficients,
            SpeedBucket::from_second_of_week(seconds_of_week),
        ))
    }

    /// Like [`PredictedSpeeds::try_speed`], but returns `None` on errors.
    #[inline]
    pub fn speed(&self, directed_edge_index: usize, seconds_of_week: u32) -> Option<f32> {
        self.try_speed(directed_edge_index, u64::from(seconds_of_week))
            .ok()
    }

    /// Returns the raw borrowed slices for the offsets and profiles (in order).
    ///
    /// This is for internal use by the builder, which provides a sane interface
    /// for creating / manipulating predicted speed data at the edge level.
    pub(crate) fn as_offsets_and_profiles(&self) -> (&'a [U32<LE>], &'a [I16<LE>]) {
        (self.offsets, self.profiles)
    }
}

/// A source of edge speeds.
///
/// A set of these is a flow mask, saying which sources a costing model may use
/// or which ones a speed came from.
/// As an [`EnumSet`], the bits are the same as mainline's masks
/// ([`EnumSet::as_repr`] gives mainline's value).
#[derive(Debug, EnumSetType)]
#[enumset(repr = "u8")]
pub enum FlowSource {
    /// The edge's free flow speed (`kFreeFlowMask`, 1),
    /// a typical speed at night.
    FreeFlow,
    /// The edge's constrained flow speed (`kConstrainedFlowMask`, 2),
    /// a typical speed during the day.
    ConstrainedFlow,
    /// The edge's speed profile over the week (`kPredictedFlowMask`, 4).
    Predicted,
    /// Live traffic (`kCurrentFlowMask`, 8).
    ///
    /// Valinor doesn't read live traffic.
    /// This flag may be in a mask (mainline's default mask has it),
    /// but speeds are picked as mainline picks them with no live traffic tile,
    /// so it never contributes.
    Current,
}

/// Every flow source, mainline's `kDefaultFlowMask` (15),
/// which costing models use unless a request says otherwise.
pub const DEFAULT_FLOW_MASK: EnumSet<FlowSource> = EnumSet::all();

/// The speed [`select_speed`] picked for an edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SelectedSpeed {
    /// The speed, in kph.
    pub kph: u32,
    /// Where the speed came from: one source, or none for the edge's default speed.
    /// This is mainline's `flow_sources`.
    pub flow_sources: EnumSet<FlowSource>,
}

/// Picks the speed of an edge at a time of week, as mainline's `GraphTile::GetSpeed` does.
///
/// * `flow_mask` is the set of sources the costing model allows.
/// * `seconds_of_week` is the local time the edge is traversed,
///   in seconds from Sunday 00:00, or `None` for no time.
///   [`INVALID_SECONDS_OF_WEEK`] also means no time, as in mainline.
///   Times wrap modulo a week.
/// * `is_truck` caps the default speed at the edge's truck speed, if it has one.
/// * `predicted_speed` decodes the edge's predicted speed at a time of week
///   (in seconds, already wrapped to less than a week).
///   It's only called if the edge [has a profile](DirectedEdge::has_predicted_speed).
///
/// The rules, in order:
///
/// 1. With a time, [`FlowSource::Predicted`] allowed, and a profile:
///    the predicted speed, raised to at least 0.5 and rounded half up
///    (`(uint32_t)(max(speed, 0.5f) + 0.5f)`).
/// 2. With no time, or a time strictly between 07:00 and 19:00
///    (the time of week modulo a day), [`FlowSource::ConstrainedFlow`] allowed,
///    and a constrained flow speed of at least 1 kph: the constrained flow speed.
/// 3. With no time, or a time outside that window, [`FlowSource::FreeFlow`] allowed,
///    and a free flow speed of at least 1 kph: the free flow speed.
/// 4. Otherwise, the edge's speed, capped at its truck speed when `is_truck` is set
///    and the truck speed is nonzero.
///
/// So with no time, the constrained flow speed wins over the free flow speed.
///
/// Mainline blends in live traffic first when [`FlowSource::Current`] is allowed,
/// a time is given, and the tile has live traffic.
/// With no live traffic, its blend reduces exactly to the rules above,
/// which is what this implements.
///
/// # Errors
///
/// Passes on any error from `predicted_speed`.
#[inline]
pub fn select_speed<E>(
    edge: &DirectedEdge,
    flow_mask: EnumSet<FlowSource>,
    seconds_of_week: Option<u64>,
    is_truck: bool,
    predicted_speed: impl FnOnce(u32) -> Result<f32, E>,
) -> Result<SelectedSpeed, E> {
    let time = seconds_of_week.filter(|&seconds| seconds != INVALID_SECONDS_OF_WEEK);

    if let Some(seconds) = time
        && flow_mask.contains(FlowSource::Predicted)
        && edge.has_predicted_speed()
    {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the time modulo a week fits in a u32"
        )]
        let seconds_of_week = (seconds % u64::from(SECONDS_PER_WEEK)) as u32;
        let speed = predicted_speed(seconds_of_week)?;
        return Ok(SelectedSpeed {
            kph: round_predicted_speed(speed),
            flow_sources: FlowSource::Predicted.into(),
        });
    }

    let is_daytime = time.is_some_and(|seconds| {
        let second_of_day = seconds % SECONDS_PER_DAY;
        DAYTIME_START_SECONDS < second_of_day && second_of_day < DAYTIME_END_SECONDS
    });

    if (time.is_none() || is_daytime)
        && flow_mask.contains(FlowSource::ConstrainedFlow)
        && edge.constrained_flow_speed() >= MIN_VALID_SPEED_KPH
    {
        return Ok(SelectedSpeed {
            kph: u32::from(edge.constrained_flow_speed()),
            flow_sources: FlowSource::ConstrainedFlow.into(),
        });
    }

    if (time.is_none() || !is_daytime)
        && flow_mask.contains(FlowSource::FreeFlow)
        && edge.free_flow_speed() >= MIN_VALID_SPEED_KPH
    {
        return Ok(SelectedSpeed {
            kph: u32::from(edge.free_flow_speed()),
            flow_sources: FlowSource::FreeFlow.into(),
        });
    }

    let speed = edge.speed();
    let kph = if is_truck && edge.truck_speed() > 0 {
        edge.truck_speed().min(speed)
    } else {
        speed
    };
    Ok(SelectedSpeed {
        kph: u32::from(kph),
        flow_sources: EnumSet::empty(),
    })
}

/// Rounds a decoded predicted speed the way mainline's `GetSpeed` does:
/// `static_cast<uint32_t>(std::max(speed, 0.5f) + 0.5f)`.
#[inline]
fn round_predicted_speed(speed: f32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is at least 1, and float to int casts saturate"
    )]
    let kph = (speed.max(0.5) + 0.5) as u32;
    kph
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small index as an `f32`.
    fn index_f32(index: usize) -> f32 {
        f32::from(u16::try_from(index).expect("small index"))
    }

    /// Decodes every bucket of the week.
    fn decode_all(coefficients: &[i16; COEFFICIENT_COUNT]) -> Vec<f32> {
        SpeedBucket::all()
            .map(|bucket| decompress_speed_bucket(coefficients, bucket))
            .collect()
    }

    /// Builds a single-edge profile view over coefficients.
    fn single_profile(coefficients: &[i16; COEFFICIENT_COUNT]) -> Vec<I16<LE>> {
        coefficients.map(I16::new).to_vec()
    }

    /// The time of week at the start of a bucket.
    fn bucket_start(index: usize) -> u32 {
        u32::try_from(index).expect("small index") * SPEED_BUCKET_SIZE_SECONDS
    }

    fn speeds_from_vec(speeds: Vec<f32>) -> Box<[f32; BUCKETS_PER_WEEK]> {
        speeds.into_boxed_slice().try_into().expect("exact length")
    }

    // Not run under Miri, since the trig ops are REALLY slow.
    #[cfg(not(miri))]
    proptest::proptest! {
        /// For arbitrary non-negative weekly speeds,
        /// the decoded speeds don't go far below zero.
        #[test]
        fn prop_non_negative_after_decode(
            // Generate 2016 bucket values in a reasonable kph range.
            speeds in proptest::collection::vec(0.0f32..250.0f32, BUCKETS_PER_WEEK)
        ) {
            let coefficients = compress_speed_buckets(&speeds_from_vec(speeds));
            for (i, s) in decode_all(&coefficients).into_iter().enumerate() {
                // These can be preeeety negative...
                proptest::prop_assert!(s >= -5.0, "negative speed at bucket {i}: {s}");
            }
        }

        #[test]
        fn prop_averaging_in_compressed_domain_white_noise(
            speeds1 in proptest::collection::vec(1.0f32..250.0f32, BUCKETS_PER_WEEK),
            speeds2 in proptest::collection::vec(1.0f32..250.0f32, BUCKETS_PER_WEEK),
            speeds3 in proptest::collection::vec(1.0f32..250.0f32, BUCKETS_PER_WEEK)
        ) {
            let coeffs1 = compress_speed_buckets(&speeds_from_vec(speeds1));
            let coeffs2 = compress_speed_buckets(&speeds_from_vec(speeds2));
            let coeffs3 = compress_speed_buckets(&speeds_from_vec(speeds3));

            // Method 1: Decompress first; then average the decompressed speeds
            // NB: Since our test contains essentially random speeds, we can't accurately compare
            // with the true average, since the compression is so lossy.
            // This approach tests the actual property which is "if we decompressed the speeds
            // and then averaged those values, is that the same as averaging the coefficients directly."
            let (b1, b2, b3) = (decode_all(&coeffs1), decode_all(&coeffs2), decode_all(&coeffs3));

            // Method 2: Average directly in the compressed domain
            let avg_coeffs = average_compressed_speeds(&[coeffs1, coeffs2, coeffs3])
                .expect("at least one profile");
            let avg_buckets_compressed = decode_all(&avg_coeffs);

            for (i, compressed) in avg_buckets_compressed.into_iter().enumerate() {
                let direct = (b1[i] + b2[i] + b3[i]) / 3.0;
                let diff = (direct - compressed).abs();
                proptest::prop_assert!(
                    diff <= 1.0,
                    "Bucket {i}: direct average = {direct}, compressed average = {compressed}, diff = {diff}"
                );
            }
        }

        /// For a single speed profile, averaging should be a no-op (round-trip unchanged).
        #[test]
        fn prop_average_single_profile_unchanged(
            speeds in proptest::collection::vec(0.0f32..250.0f32, BUCKETS_PER_WEEK)
        ) {
            let coeffs = compress_speed_buckets(&speeds_from_vec(speeds));
            let avg_coeffs = average_compressed_speeds(&[coeffs]).expect("at least one profile");
            proptest::prop_assert!(
                coeffs == avg_coeffs,
                "Averaging a single speed profile should return the same coefficients"
            );
        }

        /// For smooth, low-frequency weekly profiles, the round-trip error should be small.
        /// This mirrors the C++ deterministic test thresholds.
        #[test]
        fn prop_smooth_roundtrip_accuracy(
            base in 20.0f32..80.0f32,      // baseline kph
            amp1 in  0.0f32..25.0f32,      // daily-ish variation
            amp2 in  0.0f32..10.0f32,      // smaller component
            phase1 in 0.0f32..(core::f32::consts::TAU),
            phase2 in 0.0f32..(core::f32::consts::TAU),
            noise_scale in 0.0f32..1.0f32  // tiny jitter
        ) {
            // Build a smooth signal: combination of two low-frequency sinusoids + tiny noise,
            // clamped to non-negative speeds.
            let speeds: Vec<f32> = (0..BUCKETS_PER_WEEK)
                .map(|i| {
                    // "Time" in arbitrary units; 2016 buckets per week, keep frequencies low.
                    let t = index_f32(i);

                    // Periods chosen to be long relative to the week
                    // to bias toward low-frequency content.
                    // ~1 day (288 * 5min)
                    let s1 = (t * (2.0 * core::f32::consts::PI / 288.0) + phase1).sin();
                    // ~2.33 days
                    let s2 = (t * (2.0 * core::f32::consts::PI / 672.0) + phase2).sin();

                    // Deterministic tiny "noise": bounded in [-1,1] then scaled
                    let n = ((t * 0.12345).sin() * (t * 0.54321).cos()) * noise_scale;

                    (base + amp1 * s1 + amp2 * s2 + n).max(0.0)
                })
                .collect();

            let recon = decode_all(&compress_speed_buckets(&speeds_from_vec(speeds.clone())));
            let diffs: Vec<f32> = speeds.iter().zip(&recon).map(|(x, y)| (x - y).abs()).collect();
            let mae = normalized_l1_norm(&diffs);
            let maxe = diffs.iter().copied().fold(0.0, f32::max);

            // Match the C++ expectations: <= 1.0 average, <= 2.0 max.
            // (If you want to tighten globally, start here.)
            proptest::prop_assert!(mae <= 1.0, "MAE too large: {mae}");
            proptest::prop_assert!(maxe <= 2.0, "Max error too large: {maxe}");
        }
    }

    const SPEEDS: [u16; BUCKETS_PER_WEEK] = [
        36, 36, 36, 36, 36, 36, 36, 36, 36, 37, 37, 37, 38, 38, 39, 40, 40, 41, 41, 42, 42, 42, 42,
        42, 42, 42, 42, 41, 41, 41, 41, 41, 41, 41, 41, 42, 42, 43, 43, 44, 44, 45, 45, 45, 46, 46,
        45, 45, 45, 44, 43, 43, 42, 41, 40, 40, 39, 39, 38, 38, 37, 37, 37, 36, 36, 35, 34, 34, 33,
        32, 30, 29, 27, 26, 24, 23, 21, 20, 19, 18, 17, 17, 16, 16, 16, 16, 16, 16, 17, 17, 16, 16,
        16, 15, 15, 14, 13, 12, 12, 11, 11, 10, 10, 11, 12, 13, 14, 16, 17, 19, 21, 24, 25, 27, 29,
        30, 31, 32, 33, 33, 33, 33, 33, 32, 32, 32, 33, 33, 34, 35, 36, 38, 39, 41, 42, 44, 45, 46,
        47, 47, 48, 48, 47, 47, 46, 45, 45, 44, 43, 43, 43, 43, 43, 44, 44, 45, 46, 46, 47, 48, 48,
        49, 49, 48, 48, 48, 47, 46, 46, 45, 44, 44, 43, 43, 43, 43, 43, 43, 43, 42, 42, 42, 41, 41,
        40, 39, 38, 37, 35, 34, 33, 31, 30, 29, 28, 27, 26, 26, 25, 25, 25, 25, 25, 25, 25, 25, 25,
        25, 25, 25, 25, 25, 24, 24, 24, 23, 23, 22, 22, 21, 20, 19, 18, 18, 17, 17, 16, 16, 16, 16,
        17, 17, 18, 19, 20, 22, 23, 24, 25, 27, 27, 28, 29, 29, 29, 29, 29, 29, 29, 30, 30, 30, 31,
        32, 33, 34, 36, 37, 39, 41, 42, 43, 45, 45, 46, 47, 47, 47, 47, 47, 47, 47, 47, 47, 47, 47,
        48, 49, 49, 50, 51, 52, 52, 53, 53, 53, 53, 52, 52, 51, 50, 49, 48, 46, 45, 44, 44, 43, 42,
        41, 41, 40, 40, 39, 38, 38, 37, 36, 35, 34, 34, 33, 33, 32, 32, 32, 32, 33, 33, 34, 34, 35,
        35, 35, 35, 35, 34, 33, 32, 31, 29, 27, 26, 24, 23, 21, 20, 19, 19, 18, 18, 18, 19, 19, 20,
        20, 20, 21, 21, 21, 21, 21, 21, 21, 20, 20, 21, 21, 21, 22, 22, 23, 24, 25, 26, 27, 28, 29,
        29, 29, 30, 30, 29, 29, 29, 29, 29, 30, 30, 31, 32, 33, 35, 37, 39, 41, 43, 44, 46, 47, 48,
        49, 50, 50, 50, 49, 49, 48, 48, 47, 46, 46, 46, 46, 46, 46, 46, 47, 48, 48, 49, 49, 49, 49,
        49, 49, 49, 48, 47, 46, 45, 44, 43, 42, 42, 41, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40, 40,
        39, 39, 38, 37, 36, 35, 34, 32, 31, 29, 27, 26, 24, 23, 21, 20, 19, 19, 18, 18, 18, 18, 18,
        19, 19, 20, 20, 21, 22, 22, 23, 23, 24, 24, 24, 23, 23, 23, 22, 22, 21, 20, 20, 19, 19, 18,
        18, 18, 18, 18, 18, 18, 18, 18, 18, 19, 19, 20, 21, 22, 23, 24, 25, 27, 28, 30, 32, 33, 35,
        37, 39, 40, 41, 42, 43, 44, 44, 44, 44, 43, 42, 42, 41, 40, 39, 39, 38, 38, 38, 38, 39, 40,
        41, 42, 43, 44, 45, 47, 48, 49, 50, 50, 51, 51, 51, 51, 50, 50, 49, 48, 47, 46, 45, 44, 44,
        43, 42, 41, 41, 40, 40, 39, 38, 38, 37, 36, 35, 34, 33, 32, 31, 30, 29, 28, 27, 27, 26, 26,
        26, 26, 26, 27, 27, 27, 28, 28, 28, 28, 28, 28, 27, 26, 24, 22, 20, 18, 16, 14, 12, 10, 8,
        7, 6, 5, 5, 5, 5, 6, 7, 8, 9, 10, 12, 13, 13, 14, 15, 15, 15, 15, 15, 15, 15, 16, 16, 17,
        18, 19, 21, 23, 25, 27, 29, 32, 34, 36, 38, 40, 41, 42, 43, 44, 44, 45, 45, 44, 44, 44, 43,
        43, 43, 42, 42, 41, 41, 41, 40, 40, 39, 38, 38, 37, 37, 36, 36, 36, 36, 36, 37, 38, 38, 39,
        41, 42, 43, 44, 44, 45, 45, 45, 44, 44, 42, 41, 39, 38, 36, 34, 32, 31, 29, 28, 28, 27, 27,
        27, 27, 28, 28, 29, 29, 29, 30, 29, 29, 29, 28, 27, 26, 25, 23, 22, 21, 20, 20, 19, 19, 18,
        18, 19, 19, 19, 20, 20, 20, 21, 21, 21, 22, 22, 22, 22, 22, 22, 23, 23, 23, 24, 24, 24, 25,
        25, 25, 25, 25, 25, 25, 25, 25, 25, 24, 24, 24, 25, 25, 26, 27, 28, 29, 30, 32, 33, 35, 36,
        38, 39, 41, 42, 43, 44, 45, 46, 46, 47, 48, 49, 49, 50, 51, 52, 53, 53, 54, 54, 54, 54, 54,
        54, 53, 52, 50, 49, 47, 46, 44, 43, 42, 40, 40, 39, 39, 39, 39, 40, 40, 41, 42, 43, 43, 44,
        44, 44, 44, 44, 43, 42, 41, 40, 39, 38, 36, 35, 34, 33, 32, 32, 31, 30, 29, 29, 28, 28, 27,
        26, 25, 25, 24, 23, 22, 21, 21, 20, 19, 19, 19, 18, 18, 18, 18, 18, 17, 17, 17, 16, 16, 15,
        15, 14, 14, 14, 13, 13, 13, 13, 14, 14, 15, 16, 17, 18, 19, 21, 22, 23, 24, 26, 27, 27, 28,
        29, 30, 30, 31, 32, 32, 33, 34, 35, 35, 36, 38, 39, 40, 41, 42, 43, 44, 45, 45, 46, 46, 46,
        46, 46, 46, 46, 45, 45, 44, 44, 43, 43, 43, 43, 43, 43, 43, 43, 44, 44, 44, 45, 45, 45, 46,
        46, 46, 45, 45, 45, 44, 44, 43, 42, 41, 41, 40, 39, 37, 36, 35, 34, 32, 31, 30, 28, 27, 25,
        24, 23, 21, 20, 19, 19, 18, 18, 18, 18, 18, 19, 19, 20, 21, 21, 22, 22, 23, 23, 23, 22, 22,
        21, 20, 19, 18, 18, 17, 16, 16, 16, 16, 17, 17, 18, 19, 20, 22, 23, 24, 25, 26, 26, 27, 27,
        27, 27, 27, 27, 27, 27, 27, 28, 28, 29, 30, 31, 33, 34, 36, 37, 39, 40, 41, 41, 42, 42, 42,
        42, 41, 40, 40, 39, 38, 38, 37, 37, 37, 38, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 48,
        49, 49, 49, 49, 49, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 49, 49, 48, 47, 46, 45, 44, 42,
        41, 40, 38, 37, 36, 35, 34, 33, 33, 33, 32, 32, 32, 32, 32, 31, 31, 30, 30, 29, 28, 27, 26,
        25, 23, 22, 21, 20, 18, 18, 17, 16, 16, 16, 16, 17, 17, 18, 19, 19, 20, 21, 22, 23, 24, 24,
        25, 25, 25, 26, 26, 26, 26, 26, 27, 27, 27, 28, 28, 29, 30, 30, 31, 32, 32, 33, 33, 34, 34,
        34, 34, 34, 34, 34, 34, 33, 34, 34, 34, 35, 36, 37, 38, 39, 41, 42, 43, 45, 46, 47, 48, 49,
        50, 50, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 52, 52, 51, 51, 51, 50, 50, 49, 47, 46,
        45, 43, 42, 40, 39, 38, 37, 36, 36, 36, 36, 36, 36, 36, 37, 37, 37, 37, 37, 36, 35, 34, 33,
        32, 31, 29, 28, 27, 27, 26, 26, 26, 26, 26, 27, 27, 27, 28, 28, 28, 27, 27, 26, 25, 24, 23,
        22, 22, 21, 20, 20, 20, 21, 21, 22, 23, 24, 25, 26, 26, 27, 28, 28, 28, 28, 28, 28, 27, 27,
        26, 26, 26, 26, 26, 27, 27, 28, 29, 30, 30, 31, 32, 33, 34, 35, 36, 37, 37, 38, 39, 40, 41,
        42, 43, 44, 45, 46, 46, 47, 47, 47, 46, 46, 45, 44, 43, 42, 40, 39, 39, 38, 38, 38, 38, 39,
        41, 42, 44, 45, 47, 49, 50, 51, 52, 52, 52, 52, 51, 50, 48, 46, 45, 43, 42, 40, 39, 38, 38,
        37, 37, 37, 37, 38, 38, 38, 38, 38, 38, 37, 37, 36, 36, 35, 34, 34, 33, 32, 32, 31, 31, 30,
        30, 29, 28, 27, 26, 24, 22, 20, 18, 16, 14, 12, 10, 9, 8, 7, 7, 7, 7, 8, 9, 10, 12, 13, 15,
        16, 17, 18, 19, 20, 20, 21, 21, 21, 21, 21, 21, 21, 22, 23, 24, 25, 26, 27, 29, 30, 32, 33,
        35, 36, 38, 39, 40, 42, 43, 44, 45, 46, 46, 47, 48, 49, 49, 49, 49, 49, 49, 48, 48, 47, 45,
        44, 42, 41, 40, 38, 37, 36, 36, 36, 36, 36, 37, 39, 40, 42, 43, 45, 47, 48, 49, 50, 50, 50,
        50, 49, 47, 46, 43, 41, 39, 36, 34, 32, 29, 28, 26, 25, 24, 24, 23, 23, 24, 24, 25, 25, 26,
        26, 26, 26, 26, 26, 25, 24, 23, 22, 21, 19, 18, 17, 16, 15, 14, 13, 13, 13, 13, 14, 14, 15,
        16, 17, 17, 18, 19, 19, 20, 20, 20, 20, 20, 20, 20, 20, 20, 21, 21, 21, 22, 23, 24, 25, 26,
        27, 28, 29, 30, 31, 31, 32, 32, 32, 33, 33, 33, 33, 33, 33, 34, 34, 35, 36, 38, 39, 40, 42,
        43, 45, 46, 47, 48, 48, 48, 48, 48, 47, 47, 46, 45, 44, 43, 42, 42, 41, 41, 41, 41, 42, 42,
        42, 43, 43, 44, 44, 44, 43, 43, 43, 42, 41, 40, 40, 39, 38, 38, 38, 38, 38, 38, 38, 39, 39,
        39, 39, 39, 39, 38, 37, 35, 33, 31, 29, 27, 25, 23, 21, 19, 18, 18, 17, 17, 18, 19, 20, 21,
        22, 23, 23, 24, 24, 24, 23, 22, 21, 19, 17, 16, 14, 13, 12, 11, 11, 11, 11, 12, 14, 15, 17,
        19, 21, 23, 24, 25, 26, 27, 28, 28, 28, 28, 28, 28, 29, 29, 30, 31, 33, 35, 36, 38, 40, 42,
        43, 44, 45, 46, 46, 45, 45, 44, 42, 41, 40, 39, 39, 38, 39, 39, 40, 42, 44, 46, 48, 50, 52,
        53, 55, 56, 56, 56, 56, 55, 54, 52, 51, 49, 47, 46, 45, 44, 44, 44, 44, 44, 45, 45, 46, 47,
        47, 47, 47, 47, 46, 46, 45, 43, 42, 41, 40, 39, 37, 37, 36, 35, 34, 34, 33, 33, 32, 31, 30,
        29, 27, 26, 25, 23, 22, 20, 19, 19, 18, 18, 18, 18, 19, 19, 20, 21, 22, 22, 23, 23, 23, 22,
        21, 21, 20, 19, 18, 17, 16, 16, 16, 17, 18, 19, 21, 23, 26, 28, 30, 33, 35, 37, 38, 39, 40,
        40, 40, 40, 39, 39, 38, 37, 37, 36, 36, 36, 36, 37, 37, 38, 39, 40, 41, 42, 43, 44, 44, 45,
        45, 45, 45, 45, 45, 44, 44, 44, 43, 43, 43, 42, 42, 42, 42, 43, 43, 43, 44, 44, 45, 46, 47,
        48, 49, 50, 51, 52, 53, 53, 53, 53, 53, 52, 51, 50, 48, 46, 43, 40, 37, 34, 32, 29, 26, 24,
        22, 20, 19, 19, 19, 19, 19, 20, 21, 22, 24, 25, 26, 27, 28, 29, 29, 29, 29, 28, 28, 27, 26,
        25, 24, 22, 21, 20, 18, 17, 16, 14, 13, 12, 11, 10, 10, 9, 9, 9, 10, 10, 11, 13, 14, 16,
        17, 19, 21, 23, 24, 26, 27, 28, 29, 29, 30, 30, 30, 30, 31, 31, 31, 32, 32, 33, 34, 35, 36,
        37, 39, 40, 41, 41, 42, 42, 43, 43, 43, 43, 42, 42, 42, 42, 42, 42, 43, 43, 44, 45, 46, 47,
        47, 48, 49, 49, 49, 49, 49, 49, 48, 47, 47, 46, 45, 44, 44, 44, 43, 43, 43, 43, 43, 43, 43,
        43, 43, 42, 41, 40, 39, 37, 36, 34, 33, 31, 30, 29, 28, 27, 27, 27, 27, 27, 27, 27, 27, 26,
        26, 25, 24, 23, 21, 20, 18, 17, 15, 14, 14, 13, 13, 14, 15, 16, 18, 20, 21, 23, 25, 27, 28,
        29, 30, 30, 29, 29, 28, 27, 25, 24, 23, 22, 22, 21, 21, 22, 23, 24, 25, 26, 27, 29, 30, 31,
        31, 32, 32, 32, 32, 31, 31, 30, 30, 30, 29,
    ];

    #[test]
    fn averaging_realistic_speed_profiles() {
        // Use the existing realistic speed profile as a base
        let base_speeds: [f32; BUCKETS_PER_WEEK] = SPEEDS.map(f32::from);

        // Create two additional profiles with small realistic variations
        // Profile 1: slightly faster (5% increase)
        let speeds1: [f32; BUCKETS_PER_WEEK] = base_speeds.map(|s| s * 1.05);

        // Profile 2: slightly slower (5% decrease)
        let speeds2: [f32; BUCKETS_PER_WEEK] = base_speeds.map(|s| s * 0.95);

        // Compress all three profiles
        let coeffs_base = compress_speed_buckets(&base_speeds);
        let coeffs1 = compress_speed_buckets(&speeds1);
        let coeffs2 = compress_speed_buckets(&speeds2);

        // Average in the compressed domain, then decompress.
        let avg_coeffs = average_compressed_speeds(&[coeffs_base, coeffs1, coeffs2])
            .expect("at least one profile");
        let avg_speeds = decode_all(&avg_coeffs);

        // The true average is the base profile.
        let max_error = avg_speeds
            .iter()
            .zip(&base_speeds)
            .map(|(avg, truth)| (avg - truth).abs())
            .fold(0.0, f32::max);

        // The error should be quite small for realistic profiles
        // Even though we're averaging in the compressed domain, the smooth nature
        // of the profiles means the DCT compression doesn't introduce much error
        assert!(
            max_error <= 1.0,
            "Max error is larger than expected: {max_error} kph"
        );
    }

    #[test]
    fn round_trip_constant_speed() {
        // 50 kph across the week
        let speeds = [50.0f32; BUCKETS_PER_WEEK];
        let coeffs = compress_speed_buckets(&speeds);
        // DC should dominate; reconstruction should be ~50 everywhere.
        for s in decode_all(&coeffs) {
            assert!((s - 50.0).abs() < 0.05);
        }
    }

    #[test]
    fn base64_round_trip() {
        // simple pattern
        let coeffs: [i16; COEFFICIENT_COUNT] =
            core::array::from_fn(|i| i16::try_from(i).expect("small") + 1000);
        let enc = encode_compressed_speeds(&coeffs);
        let dec = decode_base64_speed_coefficients(&enc).unwrap();
        assert_eq!(coeffs, dec);
    }

    #[test]
    fn base64_errors() {
        assert!(matches!(
            decode_base64_speed_coefficients("not base64!"),
            Err(PredictedSpeedCodecError::Base64DecodeError(_))
        ));
        assert!(matches!(
            decode_base64_speed_coefficients("AAAA"),
            Err(PredictedSpeedCodecError::IncorrectByteCount { count: 3 })
        ));
    }

    #[test]
    fn test_base64_encode_fixture() {
        // A random fixture of a looping series.
        // This has been verified against the original C++ implementation as well
        // as another random check.
        let speeds: [f32; BUCKETS_PER_WEEK] = core::array::from_fn(|i| index_f32(i % 100));
        let coeffs = compress_speed_buckets(&speeds);
        let enc = encode_compressed_speeds(&coeffs);
        assert_eq!(
            enc,
            "CKD/4f/r/+H/6//g/+v/4P/q/9//6v/f/+r/3v/q/93/6f/b/+n/2v/o/9f/5//V/+b/0f/l/8z/4//G/+D/vf/d/67/1/+V/8z/Xf+u/nz+GwLJAEcAqwAWAFwABwA8AAAAK//8ACD/+AAZ//YAE//0AA//8gAM//AACf/uAAb/7AAE/+kAAv/m////4v/9/93/+f/U//T/xf/q/5//y/6PAQMAngApADkAFwAfABAAEwAMAAwACQAHAAcAAwAGAAAABf/9AAT/+wAD//kAA//2AAL/9AAC//EAAf/tAAH/6AAA/+EAAP/U////tv///x8AFADL//8AQ///ACf//gAa//4AE//+AA7//QAL//0ACP/9AAb//AAE//wAAv/8AAD/+//+//v//P/6//r/+v/2//n/8v/4/+v/9f/b/+//nP+TALoADAAyAAMAHgAAABX//gAQ//0ADf/9AAv//AAJ//sAB//7AAb/+gAF//kABP/4AAP/9wAC//YAAf/1AAD/8//+//D//P/q//f/2w=="
        );
    }

    /// End-to-end test decoding speeds from a base64 string.
    #[test]
    fn test_decoding() {
        let encoded_speed = "BcUACQAEACEADP/7/9sAGf/fAAwAGwARAAX/+AAAAB0AFAAS//AAF//+ACwACQAqAAAAKP/6AEMABABsAAsBBAAq/rcAAP+bAAz/zv/s/9MABf/Y//X/8//9/+wACf/P//EADv/8//L//P/y//H////7AAwAFf/5//oADgAZAAQAFf/3/+8AB//yAB8ABv/0AAUAEf/8//QAFAAG//b////j//v//QAT//7/+f/kABMABwABAAv/6//8//cAEwAAABT/8v/6//wAAAAQ//cACwAFAAT/1//sAAEADAABAAYAE//9AAn/7gAH/+AAFQAB//4AC//o/+gAE//+AAAAFf/l//kABP/+//kACAAG//cAHv/qAB0AAv/4/+v/+wALAAMABP/3AAT/8wAIAAr/9wAK//j/+wAEAAD/+P/8//v/8f/2//L//AALAAcABgAG//gAAv/5AAoAHv//AAcAFf/zABD/7AAUAAv/7v/8AAgACAAN//0ADP/iABD/9f/3//7/+P/3AAQADP//AAMABw==";

        let coeffs =
            decode_base64_speed_coefficients(encoded_speed).expect("Failed to decode coefficients");

        // Build a single-edge profile view
        let offsets = [U32::new(0)];
        let profiles = single_profile(&coeffs);
        let ps = PredictedSpeeds::new(&offsets, &profiles);

        for (i, &exp) in SPEEDS.iter().enumerate() {
            let s = ps.speed(0, bucket_start(i)).expect("speed");
            // The C++ codebase had a rather strange way of comparing these.
            // Presumably it was to account for theoretical differences that could arise
            // due to floating point imprecision?
            assert!(
                (s - f32::from(exp)).abs() < 1.0,
                "Speed at bucket {i} ({s}) doesn't match expectation ({exp})"
            );
        }
    }

    fn normalized_l1_norm(vec: &[f32]) -> f32 {
        assert!(!vec.is_empty());
        let sum: f32 = vec.iter().map(|v| v.abs()).sum();
        sum / index_f32(vec.len())
    }

    /// Tests from the C++ library.
    mod cpp_tests {
        use super::*;

        /// Reads 200 big-endian coefficients starting at byte 1 of a "broken" C++ fixture,
        /// which has one byte too many (an extra byte at the beginning).
        fn coefficients_after_first_byte(encoded: &str) -> [i16; COEFFICIENT_COUNT] {
            let decoded = STANDARD.decode(encoded.as_bytes()).expect("base64 decode");
            // Broken fixture expectation (+1)
            assert_eq!(decoded.len(), DECODED_SPEED_SIZE + 1);
            // The first value (at index 0) is 1 in the broken fixtures.
            assert_eq!(decoded[0], 1, "First value should be 1");
            let (pairs, []) = decoded[1..].as_chunks::<2>() else {
                panic!("whole coefficients");
            };
            let mut coefficients = [0i16; COEFFICIENT_COUNT];
            for (coefficient, pair) in coefficients.iter_mut().zip(pairs) {
                *coefficient = i16::from_be_bytes(*pair);
            }
            coefficients
        }

        /// Limited base64 decoding tests.
        ///
        /// Ported as-is from the C++ reference implementation.
        #[test]
        fn test_free_flow_speed() {
            // Mirrors the C++ helper that inspects a tiny base64 header containing
            // t (5 bits), free_flow_speed, constrained_flow_speed.
            // If the values don't match the expected fixtures, panics with an assertion error.
            fn try_free_flow_speed(
                encoded: &str,
                expected_free_flow: u8,
                expected_constrained_flow: u8,
            ) {
                let decoded = STANDARD.decode(encoded.as_bytes()).expect("base64 decode");
                let [t, free_flow_speed, constrained_flow_speed] = decoded[..] else {
                    panic!("three bytes");
                };
                assert_eq!(t & 0x1f, 0);
                assert_eq!(free_flow_speed, expected_free_flow);
                assert_eq!(constrained_flow_speed, expected_constrained_flow);
            }

            try_free_flow_speed("AAie", 8, 158);
            try_free_flow_speed("AACe", 0, 158);
        }

        /// Tests decoding of a base64 string to bucketized speeds.
        ///
        /// This is preserved as-is from the C++ test code for completeness.
        /// However, the C++ code noted that the test fixture was broken,
        /// and so the test case essentially copies a bunch of the existing decoding code
        /// instead of exercising the existing helpers.
        ///
        /// An improved version outside the legacy module addresses these issues.
        #[test]
        fn test_decoding() {
            // Bit of a hacky, and rather lossy comparison function.
            // Preserved as-is.
            fn within_threshold(v1: u32, v2: u32) -> bool {
                const SPEED_ERROR_THRESHOLD: u32 = 1;
                v1.abs_diff(v2) < SPEED_ERROR_THRESHOLD
            }

            // The fixture from the C++ tests, which is one byte too long.
            let encoded_speed_string = "AQXFAAkABAAhAAz/+//bABn/3wAMABsAEQAF//gAAAAdABQAEv/wABf//gAsAAkAKgAAACj/+gBDAAQAbAALAQQAKv63AAD/mwAM/87/7P/TAAX/2P/1//P//f/sAAn/z//xAA7//P/y//z/8v/x////+wAMABX/+f/6AA4AGQAEABX/9//vAAf/8gAfAAb/9AAFABH//P/0ABQABv/2////4//7//0AE//+//n/5AATAAcAAQAL/+v//P/3ABMAAAAU//L/+v/8AAAAEP/3AAsABQAE/9f/7AABAAwAAQAGABP//QAJ/+4AB//gABUAAf/+AAv/6P/oABP//gAAABX/5f/5AAT//v/5AAgABv/3AB7/6gAdAAL/+P/r//sACwADAAT/9wAE//MACAAK//cACv/4//sABAAA//j//P/7//H/9v/y//wACwAHAAYABv/4AAL/+QAKAB7//wAHABX/8wAQ/+wAFAAL/+7//AAIAAgADf/9AAz/4gAQ//X/9//+//j/9wAEAAz//wADAAc=";

            let offsets = [U32::new(0)];
            let profiles = single_profile(&coefficients_after_first_byte(encoded_speed_string));
            let ps = PredictedSpeeds::new(&offsets, &profiles);

            for (i, &exp) in SPEEDS.iter().enumerate() {
                let s = ps.speed(0, bucket_start(i)).expect("speed");
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "speeds here are small and positive, as in the C++ test"
                )]
                let s_rounded = (s + 0.5).floor() as u32;
                assert!(
                    within_threshold(s_rounded, u32::from(exp)),
                    "Speed outside of range at bucket {i}. Expected {exp}; found {s_rounded}"
                );
            }
        }

        /// Asserts that no negative speeds are deserialized.
        ///
        /// The original code did not state whether this was a regression test
        /// against some known input or if it was just a simple sanity check,
        /// so it is included as-is.
        /// However, the spirit of this test is better represented by the property test
        /// in the enclosing module.
        #[test]
        fn test_negative_speeds_original() {
            // Same “broken fixture” pattern (decoded size == DECODED_SPEED_SIZE + 1).
            let encoded_speed_string = "AQRu//UAEAAC/+4AA//6//gAAwAFAA//9wAHAAH/4AAd/+wACwAH//0AGQAYAA7//wANAAL/9//mAAUACgATAAb/8v/2//8AC//1ABMAAAAGABX/9//0//0AAAAQAAIAAv/6////9gAJAAcACf/zAAQAAwAC//oACf/2//sADQAVABD/+QADAAcACf/2//gABwAHAAAABv/9AAf/+QAM//kAEAAE//r//wAMAAD/9AAN//D/7QAK//EAE//7AAkAAQAF//f/+AAB//z/6f/y//MAAP/6ABL//AATABX//wAFAAMAGv/2AAf//wAI//sACv/5AAb/8gAOAAYADv/5AAMACP////T/7gAH//P/+f/9//n/9f/0//0AAwAP//3/8gAA//8ACv////gAAgAHAAP//QALAAcAFAAA//8ABP/vAAIAEAAM/+3/9QAC//j//v/tABj/+wAA//sAC//6//0ABwAAAAoABgAMAAb/+P/3AAX/9//7//0ADP/sAAwAB//v/+3//wAMABAACgAF//o=";

            let offsets = [U32::new(0)];
            let profiles = single_profile(&coefficients_after_first_byte(encoded_speed_string));
            let ps = PredictedSpeeds::new(&offsets, &profiles);

            for i in 0..BUCKETS_PER_WEEK {
                let s = ps.speed(0, bucket_start(i)).expect("speed");
                assert!(s >= 0.0, "Negative speed at bucket {i}");
            }
        }

        /// Same as the above test, but uses a fixture of the correct length.
        ///
        /// Provided for completeness.
        #[test]
        fn negative_speeds_correct_fixture() {
            let encoded_speed = "BG7/9QAQAAL/7gAD//r/+AADAAUAD//3AAcAAf/gAB3/7AALAAf//QAZABgADv//AA0AAv/3/+YABQAKABMABv/y//b//wAL//UAEwAAAAYAFf/3//T//QAAABAAAgAC//r////2AAkABwAJ//MABAADAAL/+gAJ//b/+wANABUAEP/5AAMABwAJ//b/+AAHAAcAAAAG//0AB//5AAz/+QAQAAT/+v//AAwAAP/0AA3/8P/tAAr/8QAT//sACQABAAX/9//4AAH//P/p//L/8wAA//oAEv/8ABMAFf//AAUAAwAa//YAB///AAj/+wAK//kABv/yAA4ABgAO//kAAwAI////9P/uAAf/8//5//3/+f/1//T//QADAA///f/yAAD//wAK////+AACAAcAA//9AAsABwAUAAD//wAE/+8AAgAQAAz/7f/1AAL/+P/+/+0AGP/7AAD/+wAL//r//QAHAAAACgAGAAwABv/4//cABf/3//v//QAM/+wADAAH/+//7f//AAwAEAAKAAX/+g==";

            let coeffs = decode_base64_speed_coefficients(encoded_speed)
                .expect("Failed to decode coefficients");

            let offsets = [U32::new(0)];
            let profiles = single_profile(&coeffs);
            let ps = PredictedSpeeds::new(&offsets, &profiles);

            for i in 0..BUCKETS_PER_WEEK {
                let s = ps.speed(0, bucket_start(i)).expect("speed");
                assert!(s >= 0.0, "Unexpected negative speed at bucket {i}");
            }
        }

        #[test]
        fn test_compress_decompress_accuracy() {
            // generate speed values for buckets
            let speeds: [f32; BUCKETS_PER_WEEK] =
                core::array::from_fn(|i| (30.0f32 + 15.0f32 * (index_f32(i) / 20.0).sin()).round());

            // compress, then decompress all buckets
            let recon = decode_all(&compress_speed_buckets(&speeds));

            // diffs
            let diffs: Vec<f32> = speeds
                .iter()
                .zip(&recon)
                .map(|(s, r)| (s - r).abs())
                .collect();

            // average error <= 1 KPH
            let l1_err = normalized_l1_norm(&diffs);
            assert!(l1_err <= 1.0, "Low decompression accuracy: L1={l1_err}");

            // max error <= 2 KPH
            let max_diff = diffs.iter().copied().fold(f32::MIN, f32::max);
            assert!(
                max_diff <= 2.0,
                "Low decompression accuracy: max={max_diff}"
            );
        }

        /// Coefficients alternating +10*i, -10*i.
        fn alternating_coefficients() -> [i16; COEFFICIENT_COUNT] {
            core::array::from_fn(|i| {
                let v = 10 * i16::try_from(i).expect("small");
                if i % 2 == 0 { v } else { -v }
            })
        }

        const ALTERNATING_ENCODED: &str = "AAD/9gAU/+IAKP/OADz/ugBQ/6YAZP+SAHj/fgCM/2oAoP9WALT/QgDI/y4A3P8aAPD/BgEE/vIBGP7eASz+ygFA/rYBVP6iAWj+jgF8/noBkP5mAaT+UgG4/j4BzP4qAeD+FgH0/gICCP3uAhz92gIw/cYCRP2yAlj9ngJs/YoCgP12ApT9YgKo/U4CvP06AtD9JgLk/RIC+Pz+Awz86gMg/NYDNPzCA0j8rgNc/JoDcPyGA4T8cgOY/F4DrPxKA8D8NgPU/CID6PwOA/z7+gQQ++YEJPvSBDj7vgRM+6oEYPuWBHT7ggSI+24EnPtaBLD7RgTE+zIE2PseBOz7CgUA+vYFFPriBSj6zgU8+roFUPqmBWT6kgV4+n4FjPpqBaD6VgW0+kIFyPouBdz6GgXw+gYGBPnyBhj53gYs+coGQPm2BlT5ogZo+Y4GfPl6BpD5Zgak+VIGuPk+Bsz5Kgbg+RYG9PkCBwj47gcc+NoHMPjGB0T4sgdY+J4HbPiKB4D4dgeU+GIHqPhOB7z4Og==";

        #[test]
        fn test_speeds_encoder() {
            let encoded = encode_compressed_speeds(&alternating_coefficients());
            assert_eq!(encoded, ALTERNATING_ENCODED, "Incorrect encoded string");
        }

        #[test]
        fn test_speeds_decoder() {
            let decoded = decode_base64_speed_coefficients(ALTERNATING_ENCODED).expect("decode ok");
            assert_eq!(
                decoded,
                alternating_coefficients(),
                "Incorrect decoded coefficients"
            );
        }
    }

    /// Checks against values from mainline 3.9.1
    /// (see `fixtures/predicted-speeds-reference` and `tools/oracle`).
    mod mainline {
        use super::*;
        use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_WITH_FLOW};
        use std::path::PathBuf;

        /// Whether this platform's `cosf` is glibc's, which mainline's reference values used.
        /// Elsewhere (and under Miri, which perturbs float results on purpose),
        /// results may differ in the last bits.
        const BIT_EXACT: bool = cfg!(all(target_os = "linux", target_env = "gnu", not(miri)));

        /// The profile in mainline's predicted speed decoding test, without its stray first byte.
        const MAINLINE_TEST: &str = "BcUACQAEACEADP/7/9sAGf/fAAwAGwARAAX/+AAAAB0AFAAS//AAF//+ACwACQAqAAAAKP/6AEMABABsAAsBBAAq/rcAAP+bAAz/zv/s/9MABf/Y//X/8//9/+wACf/P//EADv/8//L//P/y//H////7AAwAFf/5//oADgAZAAQAFf/3/+8AB//yAB8ABv/0AAUAEf/8//QAFAAG//b////j//v//QAT//7/+f/kABMABwABAAv/6//8//cAEwAAABT/8v/6//wAAAAQ//cACwAFAAT/1//sAAEADAABAAYAE//9AAn/7gAH/+AAFQAB//4AC//o/+gAE//+AAAAFf/l//kABP/+//kACAAG//cAHv/qAB0AAv/4/+v/+wALAAMABP/3AAT/8wAIAAr/9wAK//j/+wAEAAD/+P/8//v/8f/2//L//AALAAcABgAG//gAAv/5AAoAHv//AAcAFf/zABD/7AAUAAv/7v/8AAgACAAN//0ADP/iABD/9f/3//7/+P/3AAQADP//AAMABw==";

        /// The number of profiles at the start of `profiles.txt` with decoded values.
        const DECODED_PROFILES: usize = 24;

        fn reference_dir() -> PathBuf {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("predicted-speeds-reference")
        }

        /// splitmix64, as in `tools/oracle/predicted-speeds-reference.py`.
        struct Rng(u64);

        impl Rng {
            fn new(index: u64) -> Self {
                Self(0x5EED_0000 + index)
            }

            fn next(&mut self) -> u64 {
                self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^ (z >> 31)
            }

            fn below(&mut self, n: u64) -> i64 {
                i64::try_from(self.next() % n).expect("small")
            }
        }

        /// A profile's input: speeds, or coefficients to encode and decode directly.
        enum Input {
            Speeds(Box<[f32; BUCKETS_PER_WEEK]>),
            Coefficients(Box<[i16; COEFFICIENT_COUNT]>),
        }

        /// Speeds from hundredths of a kph, divided in f32 as the generator does.
        fn speeds(hundredths: impl IntoIterator<Item = i64>) -> Input {
            let mut speeds = Box::new([0f32; BUCKETS_PER_WEEK]);
            let mut count = 0;
            for (speed, h) in speeds.iter_mut().zip(hundredths) {
                *speed = f32::from(u16::try_from(h).expect("speeds are 0 to 25000")) / 100.0;
                count += 1;
            }
            assert_eq!(count, BUCKETS_PER_WEEK);
            Input::Speeds(speeds)
        }

        /// Regenerates a profile's input from its name.
        fn input(name: &str) -> Input {
            let buckets = (0..BUCKETS_PER_WEEK).map(|i| i64::try_from(i).expect("small"));
            match name {
                "zero" => return speeds(buckets.map(|_| 0)),
                "max" => return speeds(buckets.map(|_| 25_000)),
                "sawtooth" => return speeds(buckets.map(|i| (i % 100) * 100)),
                "mainline-test" => {
                    return Input::Coefficients(Box::new(
                        decode_base64_speed_coefficients(MAINLINE_TEST).expect("valid"),
                    ));
                }
                _ => {}
            }
            let (kind, index) = name.rsplit_once('-').expect("generated profile name");
            let mut rng = Rng::new(index.parse().expect("generated profile index"));
            match kind {
                "uniform" => speeds(buckets.map(|_| rng.below(25_001))),
                "walk" => {
                    let mut v = rng.below(12_001);
                    speeds(buckets.map(|_| {
                        v = (v + rng.below(401) - 200).clamp(0, 25_000);
                        v
                    }))
                }
                "steps" => {
                    let (mut remaining, mut level) = (0, 0);
                    speeds(buckets.map(|_| {
                        if remaining == 0 {
                            remaining = 1 + rng.below(288);
                            level = rng.below(25_001);
                        }
                        remaining -= 1;
                        level
                    }))
                }
                "daily" => {
                    let base = rng.below(8001);
                    let amp = rng.below(8001);
                    let phase = rng.below(288);
                    speeds(buckets.map(|i| base + amp * ((i + phase) % 288 - 144).abs() / 144))
                }
                "spikes" => {
                    let base = rng.below(10_001);
                    speeds(buckets.map(|_| {
                        if rng.below(20) == 0 {
                            rng.below(25_001)
                        } else {
                            base
                        }
                    }))
                }
                "kph" => {
                    let mut k = rng.below(131);
                    speeds(buckets.map(|_| {
                        k = (k + rng.below(7) - 3).clamp(0, 140);
                        k * 100
                    }))
                }
                "coefficients" => Input::Coefficients(Box::new(core::array::from_fn(|_| {
                    u16::try_from(rng.next() & 0xFFFF)
                        .expect("masked")
                        .cast_signed()
                }))),
                "small-coefficients" => Input::Coefficients(Box::new(core::array::from_fn(|i| {
                    let v = if i == 0 {
                        rng.below(12_001)
                    } else {
                        rng.below(2001) - 1000
                    };
                    i16::try_from(v).expect("small")
                }))),
                _ => panic!("unknown profile kind {kind}"),
            }
        }

        /// `(name, mainline base64)` for every reference profile.
        fn profiles() -> Vec<(String, String)> {
            std::fs::read_to_string(reference_dir().join("profiles.txt"))
                .expect("read profiles.txt")
                .lines()
                .filter(|line| !line.starts_with('#'))
                .map(|line| {
                    let (name, encoded) = line.split_once(' ').expect("name and base64");
                    (name.to_string(), encoded.to_string())
                })
                .collect()
        }

        #[test]
        fn constants_are_mainlines() {
            // Mainline's literals, parsed to the nearest f32 as a C++ compiler does.
            let literal = |s: &str| s.parse::<f32>().expect("literal");
            assert_eq!(ONE_OVER_SQRT_2.to_bits(), literal("0.707106781").to_bits());
            assert_eq!(
                SPEED_NORMALIZATION.to_bits(),
                literal("0.031497039").to_bits()
            );
            assert_eq!(
                PI_BUCKET_CONSTANT.to_bits(),
                (literal("3.14159265") / 2016.0).to_bits()
            );
            assert_eq!(BUCKETS_PER_WEEK, 2016);
            assert_eq!(SECONDS_PER_WEEK, 604_800);
            assert_eq!(INVALID_SECONDS_OF_WEEK, 1_048_575);
        }

        #[test]
        fn reference_profiles_are_all_there() {
            let profiles = profiles();
            assert_eq!(profiles.len(), 65);
            let speed_profiles = profiles
                .iter()
                .filter(|(name, _)| matches!(input(name), Input::Speeds(_)))
                .count();
            assert_eq!(speed_profiles, 50);
        }

        /// Compression matches mainline's `compress_speed_buckets`,
        /// and encoding matches its `encode_compressed_speeds`.
        #[test]
        #[cfg_attr(miri, ignore = "too slow under Miri")]
        fn codec_matches_mainline() {
            for (name, encoded) in profiles() {
                let expected = decode_base64_speed_coefficients(&encoded).expect("valid base64");
                match input(&name) {
                    Input::Speeds(speeds) => {
                        let coefficients = compress_speed_buckets(&speeds);
                        if BIT_EXACT {
                            assert_eq!(coefficients, expected, "{name}");
                        } else {
                            for (c, e) in coefficients.iter().zip(&expected) {
                                assert!(c.abs_diff(*e) <= 1, "{name}");
                            }
                        }
                    }
                    Input::Coefficients(coefficients) => {
                        assert_eq!(*coefficients, expected, "{name}");
                    }
                }
                assert_eq!(encode_compressed_speeds(&expected), encoded, "{name}");
            }
        }

        /// Decoding matches mainline's `decompress_speed_bucket` for every bucket of the week.
        #[test]
        #[cfg_attr(miri, ignore = "too slow under Miri")]
        fn decoding_matches_mainline() {
            let decoded = std::fs::read(reference_dir().join("decoded.bin")).expect("read");
            let (values, []) = decoded.as_chunks::<4>() else {
                panic!("decoded.bin holds whole f32 values");
            };
            assert_eq!(values.len(), DECODED_PROFILES * BUCKETS_PER_WEEK);
            for ((name, encoded), expected) in profiles()
                .iter()
                .zip(values.as_chunks::<BUCKETS_PER_WEEK>().0)
            {
                let coefficients = decode_base64_speed_coefficients(encoded).expect("valid");
                for (bucket, expected) in SpeedBucket::all().zip(expected) {
                    let speed = decompress_speed_bucket(&coefficients, bucket);
                    let expected = f32::from_le_bytes(*expected);
                    if BIT_EXACT {
                        assert_eq!(speed.to_bits(), expected.to_bits(), "{name} {bucket:?}");
                    } else {
                        assert!(
                            (speed - expected).abs() <= 1e-5 * expected.abs().max(1.0),
                            "{name} {bucket:?}: {speed} vs {expected}"
                        );
                    }
                }
            }
        }

        /// The fixture tile's profiles decode the same as mainline's reference profiles,
        /// read in place from the tile.
        #[test]
        #[cfg_attr(miri, ignore = "too slow under Miri")]
        fn tile_profiles_decode_like_mainline() {
            let decoded = std::fs::read(reference_dir().join("decoded.bin")).expect("read");
            let profiles = profiles();
            let tile = TEST_GRAPH_TILE_WITH_FLOW.borrow_dependent();
            let speeds = tile.predicted_speeds.as_ref().expect("predicted speeds");
            // Edge 7 carries the sawtooth profile, and edge 42 mainline's test profile.
            for (edge, name) in [(7, "sawtooth"), (42, "mainline-test")] {
                let position = profiles
                    .iter()
                    .position(|(n, _)| n == name)
                    .expect("profile");
                assert!(position < DECODED_PROFILES);
                let (_, encoded) = &profiles[position];
                let in_tile = speeds.coefficients(edge).expect("profile").map(I16::get);
                assert_eq!(
                    in_tile,
                    decode_base64_speed_coefficients(encoded).expect("valid")
                );
                let expected = &decoded[position * BUCKETS_PER_WEEK * 4..];
                for (bucket, expected) in SpeedBucket::all().zip(expected.as_chunks::<4>().0) {
                    let seconds = u64::from(SPEED_BUCKET_SIZE_SECONDS)
                        * u64::try_from(bucket.index()).expect("small");
                    // Any second in the bucket, and a week later, gives the same speed.
                    for seconds in [seconds, seconds + 299, seconds + 604_800] {
                        let speed = speeds.try_speed(edge, seconds).expect("speed");
                        let expected = f32::from_le_bytes(*expected);
                        if BIT_EXACT {
                            assert_eq!(speed.to_bits(), expected.to_bits(), "{edge} {seconds}");
                        } else {
                            assert!((speed - expected).abs() <= 1e-4, "{edge} {seconds}");
                        }
                    }
                }
            }
        }

        /// Speed selection matches mainline's `GraphTile::GetSpeed` on the fixture tile
        /// for every flow mask, a range of times (including no time), and both truck flags.
        #[test]
        #[cfg_attr(miri, ignore = "too slow under Miri")]
        fn select_speed_matches_mainline_get_speed() {
            let reference = std::fs::read_to_string(reference_dir().join("get-speed.txt"))
                .expect("read get-speed.txt");
            let tile = TEST_GRAPH_TILE_WITH_FLOW.borrow_dependent();
            let speeds = tile.predicted_speeds.as_ref().expect("predicted speeds");
            let mut checked = 0;
            for line in reference.lines().filter(|line| line.starts_with("speed ")) {
                let fields: Vec<u64> = line
                    .split(' ')
                    .skip(1)
                    .map(|field| field.parse().expect("number"))
                    .collect();
                let [edge, mask, seconds, truck, kph, sources] = fields[..] else {
                    panic!("bad line {line}");
                };
                let edge = usize::try_from(edge).expect("small");
                let mask = EnumSet::<FlowSource>::try_from_repr(u8::try_from(mask).expect("mask"))
                    .expect("valid mask");
                let selected = select_speed(
                    &tile.directed_edges()[edge],
                    mask,
                    Some(seconds),
                    truck == 1,
                    |seconds| speeds.try_speed(edge, u64::from(seconds)),
                )
                .expect("speed");
                // Mainline's float speeds equal ours on glibc, so the rounding agrees exactly.
                // Elsewhere, an ulp could in principle tip a speed at x.5 the other way.
                if BIT_EXACT {
                    assert_eq!(u64::from(selected.kph), kph, "{line}");
                } else {
                    assert!(u64::from(selected.kph).abs_diff(kph) <= 1, "{line}");
                }
                assert_eq!(
                    u64::from(selected.flow_sources.as_repr()),
                    sources,
                    "{line}"
                );
                checked += 1;
            }
            assert_eq!(checked, 4 * 16 * 23 * 2 + 2 * BUCKETS_PER_WEEK);
        }

        /// No time and mainline's "invalid time" sentinel are the same thing.
        #[test]
        fn no_time_is_the_invalid_sentinel() {
            let tile = TEST_GRAPH_TILE_WITH_FLOW.borrow_dependent();
            for edge in tile.directed_edges().iter().take(64) {
                for mask in [
                    EnumSet::empty(),
                    DEFAULT_FLOW_MASK,
                    FlowSource::FreeFlow.into(),
                ] {
                    let unreachable = |_| -> Result<f32, ()> { panic!("no time, no profile") };
                    assert_eq!(
                        select_speed(edge, mask, None, false, unreachable),
                        select_speed(
                            edge,
                            mask,
                            Some(INVALID_SECONDS_OF_WEEK),
                            false,
                            unreachable
                        ),
                    );
                }
            }
        }

        #[test]
        fn flow_masks_are_mainlines() {
            assert_eq!(EnumSet::from(FlowSource::FreeFlow).as_repr(), 1);
            assert_eq!(EnumSet::from(FlowSource::ConstrainedFlow).as_repr(), 2);
            assert_eq!(EnumSet::from(FlowSource::Predicted).as_repr(), 4);
            assert_eq!(EnumSet::from(FlowSource::Current).as_repr(), 8);
            assert_eq!(DEFAULT_FLOW_MASK.as_repr(), 15);
        }
    }

    /// Reading profiles from tile data never panics, whatever the offsets say.
    mod lookup {
        use super::*;

        fn profiles(count: usize) -> Vec<I16<LE>> {
            (0..count * COEFFICIENT_COUNT)
                .map(|i| I16::new(i16::try_from(i % 1000).expect("small")))
                .collect()
        }

        #[test]
        fn edge_index_out_of_range() {
            let offsets = [U32::new(0)];
            let profiles = profiles(1);
            let speeds = PredictedSpeeds::new(&offsets, &profiles);
            assert_eq!(
                speeds.try_speed(1, 0),
                Err(PredictedSpeedLookupError::EdgeIndexOutOfRange {
                    index: 1,
                    edge_count: 1
                })
            );
            assert_eq!(speeds.speed(usize::MAX, 0), None);
        }

        #[test]
        fn offset_out_of_range() {
            let offsets = [U32::new(1), U32::new(u32::MAX), U32::new(200)];
            let profiles = profiles(1);
            let speeds = PredictedSpeeds::new(&offsets, &profiles);
            for (edge, offset) in [(0, 1), (1, u32::MAX), (2, 200)] {
                assert_eq!(
                    speeds.coefficients(edge),
                    Err(PredictedSpeedLookupError::ProfileOutOfRange {
                        offset,
                        available: 200
                    })
                );
            }
        }

        #[test]
        fn unaligned_offset_reads_like_mainline() {
            // Mainline reads 200 coefficients from wherever the offset points.
            let offsets = [U32::new(3)];
            let profiles = profiles(2);
            let speeds = PredictedSpeeds::new(&offsets, &profiles);
            let coefficients = speeds.coefficients(0).expect("in range");
            assert_eq!(coefficients[0].get(), 3);
            assert_eq!(coefficients[199].get(), 202);
        }

        #[test]
        fn empty_tile_data() {
            let speeds = PredictedSpeeds::new(&[], &[]);
            assert!(speeds.try_speed(0, 0).is_err());
        }

        #[cfg(not(miri))]
        proptest::proptest! {
            #[test]
            fn prop_lookup_never_panics(
                offsets in proptest::collection::vec(proptest::num::u32::ANY, 0..8),
                profile_len in 0usize..1000,
                edge in 0usize..10,
                seconds in proptest::num::u64::ANY,
            ) {
                let offsets: Vec<U32<LE>> = offsets.into_iter().map(U32::new).collect();
                let profiles: Vec<I16<LE>> = (0..profile_len).map(|_| I16::new(1)).collect();
                let speeds = PredictedSpeeds::new(&offsets, &profiles);
                let in_range = offsets.get(edge).is_some_and(|offset| {
                    usize::try_from(offset.get()).is_ok_and(|o| o + COEFFICIENT_COUNT <= profile_len)
                });
                proptest::prop_assert_eq!(speeds.try_speed(edge, seconds).is_ok(), in_range);
            }
        }
    }

    mod buckets {
        use super::*;

        #[test]
        fn bucket_bounds() {
            assert_eq!(SpeedBucket::new(0).map(SpeedBucket::index), Some(0));
            assert_eq!(SpeedBucket::new(2015).map(SpeedBucket::index), Some(2015));
            assert_eq!(SpeedBucket::new(2016), None);
            assert_eq!(SpeedBucket::new(usize::MAX), None);
            assert_eq!(SpeedBucket::all().count(), BUCKETS_PER_WEEK);
        }

        #[test]
        fn seconds_wrap_by_week() {
            let bucket = |s| SpeedBucket::from_second_of_week(s).index();
            assert_eq!(bucket(0), 0);
            assert_eq!(bucket(299), 0);
            assert_eq!(bucket(300), 1);
            assert_eq!(bucket(604_799), 2015);
            assert_eq!(bucket(604_800), 0);
            assert_eq!(bucket(604_800 + 300), 1);
            assert_eq!(
                bucket(u64::MAX),
                usize::try_from((u64::MAX % 604_800) / 300).unwrap()
            );
        }

        #[test]
        fn average_of_nothing() {
            assert_eq!(average_compressed_speeds(&[]), None);
        }
    }
}
