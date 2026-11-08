//! # The Valhalla Tile Hierarchy
//!
//! Valhalla uses a tiered hierarchy based on road class.
//! This enables efficient traversal of the graph.
//! See <https://valhalla.github.io/valhalla/tiles/> for a full writeup.

use super::{GraphId, RoadClass};
use geo::{Coord, CoordFloat, Rect, coord};
use num_traits::FromPrimitive;
use std::sync::LazyLock;

/// A tiling system description.
///
/// Valhalla is relatively generic here,
/// though everything is relatively hard-coded to WGS84.
pub struct TilingSystem {
    /// The bounding box of the tiling system.
    pub bounding_box: Rect<f32>,
    /// The size of each side of a square tile.
    pub tile_size: f32,
    /// The number of rows in the tiling system.
    pub n_rows: u32,
    /// The number of columns in the tiling system.
    pub n_cols: u32,
    /// The number of subdivisions within a single tile.
    pub n_subdivisions: u8,
    /// Does the tiling system wrap in the x direction (e.g. at longitude = 180)?
    pub wrap_x: bool,
}

impl TilingSystem {
    fn new(bounding_box: Rect<f32>, tile_size: f32) -> Self {
        Self {
            bounding_box,
            tile_size,
            #[expect(clippy::cast_possible_truncation)]
            #[expect(clippy::cast_sign_loss)]
            n_rows: (bounding_box.height() / tile_size).round() as u32,
            #[expect(clippy::cast_possible_truncation)]
            #[expect(clippy::cast_sign_loss)]
            n_cols: (bounding_box.width() / tile_size).round() as u32,
            n_subdivisions: 5,
            wrap_x: true, // I've not seen this overridden in Valhalla so far...
        }
    }

    /// The total number of tiles in the tiling system.
    #[inline]
    pub const fn tile_count(&self) -> u32 {
        self.n_rows * self.n_cols
    }

    /// The southwest corner of a tile, computed exactly as Valhalla's `Tiles::Base` does.
    ///
    /// Valhalla computes the corner in double precision
    /// and stores it in the tile header in single precision,
    /// so we do the same to match it bit for bit.
    ///
    /// Returns `None` if the tile ID is out of range for this tiling system.
    #[inline]
    pub fn base_coordinate(&self, tile_id: u64) -> Option<Coord<f32>> {
        let tile_id = u32::try_from(tile_id)
            .ok()
            .filter(|&id| id < self.tile_count())?;
        let row = tile_id / self.n_cols;
        let col = tile_id % self.n_cols;
        let tile_size = f64::from(self.tile_size);
        let min = self.bounding_box.min();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Valhalla narrows the corner to single precision in the same way."
        )]
        Some(coord! {
            x: (f64::from(min.x) + f64::from(col) * tile_size) as f32,
            y: (f64::from(min.y) + f64::from(row) * tile_size) as f32,
        })
    }

    pub const fn min_zoom(&self) -> u8 {
        if self.tile_size > 1.0 {
            0
        } else if self.tile_size > 0.25 {
            6
        } else {
            12
        }
    }
}

/// A level in the Valhalla tile hierarchy.
pub struct TileLevel {
    /// The hierarchy level.
    pub level: u8,
    /// The minimum class of road contained in this hierarchy level.
    pub minimum_road_class: RoadClass,
    /// The human-readable name of the level.
    pub name: &'static str,
    /// The tiling system used for this level.
    pub tiling_system: TilingSystem,
}

