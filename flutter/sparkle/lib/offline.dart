/// Offline map regions: planning a region's byte ranges of the published PMTiles archive,
/// and unpacking downloaded ranges into the device's MBTiles, which the map reads (see the
/// `offline_map` crate). The download engine in the app is the only user.
///
/// It is its own library, apart from `sparkle.dart`, so that its [LngLat] doesn't clash
/// with the app's.
library;

export 'src/rust/api/map.dart' show LngLat;
export 'src/rust/api/offline.dart'
    show
        ByteRange,
        MapChunkPlan,
        MapError,
        MapErrorKind,
        MapPlanner,
        MapRegionPlan,
        UnpackedChunk,
        basemapClaim,
        basemapPrepare,
        basemapRegionTiles,
        basemapRelease,
        basemapUnpack,
        mapFirstFetch;
export 'src/sparkle_map.dart' show initSparkle;
