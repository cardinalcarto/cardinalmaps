//! # Tileset consistency checks
//!
//! Since Valhalla 3.9, every tile header carries a tileset build ID,
//! which is derived from the hashes of *all* tiles in the build
//! (see [`BuildId`] and [`TileHash`]).
//! Every tile from one build has the same build ID,
//! so tiles from different builds can be told apart.
//!
//! This guards offline regions against mixed or partially updated downloads:
//!
//! - [`TarballTileProvider::check_build_ids`] is cheap (it only reads tile headers),
//!   and detects tiles from different builds.
//! - [`TarballTileProvider::verify_tileset`] reads and hashes every tile,
//!   which additionally detects corrupt tile data,
//!   and recomputes the build ID of the tiles in the extract.

use crate::GraphId;
use crate::graph_tile::{BuildId, GraphTileDecodingError, GraphTileHeader, TileHash};
use crate::tile_provider::{GraphTileProviderError, TarballTileProvider};
use std::collections::HashMap;
use zerocopy::FromBytes;

/// The tileset build IDs in the headers of a set of tiles,
/// as reported by [`TarballTileProvider::check_build_ids`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileBuildIds {
    /// The tiles grouped by build ID.
    ///
    /// Sorted by group size (largest first), then by build ID;
    /// the tiles in each group are sorted by ID.
    groups: Vec<(BuildId, Vec<GraphId>)>,
}

impl TileBuildIds {
    /// Groups tiles by build ID.
    fn new(tiles: impl IntoIterator<Item = (GraphId, BuildId)>) -> Self {
        let mut by_build_id: HashMap<BuildId, Vec<GraphId>> = HashMap::new();
        for (graph_id, build_id) in tiles {
            by_build_id.entry(build_id).or_default().push(graph_id);
        }
        let mut groups: Vec<_> = by_build_id.into_iter().collect();
        for (_, tiles) in &mut groups {
            tiles.sort_unstable();
        }
        groups.sort_unstable_by(|(a_id, a_tiles), (b_id, b_tiles)| {
            b_tiles
                .len()
                .cmp(&a_tiles.len())
                .then(a_id.value().cmp(&b_id.value()))
        });
        Self { groups }
    }

    /// Do all tiles share one build ID?
    ///
    /// This is true for an empty tileset.
    #[inline]
    pub fn is_consistent(&self) -> bool {
        self.groups.len() <= 1
    }

    /// The build ID of the tileset, if all tiles share one.
    #[inline]
    pub fn build_id(&self) -> Option<BuildId> {
        match self.groups.as_slice() {
            [(build_id, _)] => Some(*build_id),
            _ => None,
        }
    }

    /// The build ID shared by the most tiles (ties go to the smallest build ID).
    ///
    /// For an inconsistent tileset, this is the most likely build ID of the intended tileset.
    #[inline]
    pub fn majority_build_id(&self) -> Option<BuildId> {
        self.groups.first().map(|(build_id, _)| *build_id)
    }

    /// The tiles grouped by build ID,
    /// sorted by group size (largest first), then by build ID.
    #[inline]
    pub fn groups(&self) -> &[(BuildId, Vec<GraphId>)] {
        &self.groups
    }

    /// The tiles whose build ID differs from [`TileBuildIds::majority_build_id`],
    /// with their build IDs.
    #[inline]
    pub fn inconsistent_tiles(&self) -> impl Iterator<Item = (GraphId, BuildId)> + '_ {
        self.groups
            .iter()
            .skip(1)
            .flat_map(|(build_id, tiles)| tiles.iter().map(move |graph_id| (*graph_id, *build_id)))
    }
}

/// A tile whose data does not match the tile hash in its header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TileHashMismatch {
    /// The base graph ID of the tile.
    pub graph_id: GraphId,
    /// The tile hash in the header.
    pub stored: TileHash,
    /// The hash of the tile data.
    pub computed: TileHash,
}

/// The result of [`TarballTileProvider::verify_tileset`].
///
/// # Regional extracts
///
/// A regional extract is usually cut from a larger build (for example, the planet),
/// so its tiles carry the build ID of that larger build,
/// which does *not* equal the build ID computed from the tiles in the extract.
/// That's expected; only a complete tileset (all the tiles of its build) matches
/// (see [`TilesetVerification::is_complete_tileset`]).
/// For a regional extract, check [`TilesetVerification::is_intact`] instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TilesetVerification {
    build_ids: TileBuildIds,
    corrupt_tiles: Vec<TileHashMismatch>,
    unhashed_tiles: Vec<GraphId>,
    computed_build_id: BuildId,
}