impl TileLevel {
    /// Returns the base IDs (row-major) of the tiles in this level
    /// whose geographic extent intersects the axis-aligned bbox:
    /// `lon ∈ [west, east], lat ∈ [south, north]`.
    ///
    /// Longitudes are expected in degrees, in the `[-180, 180]` range.
    /// If `west > east`, the bbox is assumed to wrap across the antimeridian
    /// and is treated as the union of `[west, 180] ∪ [-180, east]`.
    ///
    /// # Correctness
    ///
    /// This function does not sanity check the input beyond never panicking.
    /// It is the responsibility of the caller to ensure that the coordinates are in the range for
    /// latitude and longitude, and that they describe a valid bounding box.
    /// Coordinates past the edge of the world (including infinite ones) are clamped to the edge,
    /// and a bbox with a NaN coordinate intersects no tiles.
    pub fn tiles_intersecting_bbox<N: CoordFloat + FromPrimitive>(
        &self,
        north: N,
        east: N,
        south: N,
        west: N,
    ) -> Vec<GraphId> {
        // These conversions are exact for f32 and f64.
        // They could only fail for an exotic float type that cannot represent small numbers,
        // which has no meaningful tiles anyway.
        let (Some(size), Some(n_90), Some(n_180)) = (
            N::from(self.tiling_system.tile_size),
            N::from_u8(90),
            N::from_u8(180),
        ) else {
            return Vec::new();
        };
        let width = i64::from(self.tiling_system.n_cols);
        let height = i64::from(self.tiling_system.n_rows);

        // Map lon from [-180, 180] to [0, 360], and lat from [-90, 90] to [0, 180],
        // and then to tile columns and rows.
        let (Some(west_x), Some(east_x), Some(min_y), Some(max_y)) = (
            grid_index((west + n_180) / size, width),
            grid_index((east + n_180) / size, width),
            grid_index((south + n_90) / size, height),
            grid_index((north + n_90) / size, height),
        ) else {
            return Vec::new();
        };

        // A bbox that wraps across the antimeridian covers [west, 180] and then [-180, east].
        // We list the two parts one after the other, each in row-major order.
        // (The second range is empty unless the bbox wraps.)
        #[expect(
            clippy::reversed_empty_ranges,
            reason = "the empty range stands in for the missing second part of the bbox"
        )]
        let column_ranges = if west > east {
            [west_x..=width - 1, 0..=east_x]
        } else {
            [west_x..=east_x, 1..=0]
        };

        column_ranges
            .into_iter()
            .flat_map(|columns| {
                (min_y..=max_y).flat_map(move |y| columns.clone().map(move |x| (x, y)))
            })
            .filter_map(|(x, y)| {
                // Both are clamped to the grid, so the index is non-negative and in range,
                // and fits in a graph ID; this never filters anything out.
                let tile_index = u64::try_from(y * width + x).ok()?;
                GraphId::try_from_components(self.level, tile_index, 0).ok()
            })
            .collect()
    }
}

/// Converts a position in units of tiles to a tile row or column, clamped to the grid.
///
/// Positions past either edge of the grid (including infinite ones) map to that edge.
/// Returns `None` for NaN, which is nowhere on the grid.
fn grid_index<N: CoordFloat>(position: N, count: i64) -> Option<i64> {
    if position.is_nan() {
        return None;
    }
    // Conversion only fails for values out of the i64 range, which we clamp anyway.
    let index = position
        .floor()
        .to_i64()
        .unwrap_or(if position.is_sign_negative() {
            0
        } else {
            count - 1
        });
    Some(index.clamp(0, count - 1))
}

/// A concrete instantiation of the standard Valhalla tile system.
///
/// While other systems are technically possible, you should probably stick to the canonical one.
pub static STANDARD_LEVELS: LazyLock<[TileLevel; 3]> = LazyLock::new(|| {
    [
        TileLevel {
            level: 0,
            minimum_road_class: RoadClass::Primary,
            name: "highway",
            tiling_system: TilingSystem::new(
                Rect::new(
                    coord! { x: -180f32, y: -90f32 },
                    coord! { x: 180f32, y: 90f32 },
                ),
                4.0,
            ),
        },
        TileLevel {
            level: 1,
            minimum_road_class: RoadClass::Tertiary,
            name: "arterial",
            tiling_system: TilingSystem::new(
                Rect::new(
                    coord! { x: -180f32, y: -90f32 },
                    coord! { x: 180f32, y: 90f32 },
                ),
                1.0,
            ),
        },
        TileLevel {
            level: 2,
            minimum_road_class: RoadClass::ServiceOther,
            name: "local",
            tiling_system: TilingSystem::new(
                Rect::new(
                    coord! { x: -180f32, y: -90f32 },
                    coord! { x: 180f32, y: 90f32 },
                ),
                0.25,
            ),
        },
    ]
});

