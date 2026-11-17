//! The offline geocoder on the device, reached from Dart through flutter_rust_bridge
//! ([`api`]).
//!
//! The phone builds its search index from the map tiles it downloaded and the Who's On First
//! places fetched for its regions: [`extract`] reads a z14 MLT tile's features (POIs,
//! addresses, streets, named areas and labels) into [`extract::ExtractedFeature`]s, [`merge`]
//! joins the pieces of streets and areas that tiles cut, [`region`] runs tiles through both
//! into a [`region::FeatureSink`], [`documents`] makes airmail documents of the features with
//! their areas from [`wof`], and [`build`] writes them into an index. [`search`] searches it.
//!
//! This library has no SQLite: the basemap is read through sparkle_flutter's, which writes it
//! (two copies of SQLite in one process break each other's locks; see `offline_map::mbtiles`),
//! and its tiles come in from Dart.

#[cfg(target_os = "android")]
pub(crate) mod allocator {
    //! On Android, bionic's allocator (jemalloc without thread caches) took most of an
    //! index build's time, locking for every allocation of the document threads: mimalloc
    //! for this library's own allocations (the rest of the app keeps the platform's).

    #[global_allocator]
    static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

    unsafe extern "C" {
        fn mi_option_set(option: i32, value: std::ffi::c_long);
    }

    /// `mi_option_purge_delay` in mimalloc 3's `mimalloc.h` (the version is pinned).
    const PURGE_DELAY: i32 = 15;

    /// Gives freed memory back after 10 ms, not mimalloc 3's second: a build frees as it
    /// goes, and its peak RSS on the phone was ~35 MB lower for it, as fast.
    pub fn tune() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        // SAFETY: setting an option mimalloc reads as it purges.
        ONCE.call_once(|| unsafe { mi_option_set(PURGE_DELAY, 10) });
    }
}

pub mod api;
pub mod build;
pub mod documents;
pub mod extract;
mod frb_generated;
pub mod merge;
pub mod pool;
pub mod region;
pub mod search;
pub mod wof;