impl TilesetVerification {
    /// The build IDs in the tile headers.
    #[inline]
    pub fn build_ids(&self) -> &TileBuildIds {
        &self.build_ids
    }

    /// The tiles whose data does not match the hash in their header, sorted by ID.
    #[inline]
    pub fn corrupt_tiles(&self) -> &[TileHashMismatch] {
        &self.corrupt_tiles
    }

    /// The tiles without a checksum in their header, sorted by ID.
    ///
    /// Tiles built before Valhalla 3.9 have no checksum (the header field is zero),
    /// so their data can't be verified.
    #[inline]
    pub fn unhashed_tiles(&self) -> &[GraphId] {
        &self.unhashed_tiles
    }

    /// The build ID computed from the hashes of the tile data (see [`BuildId::of_tileset`]).
    #[inline]
    pub fn computed_build_id(&self) -> BuildId {
        self.computed_build_id
    }

    /// Are all tiles from one build, with data that matches their hashes?
    ///
    /// This is the check to use for a regional extract.
    /// Tiles without a checksum fail it, since they can't be verified.
    #[inline]
    pub fn is_intact(&self) -> bool {
        self.build_ids.is_consistent()
            && self.corrupt_tiles.is_empty()
            && self.unhashed_tiles.is_empty()
    }

    /// Is this the complete tileset of its build?
    ///
    /// In addition to [`TilesetVerification::is_intact`],
    /// the build ID in the headers must equal the build ID computed from the tiles,
    /// which only holds when no tile of the build is missing (and none was added).
    /// Regional extracts cut from a larger build fail this check
    /// (see [regional extracts](TilesetVerification#regional-extracts)).
    #[inline]
    pub fn is_complete_tileset(&self) -> bool {
        self.is_intact() && self.build_ids.build_id() == Some(self.computed_build_id)
    }
}

/// Reads the header at the start of a tile's bytes.
fn read_header(
    graph_id: GraphId,
    bytes: &[u8],
) -> Result<&GraphTileHeader, GraphTileProviderError> {
    let (header, _) = GraphTileHeader::ref_from_prefix(bytes).map_err(|_| {
        GraphTileProviderError::InvalidTile {
            graph_id,
            source: GraphTileDecodingError::SliceLength,
        }
    })?;
    let actual = header.graph_id().tile_base_id();
    if actual != graph_id {
        return Err(GraphTileProviderError::TileIdMismatch {
            expected: graph_id,
            actual,
        });
    }
    Ok(header)
}

impl<const CACHE: bool> TarballTileProvider<false, CACHE> {
    /// Checks that all tiles in the tarball share one tileset build ID.
    ///
    /// This only reads tile headers, so it does not detect corrupt tile data;
    /// use [`TarballTileProvider::verify_tileset`] for that.
    /// See [`TilesetVerification`] for what to expect of regional extracts.
    ///
    /// # Performance
    ///
    /// This reads the header of every tile, which touches one page per tile.
    /// It's cheap enough to run every time an extract is opened:
    /// even a planet extract only has a few hundred thousand tiles,
    /// and a regional extract a few hundred.
    ///
    /// # Errors
    ///
    /// Fails if a tile is too small to contain a header,
    /// or its header names a different tile than the index.
    pub fn check_build_ids(&self) -> Result<TileBuildIds, GraphTileProviderError> {
        let mut build_ids = Vec::with_capacity(self.tile_count());
        for (graph_id, pointer) in self.tile_pointers_in_file_order() {
            // SAFETY: The pointer lies within the memory map (checked when the index was loaded).
            // The backing memory must not be mutated while the slice is alive,
            // which is the documented contract of this provider.
            let bytes = unsafe { pointer.as_tile_bytes() };
            build_ids.push((graph_id, read_header(graph_id, bytes)?.build_id()));
        }
        Ok(TileBuildIds::new(build_ids))
    }