pub static TRANSIT_LEVEL: LazyLock<TileLevel> = LazyLock::new(|| TileLevel {
    level: 3,
    minimum_road_class: RoadClass::ServiceOther,
    name: "transit",
    tiling_system: TilingSystem::new(
        Rect::new(
            coord! { x: -180f32, y: -90f32 },
            coord! { x: 180f32, y: 90f32 },
        ),
        0.25,
    ),
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile_hierarchy::STANDARD_LEVELS;
    use proptest::{prop_assert, proptest};

    /// Helper to compute the base tile `GraphId` at (x, y) for a given level.
    fn base_tile_id(level: &TileLevel, x: i64, y: i64) -> GraphId {
        let width = i64::from(level.tiling_system.n_cols);
        let tile_index = u64::try_from(y * width + x).expect("non-negative tile index");
        GraphId::try_from_components(level.level, tile_index, 0)
            .expect("valid base tile GraphId in test")
    }

    #[test]
    fn bbox_inside_single_tile() {
        let level = &STANDARD_LEVELS[0];
        let size = f64::from(level.tiling_system.tile_size);
        let width = i64::from(level.tiling_system.n_cols);
        let height = i64::from(level.tiling_system.n_rows);
        assert!(width > 0 && height > 0);

        // Pick a tile somewhere in the grid (center-ish if possible).
        let x = width / 2;
        let y = height / 2;

        let west = -180.0 + f64::from(u32::try_from(x).expect("small")) * size;
        let east = west + size;
        let south = -90.0 + f64::from(u32::try_from(y).expect("small")) * size;
        let north = south + size;

        // Bbox strictly inside that tile.
        let ids: Vec<GraphId> = level.tiles_intersecting_bbox(
            north - 0.1 * size,
            east - 0.1 * size,
            south + 0.1 * size,
            west + 0.1 * size,
        );

        assert_eq!(ids, vec![base_tile_id(level, x, y)]);
    }

    #[test]
    fn bbox_spans_a_row_of_tiles() {
        let level = &STANDARD_LEVELS[0];
        let size = f64::from(level.tiling_system.tile_size);
        let width = i64::from(level.tiling_system.n_cols);
        assert!(width >= 2);

        // Use row 0: lat band from -90 to -90+size.
        let y = 0_i64;
        let south = -90.0 + 0.1 * size;
        let north = -90.0 + 0.9 * size;

        // Bbox spanning almost entire longitude range, no wrap.
        let west = -179.9;
        let east = 179.9;

        let mut ids: Vec<GraphId> = level.tiles_intersecting_bbox(north, east, south, west);
        ids.sort_by_key(GraphId::value);

        let expected: Vec<GraphId> = (0..width).map(|x| base_tile_id(level, x, y)).collect();
        assert_eq!(ids, expected);
    }

    #[test]
    fn bbox_crosses_antimeridian_hits_edge_tiles() {
        let level = &STANDARD_LEVELS[0];
        let size = f64::from(level.tiling_system.tile_size);
        let width = i64::from(level.tiling_system.n_cols);
        assert!(width >= 2);

        // Use bottom row
        let y = 0_i64;
        let south = -90.0 + 0.1 * size;
        let north = -90.0 + 0.9 * size;

        // Bbox that crosses the antimeridian: west ~ +179, east ~ -179.
        let west = 179.0;
        let east = -179.0;

        let ids: Vec<GraphId> = level.tiles_intersecting_bbox(north, east, south, west);

        let first = base_tile_id(level, 0, y);
        let last = base_tile_id(level, width - 1, y);

        // It should include only the leftmost and rightmost tiles in that row.
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&first));
        assert!(ids.contains(&last));
    }

    #[test]
    fn bbox_on_tile_boundaries_includes_neighboring_tiles_level_0() {
        let level = &STANDARD_LEVELS[0];
        let size = f64::from(level.tiling_system.tile_size);

        // Use bottom-left tile at (x=0, y=0):
        // lon: [-180, -180 + size], lat: [-90, -90 + size]
        let west = -180.0;
        let east = -180.0 + size;
        let south = -90.0;
        let north = -90.0 + size;

        let ids: Vec<GraphId> = level.tiles_intersecting_bbox(north, east, south, west);
        assert_eq!(ids.len(), 4);

        assert!(ids.contains(&base_tile_id(level, 0, 0)));
        assert!(ids.contains(&base_tile_id(level, 1, 0)));
        assert!(ids.contains(&base_tile_id(level, 0, 1)));
        assert!(ids.contains(&base_tile_id(level, 1, 1)));
    }

    #[test]
    fn bbox_on_tile_boundaries_includes_neighboring_tiles_level_1() {
        // Same as the above test area-wise, but covers 25 tiles at level 1 instead!
        let level = &STANDARD_LEVELS[1];
        let size = f64::from(level.tiling_system.tile_size);

        // Use bottom-left tile at (x=0, y=0):
        // lon: [-180, -180 + size], lat: [-90, -90 + size]
        let west = -180.0;
        let east = -180.0 + size;
        let south = -90.0;
        let north = -90.0 + size;

        let ids: Vec<GraphId> = level.tiles_intersecting_bbox(north, east, south, west);
        assert_eq!(ids.len(), 4);

        assert!(ids.contains(&base_tile_id(level, 0, 0)));
        assert!(ids.contains(&base_tile_id(level, 1, 0)));
        assert!(ids.contains(&base_tile_id(level, 0, 1)));
        assert!(ids.contains(&base_tile_id(level, 1, 1)));
    }

    #[test]
    fn test_base_tile_id() {
        // Test the base_tile_id function
        let level = &STANDARD_LEVELS[0];
        assert_eq!(
            base_tile_id(level, 0, 0),
            GraphId::try_from_components(0, 0, 0).unwrap()
        );
        assert_eq!(
            base_tile_id(level, 1, 0),
            GraphId::try_from_components(0, 1, 0).unwrap()
        );
        assert_eq!(
            base_tile_id(level, 0, 1),
            GraphId::try_from_components(0, 90, 0).unwrap()
        );
        assert_eq!(
            base_tile_id(level, 1, 1),
            GraphId::try_from_components(0, 91, 0).unwrap()
        );

        // Same idea for level 1
        let level = &STANDARD_LEVELS[1];
        assert_eq!(
            base_tile_id(level, 0, 0),
            GraphId::try_from_components(1, 0, 0).unwrap()
        );
        assert_eq!(
            base_tile_id(level, 1, 0),
            GraphId::try_from_components(1, 1, 0).unwrap()
        );
        assert_eq!(
            base_tile_id(level, 0, 1),
            GraphId::try_from_components(1, 360, 0).unwrap()
        );
        assert_eq!(
            base_tile_id(level, 1, 1),
            GraphId::try_from_components(1, 361, 0).unwrap()
        );
    }

    #[test]
    fn bbox_with_nan_intersects_nothing() {
        let level = &STANDARD_LEVELS[2];
        for (north, east, south, west) in [
            (f64::NAN, 1.0, 0.0, 0.0),
            (1.0, f64::NAN, 0.0, 0.0),
            (1.0, 1.0, f64::NAN, 0.0),
            (1.0, 1.0, 0.0, f64::NAN),
        ] {
            assert!(
                level
                    .tiles_intersecting_bbox(north, east, south, west)
                    .is_empty()
            );
        }
    }

    #[test]
    fn bbox_with_infinite_coordinates_clamps_to_the_edge() {
        let level = &STANDARD_LEVELS[0];
        let width = i64::from(level.tiling_system.n_cols);
        let height = i64::from(level.tiling_system.n_rows);

        // A bbox running off the north-east edge of the world.
        let ids = level.tiles_intersecting_bbox(f64::INFINITY, f64::INFINITY, 89.0, 179.0);
        assert_eq!(ids, vec![base_tile_id(level, width - 1, height - 1)]);

        // Infinite west and east longitudes "wrap" (west > east) but must not recurse forever.
        let ids = level.tiles_intersecting_bbox(-89.0, -f64::INFINITY, -90.0, f64::INFINITY);
        assert_eq!(
            ids,
            vec![base_tile_id(level, width - 1, 0), base_tile_id(level, 0, 0)]
        );

        // Out-of-range finite longitudes behave the same way.
        let ids = level.tiles_intersecting_bbox(-89.0, -200.0, -90.0, 200.0);
        assert_eq!(
            ids,
            vec![base_tile_id(level, width - 1, 0), base_tile_id(level, 0, 0)]
        );
    }

    proptest! {
        #[test]
        fn bbox_never_panics(
            north in proptest::num::f64::ANY,
            east in proptest::num::f64::ANY,
            south in proptest::num::f64::ANY,
            west in proptest::num::f64::ANY,
        ) {
            let level = &STANDARD_LEVELS[0];
            let ids = level.tiles_intersecting_bbox(north, east, south, west);
            let tile_count = u64::from(level.tiling_system.tile_count());
            prop_assert!(ids.iter().all(|id| id.tile_id() < tile_count));
        }
    }

    #[test]
    fn test_base_coordinate() {
        // Cross-checked with the base_ll of the Andorra fixture tiles built by Valhalla 3.9.1.
        let cases = [
            (0, 3015, coord! {x: 0.0, y: 42.0}),
            (1, 47701, coord! {x: 1.0, y: 42.0}),
            (2, 762_485, coord! {x: 1.25, y: 42.25}),
            (2, 763_927, coord! {x: 1.75, y: 42.5}),
        ];
        for (level, tile_id, expected) in cases {
            assert_eq!(
                STANDARD_LEVELS[level]
                    .tiling_system
                    .base_coordinate(tile_id),
                Some(expected),
                "level {level} tile {tile_id}"
            );
        }

        let tiling = &STANDARD_LEVELS[2].tiling_system;
        assert_eq!(
            tiling.base_coordinate(0),
            Some(coord! {x: -180.0, y: -90.0})
        );
        let last = u64::from(tiling.tile_count()) - 1;
        assert_eq!(
            tiling.base_coordinate(last),
            Some(coord! {x: 179.75, y: 89.75})
        );
        assert_eq!(tiling.base_coordinate(last + 1), None);
        assert_eq!(tiling.base_coordinate(u64::MAX), None);
    }
}
