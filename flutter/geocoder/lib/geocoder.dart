/// The offline geocoder on the device: builds the phone's search index from its z14 map
/// tiles and the Who's On First places fetched for its regions ([IndexBuilder]), and
/// searches it ([OfflineIndex]).
///
/// The tiles come from the basemap through sparkle's `basemapReadRegionTiles`, page by page
/// (the basemap is read through the SQLite that writes it, which this library doesn't have),
/// and go to [IndexBuilder.addTiles] as [GeocoderTile]s. Search results are the geocoder
/// server's JSON; a result that's a pointer to its tile feature (`pointer`) is read from the
/// tile with [poiFeature]. [RegionExtraction] only counts what a region's tiles hold.
library;

import 'src/rust/frb_generated.dart';

export 'src/rust/api/extract.dart'
    show
        ExtractError,
        ExtractErrorKind,
        ExtractionStats,
        GeocoderTile,
        LayerStats,
        RegionExtraction;
export 'src/rust/api/index.dart'
    show
        IndexBuildStats,
        IndexBuilder,
        IndexError,
        IndexErrorKind,
        OfflineIndex,
        OfflineQuery,
        PoiFeature,
        WofBox,
        poiFeature,
        wofBoxes;

/// Loads the native library. Call once (it is safe to call again) before using it, in
/// each isolate that does.
Future<void> initGeocoder() => _init ??= RustLib.init();
Future<void>? _init;
