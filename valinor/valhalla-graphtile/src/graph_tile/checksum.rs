//! # Tile hashes and tileset build IDs
//!
//! Since Valhalla 3.9, every graph tile header carries a 64-bit checksum field.
//! The low 48 bits are a hash of the tile's own data ([`TileHash`]),
//! and the high 16 bits identify the tileset build that the tile belongs to ([`BuildId`]).
//!
//! The tile hash depends only on the bytes of the tile after the header,
//! so it can be computed for a single tile.
//! The build ID is derived from the hashes of *every* tile in the tileset,
//! so changing a single tile changes the build ID stamped into every header.
//! Valhalla uses it to detect tiles from mixed builds
//! (for example, when fetching tiles from a remote server).
//!
//! The algorithms here mirror `fold_md5` in `mjolnir/graphtilebuilder.cc`
//! and `compute_tileset_build_id` in `mjolnir/util.cc`.

use md5::{Digest, Md5};

/// The number of low bits of the header checksum that hold the tile hash.
///
/// This is `kTileHashBits` in Valhalla.
const TILE_HASH_BITS: u32 = 48;

/// Masks the tile hash bits of the header checksum.
const TILE_HASH_MASK: u64 = (1 << TILE_HASH_BITS) - 1;

/// The 48-bit hash of a graph tile's data.
///
/// This is computed by Valhalla (and Valinor) as follows:
///
/// 1. Take the MD5 digest of every byte in the tile after the header.
/// 2. Split the digest into two big-endian 64-bit words, `lo` (bytes 0-7) and `hi` (bytes 8-15).
/// 3. Fold them as `lo ^ (hi + 0x9e3779b97f4a7c15 + (lo << 12) + (lo >> 4))`
///    using wrapping 64-bit arithmetic.
/// 4. Keep the low 48 bits.
///
/// Step 3 is a `boost::hash_combine`-style mix in which Valhalla hashes `hi` with `std::hash`,
/// which is the identity function for integers in the standard libraries Valhalla supports.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct TileHash(u64);

impl TileHash {
    /// Computes the hash of a tile's data.
    ///
    /// The input must be every byte of the tile *after* the header.
    /// For a tile that is produced in several chunks, use [`TileHasher`].
    pub fn of_tile_data(data: &[u8]) -> Self {
        let mut hasher = TileHasher::new();
        hasher.update(data);
        hasher.finish()
    }

    /// Extracts the tile hash from a raw header checksum.
    #[inline]
    pub(crate) const fn from_checksum(checksum: u64) -> Self {
        Self(checksum & TILE_HASH_MASK)
    }

