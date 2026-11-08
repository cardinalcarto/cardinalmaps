use std::borrow::Cow;
use zerocopy::{LE, U32};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, Unaligned};

/// An administrative area (country and principal subdivision) referenced by nodes in the tile
/// (mainline's `Admin`).
///
/// The names are in the text list;
/// use [`GraphTile::get_admin_info`](super::GraphTile::get_admin_info) to resolve them.
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, Debug, Clone)]
#[repr(C)]
pub struct Admin {
    /// The offset into the graph tile text list for the country name.
    pub country_name_offset: U32<LE>,
    /// The offset into the graph tile text list for the principal subdivision name.
    pub principal_subdivision_offset: U32<LE>,
    country_iso: [u8; 2],
    principal_subdivision_iso: [u8; 3],
    // padding for alignment
    _spare: [u8; 3],
}

impl Admin {
    /// Gets the ISO 3166-1 country code (empty if unknown).
    ///
    /// As in mainline, the code is empty if its first byte is NUL,
    /// and otherwise both bytes.
    #[inline]
    pub fn country_iso(&self) -> Cow<'_, str> {
        if self.country_iso[0] == 0 {
            Cow::Borrowed("")
        } else {
            String::from_utf8_lossy(&self.country_iso)
        }
    }

    /// Gets the principal subdivision part of the ISO 3166-2 code
    /// (for example, `PA`; empty if unknown).
    #[inline]
    pub fn principal_subdivision_iso(&self) -> Cow<'_, str> {
        let end = self
            .principal_subdivision_iso
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.principal_subdivision_iso.len());
        String::from_utf8_lossy(&self.principal_subdivision_iso[..end])
    }
}

#[cfg(test)]
mod tests {
    use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_L0};

    #[test]
    fn test_parse_admin_count() {
        let tile = &*TEST_GRAPH_TILE_L0;

        assert_eq!(tile.admins().len(), tile.header().admin_count() as usize);
    }

    #[test]
    fn test_parse_admins() {
        let tile = &*TEST_GRAPH_TILE_L0;

        let country_isos = tile
            .admins()
            .iter()
            .map(|admin| admin.country_iso())
            .collect::<Vec<_>>();
        let principal_subdivision_isos = tile
            .admins()
            .iter()
            .map(|admin| admin.principal_subdivision_iso())
            .collect::<Vec<_>>();

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!("raw_admin_snapshot", tile.admins());
            insta::assert_debug_snapshot!("country_iso", country_isos);
            insta::assert_debug_snapshot!("principal_subdivision_iso", principal_subdivision_isos);
        }
    }

    #[test]
    fn test_get_admin_info() {
        // Cross-checked with GraphTile::admininfo in Valhalla 3.9.1.
        let tile = &*TEST_GRAPH_TILE_L0;
        let none = tile.get_admin_info(0).unwrap();
        assert_eq!(none.country_text, "None");
        assert_eq!(none.state_text, "None");
        assert_eq!(none.country_iso, "");
        let andorra = tile.get_admin_info(1).unwrap();
        assert_eq!(andorra.country_text, "Andorra");
        assert_eq!(andorra.state_text, "");
        assert_eq!(andorra.country_iso, "AD");
        assert_eq!(andorra.state_iso, "");
        assert!(tile.get_admin_info(2).is_err());
    }
}
