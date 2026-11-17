/// Offline map regions: planning a region's byte ranges of the published PMTiles archive,
/// and unpacking downloaded ranges into the device's MBTiles, which the map reads (see the
/// `offline_map` crate). The download engine in the app uses it, and the offline geocoder
/// reads a region's tiles with [basemapReadRegionTiles] (the basemap must be read through
/// this library's SQLite, which writes it).
///
/// It is its own library, apart from `sparkle.dart`, so that its [LngLat] doesn't clash
/// with the app's.
library;

export 'src/rust/api/map.dart' show LngLat;
export 'src/rust/api/offline.dart'
    show
        BasemapTile,
        BasemapTileAfter,
        BasemapTileKey,
        ByteRange,
        MapChunkPlan,
        MapError,
        MapErrorKind,
        MapPlanner,
        MapRegionPlan,
        UnpackedChunk,
        basemapClaim,
        basemapPrepare,
        basemapReadRegionTiles,
        basemapReadTiles,
        basemapRegionTiles,
        basemapRegionZoomTiles,
        basemapRelease,
        basemapUnpack,
        mapFirstFetch;
export 'src/sparkle_map.dart' show initSparkle;