    /// The header of one tile (for example, to read its build ID),
    /// without decoding the rest of the tile.
    ///
    /// # Performance
    ///
    /// This touches only the tile's first page,
    /// so a sample of headers is cheap even from a planet extract that isn't in the page cache.
    ///
    /// # Errors
    ///
    /// [`GraphTileProviderError::TileNotAvailable`] if the tile isn't in the tarball,
    /// and otherwise fails as [`TarballTileProvider::check_build_ids`] does.
    pub fn tile_header(
        &self,
        graph_id: GraphId,
    ) -> Result<GraphTileHeader, GraphTileProviderError> {
        let base_id = graph_id.tile_base_id();
        let pointer = self.get_pointer_for_tile_containing(base_id)?;
        // SAFETY: The pointer lies within the memory map (checked when the index was loaded).
        // The backing memory must not be mutated while the slice is alive,
        // which is the documented contract of this provider.
        let bytes = unsafe { pointer.as_tile_bytes() };
        read_header(base_id, bytes).cloned()
    }

    /// Verifies every tile in the tarball against its hash,
    /// and recomputes the tileset build ID.
    ///
    /// This detects tiles from different builds (like [`TarballTileProvider::check_build_ids`]),
    /// corrupt tile data, and (for a complete tileset) missing or extra tiles.
    /// See [`TilesetVerification`] for what to expect of regional extracts.
    ///
    /// # Performance
    ///
    /// This reads and hashes (MD5) every byte of every tile, in file order.
    /// Expect it to take about as long as reading the file plus a second or two per gigabyte
    /// on one core; a planet extract takes minutes.
    /// On a phone, run it once after a download completes (on a background thread),
    /// not every time the extract is opened.
    /// The provider is [`Sync`], so routing can continue meanwhile.
    ///
    /// The pages read along the way stay in the page cache until the operating system reclaims them.
    ///
    /// # Errors
    ///
    /// Fails if a tile is too small to contain a header,
    /// or its header names a different tile than the index.
    pub fn verify_tileset(&self) -> Result<TilesetVerification, GraphTileProviderError> {
        let mut build_ids = Vec::with_capacity(self.tile_count());
        let mut computed_hashes = Vec::with_capacity(self.tile_count());
        let mut corrupt_tiles = Vec::new();
        let mut unhashed_tiles = Vec::new();

        for (graph_id, pointer) in self.tile_pointers_in_file_order() {
            // SAFETY: The pointer lies within the memory map (checked when the index was loaded).
            // The backing memory must not be mutated while the slice is alive,
            // which is the documented contract of this provider.
            let bytes = unsafe { pointer.as_tile_bytes() };
            let header = read_header(graph_id, bytes)?;
            build_ids.push((graph_id, header.build_id()));

            // The hash covers everything after the header.
            let data = bytes
                .get(size_of::<GraphTileHeader>()..)
                .unwrap_or_default();
            let computed = TileHash::of_tile_data(data);
            computed_hashes.push(computed);

            let stored = header.tile_hash();
            if stored == TileHash::default() && header.build_id() == BuildId::default() {
                unhashed_tiles.push(graph_id);
            } else if stored != computed {
                corrupt_tiles.push(TileHashMismatch {
                    graph_id,
                    stored,
                    computed,
                });
            }
        }

        corrupt_tiles.sort_unstable_by_key(|mismatch| mismatch.graph_id);
        unhashed_tiles.sort_unstable();

        Ok(TilesetVerification {
            build_ids: TileBuildIds::new(build_ids),
            corrupt_tiles,
            unhashed_tiles,
            computed_build_id: BuildId::of_tileset(computed_hashes),
        })
    }
}

