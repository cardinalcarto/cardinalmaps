/// The offline geocoder on the device: reads a region's z14 map tiles into the features the
/// phone's search index is built from ([RegionExtraction]).
///
/// The tiles come from the basemap through sparkle's `basemapReadRegionTiles`, page by page
/// (the basemap is read through the SQLite that writes it, which this library doesn't have),
/// and go to [RegionExtraction.addTiles] as [GeocoderTile]s; then
/// [RegionExtraction.finish] joins streets and areas across tiles. For now it only counts
/// what it would index ([ExtractionStats]).
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

/// Loads the native library. Call once (it is safe to call again) before using it.
Future<void> initGeocoder() => _init ??= RustLib.init();
Future<void>? _init;