    /// The hash as an integer (only the low 48 bits may be set).
    #[inline]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Incrementally computes a [`TileHash`] over tile data produced in chunks.
#[derive(Clone, Default)]
pub struct TileHasher(Md5);

impl TileHasher {
    /// Creates a hasher with no data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next chunk of tile data (after the header) into the hasher.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    /// Finishes hashing and returns the tile hash.
    pub fn finish(self) -> TileHash {
        let digest: [u8; 16] = self.0.finalize().into();
        let mut lo = [0u8; 8];
        let mut hi = [0u8; 8];
        lo.copy_from_slice(&digest[..8]);
        hi.copy_from_slice(&digest[8..]);
        TileHash(fold_md5(u64::from_be_bytes(lo), u64::from_be_bytes(hi)) & TILE_HASH_MASK)
    }
}

/// Folds the two halves of an MD5 digest into 64 bits, exactly like Valhalla's `fold_md5`.
#[inline]
const fn fold_md5(lo: u64, hi: u64) -> u64 {
    lo ^ hi
        .wrapping_add(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(lo << 12)
        .wrapping_add(lo >> 4)
}

/// Identifies a tileset build.
///
/// Every tile in a tileset carries the same build ID,
/// which is derived from the [`TileHash`] of every tile in the set
/// (see [`BuildId::of_tileset`]).
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct BuildId(u16);

impl BuildId {
    /// Creates a build ID from its raw value.
    #[inline]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Computes the build ID of a tileset from the hashes of all of its tiles.
    ///
    /// This matches Valhalla's `compute_tileset_build_id`:
    /// the hashes are summed (with wrapping 64-bit arithmetic, so order does not matter),
    /// and the sum is folded into 16 bits by combining its four 16-bit words with XOR.
    ///
    /// The tileset includes every graph tile at every level (including transit tiles).
    /// Each tile must be counted exactly once.
    pub fn of_tileset<I: IntoIterator<Item = TileHash>>(tile_hashes: I) -> Self {
        let sum = tile_hashes
            .into_iter()
            .fold(0u64, |acc, hash| acc.wrapping_add(hash.value()));
        let folded = sum ^ (sum >> 16) ^ (sum >> 32) ^ (sum >> 48);
        // Valhalla returns this as a uint16_t, which keeps the low 16 bits.
        Self(low_u16(folded))
    }

    /// Extracts the build ID from a raw header checksum.
    #[inline]
    pub(crate) const fn from_checksum(checksum: u64) -> Self {
        // The shift leaves exactly 16 bits.
        Self(low_u16(checksum >> TILE_HASH_BITS))
    }

    /// The build ID as an integer.
    #[inline]
    pub const fn value(self) -> u16 {
        self.0
    }
}

/// The low 16 bits of a 64-bit integer.
#[inline]
const fn low_u16(value: u64) -> u16 {
    let bytes = value.to_le_bytes();
    u16::from_le_bytes([bytes[0], bytes[1]])
}

/// Packs a build ID and a tile hash into a raw header checksum.
#[inline]
pub(crate) const fn checksum(build_id: BuildId, tile_hash: TileHash) -> u64 {
    ((build_id.0 as u64) << TILE_HASH_BITS) | tile_hash.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn tile_hash_of_empty_data() {
        // MD5("") = d41d8cd98f00b204 e9800998ecf8427e.
        // Folded by hand following the doc comment on TileHash:
        // lo << 12 = 0xd8cd98f00b204000, lo >> 4 = 0x0d41d8cd98f00b20,
        // hi + 0x9e3779b97f4a7c15 + (lo << 12) + (lo >> 4) = 0x6dc6f510105309b3 (mod 2^64),
        // lo ^ that = 0xb9db79c99f53bbb7, and the low 48 bits are 0x79c99f53bbb7.
        assert_eq!(TileHash::of_tile_data(&[]).value(), 0x79c9_9f53_bbb7);
    }

    #[test]
    fn tile_hasher_chunking_does_not_matter() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let mut hasher = TileHasher::new();
        for chunk in data.chunks(7) {
            hasher.update(chunk);
        }
        assert_eq!(hasher.finish(), TileHash::of_tile_data(&data));
    }

    #[test]
    fn build_id_of_empty_tileset() {
        assert_eq!(BuildId::of_tileset([]), BuildId::new(0));
    }

    #[test]
    fn build_id_folds_all_words() {
        // Sum = 0x0001_0002_0003_0004 + 0x0000_0000_0000_0001 = 0x0001_0002_0003_0005.
        // Folding XORs the words: 0x0005 ^ 0x0003 ^ 0x0002 ^ 0x0001 = 0x0005.
        let hashes = [TileHash(0x0001_0002_0003_0004), TileHash(1)];
        assert_eq!(BuildId::of_tileset(hashes), BuildId::new(0x0005));
    }

    #[test]
    fn build_id_sum_wraps() {
        // Real tile hashes are only 48 bits, so the sum only wraps for absurdly large tilesets,
        // but the arithmetic must still match Valhalla's uint64_t accumulator.
        let hashes = [TileHash(u64::MAX), TileHash(2)];
        // Sum = 1 (mod 2^64), which folds to 1.
        assert_eq!(BuildId::of_tileset(hashes), BuildId::new(1));
    }

    proptest! {
        #[test]
        fn checksum_round_trip(build_id: u16, hash in 0u64..=TILE_HASH_MASK) {
            let raw = checksum(BuildId::new(build_id), TileHash(hash));
            prop_assert_eq!(BuildId::from_checksum(raw), BuildId::new(build_id));
            prop_assert_eq!(TileHash::from_checksum(raw), TileHash(hash));
        }

        #[test]
        fn build_id_is_order_independent(mut hashes in prop::collection::vec(0u64..=TILE_HASH_MASK, 0..50)) {
            let forward = BuildId::of_tileset(hashes.iter().copied().map(TileHash));
            hashes.reverse();
            let reversed = BuildId::of_tileset(hashes.iter().copied().map(TileHash));
            prop_assert_eq!(forward, reversed);
        }
    }
}