// Tragically, memory maps are not supported by Miri!
#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::graph_tile::GraphTile;
    use crate::tile_provider::GraphTileProvider;
    use crate::tile_provider::tarball::test_support::{
        TempFile, build_extract, fixture_tile, fixture_tile_ids,
    };
    use std::path::PathBuf;

    /// The build ID that mainline Valhalla 3.9.1 stamped into every fixture tile.
    const FIXTURE_BUILD_ID: BuildId = BuildId::new(53174);

    fn fixture_tiles() -> Vec<(GraphId, Vec<u8>)> {
        fixture_tile_ids()
            .into_iter()
            .map(|id| (id, fixture_tile(id)))
            .collect()
    }

    fn open(tiles: &[(GraphId, Vec<u8>)]) -> (TempFile, TarballTileProvider<false, false>) {
        let file = TempFile::new(&build_extract(tiles));
        let provider = TarballTileProvider::new_readonly(file.path()).expect("valid extract");
        (file, provider)
    }

    #[test]
    fn fixture_tileset_is_complete() {
        // The fixture was built and packed by mainline Valhalla 3.9.1,
        // so this cross-checks our build ID computation against mainline's.
        let provider = TarballTileProvider::new_readonly(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("andorra-tiles.tar"),
        )
        .expect("valid extract");

        let build_ids = provider.check_build_ids().expect("readable headers");
        assert!(build_ids.is_consistent());
        assert_eq!(build_ids.build_id(), Some(FIXTURE_BUILD_ID));
        assert_eq!(build_ids.groups()[0].1.len(), 7);
        assert_eq!(build_ids.inconsistent_tiles().count(), 0);

        let verification = provider.verify_tileset().expect("readable tiles");
        assert!(verification.is_intact());
        assert!(verification.corrupt_tiles().is_empty());
        assert!(verification.unhashed_tiles().is_empty());
        assert_eq!(verification.computed_build_id(), FIXTURE_BUILD_ID);
        assert!(verification.is_complete_tileset());

        // The cached provider gives the same answers
        // SAFETY: Nothing mutates the fixture.
        let immutable = unsafe {
            TarballTileProvider::new_immutable(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("fixtures")
                    .join("andorra-tiles.tar"),
            )
        }
        .expect("valid extract");
        assert_eq!(immutable.check_build_ids().expect("headers"), build_ids);
        assert_eq!(immutable.verify_tileset().expect("tiles"), verification);
    }

    #[test]
    fn reads_single_headers() {
        let provider = TarballTileProvider::new_readonly(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("andorra-tiles.tar"),
        )
        .expect("valid extract");
        for id in fixture_tile_ids() {
            // Any ID in the tile names it.
            let header = provider
                .tile_header(id.with_feature_index(3).expect("valid ID"))
                .expect("a header");
            assert_eq!(header.graph_id(), id);
            assert_eq!(header.build_id(), FIXTURE_BUILD_ID);
            let full = provider
                .with_tile_containing(id, |tile| tile.header().clone())
                .expect("tile");
            assert_eq!(header, full);
        }
        let missing = GraphId::try_from_components(0, 3016, 0).expect("valid tile ID");
        assert!(matches!(
            provider.tile_header(missing),
            Err(GraphTileProviderError::TileNotAvailable(id)) if id == missing
        ));
    }

    #[test]
    fn mixed_builds_are_detected() {
        // Adding predicted speeds to tile 0/003/015 changed its data,
        // so mainline stamped a new build ID into it (see tools/oracle/README.md).
        let replaced = GraphId::try_from_components(0, 3015, 0).expect("valid tile ID");
        let newer = std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("andorra-tiles-with-traffic")
                .join("0/003/015.gph"),
        )
        .expect("Unable to read fixture");
        let mut tiles = fixture_tiles();
        for (id, bytes) in &mut tiles {
            if *id == replaced {
                bytes.clone_from(&newer);
            }
        }
        let (_file, provider) = open(&tiles);
        let newer_build_id = provider
            .with_tile_containing(replaced, |tile| tile.header().build_id())
            .expect("tile");
        assert_ne!(newer_build_id, FIXTURE_BUILD_ID);

        let build_ids = provider.check_build_ids().expect("readable headers");
        assert!(!build_ids.is_consistent());
        assert_eq!(build_ids.build_id(), None);
        assert_eq!(build_ids.majority_build_id(), Some(FIXTURE_BUILD_ID));
        assert_eq!(
            build_ids.inconsistent_tiles().collect::<Vec<_>>(),
            vec![(replaced, newer_build_id)]
        );

        let verification = provider.verify_tileset().expect("readable tiles");
        // Each tile is internally sound; they just don't belong together.
        assert!(verification.corrupt_tiles().is_empty());
        assert!(!verification.is_intact());
        assert!(!verification.is_complete_tileset());
    }

    #[test]
    fn corrupt_tile_data_is_detected() {
        let corrupted = GraphId::try_from_components(2, 762_486, 0).expect("valid tile ID");
        let mut tiles = fixture_tiles();
        for (id, bytes) in &mut tiles {
            if *id == corrupted {
                let last = bytes.len() - 1;
                bytes[last] ^= 0xff;
            }
        }
        let (_file, provider) = open(&tiles);

        // The headers are untouched, so the cheap check can't tell.
        let build_ids = provider.check_build_ids().expect("readable headers");
        assert_eq!(build_ids.build_id(), Some(FIXTURE_BUILD_ID));

        let verification = provider.verify_tileset().expect("readable tiles");
        assert!(!verification.is_intact());
        let [mismatch] = verification.corrupt_tiles() else {
            panic!("Expected one corrupt tile: {verification:?}");
        };
        assert_eq!(mismatch.graph_id, corrupted);
        assert_ne!(mismatch.stored, mismatch.computed);
        assert_ne!(verification.computed_build_id(), FIXTURE_BUILD_ID);
    }

    #[test]
    fn regional_extract_is_intact_but_not_complete() {
        // Leaving out a tile is what a regional extract of a larger build looks like.
        let left_out = GraphId::try_from_components(2, 763_926, 0).expect("valid tile ID");
        let tiles: Vec<_> = fixture_tiles()
            .into_iter()
            .filter(|(id, _)| *id != left_out)
            .collect();
        let (_file, provider) = open(&tiles);

        let build_ids = provider.check_build_ids().expect("readable headers");
        assert_eq!(build_ids.build_id(), Some(FIXTURE_BUILD_ID));

        let verification = provider.verify_tileset().expect("readable tiles");
        assert!(verification.is_intact());
        assert_ne!(verification.computed_build_id(), FIXTURE_BUILD_ID);
        assert!(!verification.is_complete_tileset());
    }

    #[test]
    fn tiles_without_checksums_are_reported() {
        let unhashed = GraphId::try_from_components(2, 763_927, 0).expect("valid tile ID");
        let mut tiles = fixture_tiles();
        for (id, bytes) in &mut tiles {
            if *id == unhashed {
                // Zero the checksum field, as in tiles built before Valhalla 3.9
                let (header, _) =
                    GraphTileHeader::ref_from_prefix(bytes.as_slice()).expect("header");
                let checksum =
                    (u64::from(header.build_id().value()) << 48) | header.tile_hash().value();
                let needle = checksum.to_le_bytes();
                let header_len = size_of::<GraphTileHeader>();
                let pos = bytes[..header_len]
                    .windows(8)
                    .position(|w| w == needle)
                    .expect("checksum in header");
                bytes[pos..pos + 8].fill(0);
            }
        }
        let (_file, provider) = open(&tiles);

        let build_ids = provider.check_build_ids().expect("readable headers");
        assert_eq!(
            build_ids.inconsistent_tiles().collect::<Vec<_>>(),
            vec![(unhashed, BuildId::new(0))]
        );

        let verification = provider.verify_tileset().expect("readable tiles");
        assert_eq!(verification.unhashed_tiles(), &[unhashed]);
        assert!(verification.corrupt_tiles().is_empty());
        assert!(!verification.is_intact());
    }

    #[test]
    fn unreadable_headers_are_errors() {
        let id = GraphId::try_from_components(2, 763_927, 0).expect("valid tile ID");
        let (_file, provider) = open(&[(id, vec![0u8; 16])]);
        assert!(matches!(
            provider.check_build_ids(),
            Err(GraphTileProviderError::InvalidTile { graph_id, .. }) if graph_id == id
        ));
        assert!(matches!(
            provider.verify_tileset(),
            Err(GraphTileProviderError::InvalidTile { graph_id, .. }) if graph_id == id
        ));

        let other = GraphId::try_from_components(2, 762_486, 0).expect("valid tile ID");
        let (_file, provider) = open(&[(id, fixture_tile(other))]);
        assert!(matches!(
            provider.check_build_ids(),
            Err(GraphTileProviderError::TileIdMismatch { expected, actual }) if expected == id && actual == other
        ));
    }

    #[test]
    fn build_id_groups_are_deterministic() {
        let a = GraphId::try_from_components(2, 1, 0).expect("valid tile ID");
        let b = GraphId::try_from_components(2, 2, 0).expect("valid tile ID");
        let c = GraphId::try_from_components(2, 3, 0).expect("valid tile ID");
        let ids = TileBuildIds::new([
            (c, BuildId::new(9)),
            (b, BuildId::new(7)),
            (a, BuildId::new(9)),
        ]);
        assert_eq!(
            ids.groups(),
            &[(BuildId::new(9), vec![a, c]), (BuildId::new(7), vec![b])]
        );
        // Ties go to the smallest build ID
        let tie = TileBuildIds::new([(a, BuildId::new(9)), (b, BuildId::new(7))]);
        assert_eq!(tie.majority_build_id(), Some(BuildId::new(7)));
        assert_eq!(
            tie.inconsistent_tiles().collect::<Vec<_>>(),
            vec![(a, BuildId::new(9))]
        );
        let empty = TileBuildIds::new([]);
        assert!(empty.is_consistent());
        assert_eq!(empty.build_id(), None);
    }
}
