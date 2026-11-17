import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';

import 'package:geocoder/geocoder.dart' as rust;
import 'package:http/http.dart' as http;
import 'package:sparkle/offline.dart' as sparkle;
import 'package:sparkle/sparkle.dart' show ClickedFeature;

import '../map/poi.dart';
import '../offline/downloads/engine.dart';
import '../store/store.dart';
import 'geocoder.dart';
import 'search_index.dart';

/// The geocoder's models, as paths; null for one that isn't downloaded.
typedef GeocoderModelFiles = ({String? tagger, String? categories});

/// A tile and the stable POI ID of a feature in it.
typedef PoiInTile = ({int z, int x, int y, int poiId});

/// The device's index, open for searching.
abstract interface class OfflineSearcher {
  /// The server's `{"results": [...]}` JSON.
  Future<String> search(rust.OfflineQuery query);

  /// The server's `result` JSON, or null.
  Future<String?> place(String id, String? lang);

  void dispose();
}

/// The native side of offline search, apart for tests.
abstract interface class OfflineSearchNative {
  Future<OfflineSearcher> open(String dir, GeocoderModelFiles models);

  /// The properties of each feature, from the basemap's tiles; null for one that isn't
  /// there.
  Future<List<Map<String, String>?>> features(
    String basemap,
    List<PoiInTile> wanted,
  );
}

/// Offline search through `geocoder_flutter` and sparkle's basemap reads.
class RustOfflineSearch implements OfflineSearchNative {
  const RustOfflineSearch();

  @override
  Future<OfflineSearcher> open(String dir, GeocoderModelFiles models) async {
    await rust.initGeocoder();
    return _RustSearcher(
      await rust.OfflineIndex.open(
        dir: dir,
        tagger: models.tagger,
        categories: models.categories,
      ),
    );
  }

  @override
  Future<List<Map<String, String>?>> features(
    String basemap,
    List<PoiInTile> wanted,
  ) async {
    await sparkle.initSparkle();
    await rust.initGeocoder();
    final keys = {for (final w in wanted) (w.z, w.x, w.y)}.toList();
    final tiles = await sparkle.basemapReadTiles(
      path: basemap,
      keys: [
        for (final (z, x, y) in keys) sparkle.BasemapTileKey(z: z, x: x, y: y),
      ],
    );
    final byKey = {for (final t in tiles) (t.z, t.x, t.y): t};
    return [
      for (final w in wanted)
        if (byKey[(w.z, w.x, w.y)] case final tile?)
          (await rust.poiFeature(
            tile: rust.GeocoderTile(
              z: tile.z,
              x: tile.x,
              y: tile.y,
              data: tile.data,
            ),
            poiId: BigInt.from(w.poiId),
          ))?.properties
        else
          null,
    ];
  }
}

class _RustSearcher implements OfflineSearcher {
  _RustSearcher(this.index);

  final rust.OfflineIndex index;

  @override
  Future<String> search(rust.OfflineQuery query) => index.search(query: query);

  @override
  Future<String?> place(String id, String? lang) =>
      index.place(id: id, lang: lang);

  @override
  void dispose() => index.dispose();
}

/// The geocoder on the device, over the index built from the offline areas' map tiles
/// ([SearchIndex]), with the query tagger and categorical model once they're downloaded
/// (it works without them).
///
/// Results come back as the server's are, and those that point to their tile feature (a
/// POI with a stable ID stores only its names) are filled in from the basemap's tiles, as
/// a tapped map POI is ([placeFromPoi]).
class OfflineGeocoder implements Geocoder, OfflineListener {
  OfflineGeocoder({
    required this.index,
    required this.models,
    this.native = const RustOfflineSearch(),
  });

  /// No index: finds nothing, as on a device with no offline area.
  OfflineGeocoder.none()
    : index = Future.value(),
      models = (() async => (tagger: null, categories: null)),
      native = const RustOfflineSearch();

  /// The device's index, or null in a build without downloads.
  final Future<SearchIndex?> index;

  /// The models downloaded, read again after downloads change.
  final Future<GeocoderModelFiles> Function() models;
  final OfflineSearchNative native;

  ({
    SearchIndexFiles files,
    GeocoderModelFiles models,
    OfflineSearcher searcher,
  })?
  _open;
  Future<OfflineSearcher?>? _opening;
  GeocoderModelFiles? _models;

  /// Results hydrated from tiles, at most: a search shows a screenful.
  static const _defaultLimit = 10;

  @override
  Future<void> offlineChanged({
    required bool feedsChanged,
    bool filesRemoved = false,
  }) async {
    // The models may have arrived.
    _models = null;
  }

  Future<OfflineSearcher?> _searcher() async {
    final files = (await index)?.current.value;
    if (files == null) {
      _close();
      return null;
    }
    final models = _models ??= await this.models();
    final open = _open;
    if (open != null && open.files == files && open.models == models) {
      return open.searcher;
    }
    return _opening ??= () async {
      try {
        final searcher = await native.open(files.dir, models);
        _close();
        _open = (files: files, models: models, searcher: searcher);
        debugPrint(
          'offline search: opened ${files.dir} (tagger ${models.tagger != null}, '
          'categories ${models.categories})',
        );
        return searcher;
      } finally {
        _opening = null;
      }
    }();
  }

  void _close() {
    _open?.searcher.dispose();
    _open = null;
  }

  @override
  Future<List<Place>> search(
    String query, {
    LngLat? focus,
    Viewport? viewport,
    String? lang,
    bool prefix = false,
    bool typos = false,
    int? limit,
    Future<void>? abort,
  }) async {
    var aborted = false;
    unawaited(abort?.then((_) => aborted = true));
    final watch = Stopwatch()..start();
    final searcher = await _searcher();
    if (searcher == null) return const [];
    final opened = watch.elapsedMicroseconds;
    final basemap = _open!.files.basemap;
    final json = await searcher.search(
      rust.OfflineQuery(
        text: query,
        lang: lang,
        prefix: prefix,
        typos: typos,
        focus: focus == null
            ? null
            : Float64List.fromList([focus.lon, focus.lat]),
        viewport: viewport == null
            ? null
            : Float64List.fromList([
                viewport.west,
                viewport.south,
                viewport.east,
                viewport.north,
              ]),
        limit: limit ?? _defaultLimit,
      ),
    );
    if (aborted) throw http.RequestAbortedException();
    final results = [
      for (final r
          in (jsonDecode(json) as Map<String, Object?>)['results']
              as List<Object?>)
        r as Map<String, Object?>,
    ];
    final searched = watch.elapsedMicroseconds;
    final places = await _hydrate(results, basemap, lang);
    debugPrint(
      'cardinal-timing: offline search "$query" ${results.length} results: '
      'open ${opened ~/ 1000} ms, search ${(searched - opened) / 1000} ms, '
      'hydrate ${(watch.elapsedMicroseconds - searched) / 1000} ms',
    );
    if (aborted) throw http.RequestAbortedException();
    return places;
  }

  @override
  Future<Place?> place(String id, {String? lang, Future<void>? abort}) async {
    final searcher = await _searcher();
    if (searcher == null) return null;
    final json = await searcher.place(id, lang);
    if (json == null) return null;
    final places = await _hydrate(
      [jsonDecode(json) as Map<String, Object?>],
      _open!.files.basemap,
      lang,
    );
    return places.single;
  }

  /// The results as places, pointers filled in from their tiles.
  Future<List<Place>> _hydrate(
    List<Map<String, Object?>> results,
    String basemap,
    String? lang,
  ) async {
    final wanted = <PoiInTile>[];
    for (final r in results) {
      if (_pointer(r) case final p?) wanted.add(p);
    }
    final features = wanted.isEmpty
        ? const <Map<String, String>?>[]
        : await native.features(basemap, wanted);
    var next = 0;
    return [
      for (final r in results)
        if (_pointer(r) case final p?)
          _pointerPlace(r, p, features[next++], lang)
        else
          placeFromResult(r),
    ];
  }

  static PoiInTile? _pointer(Map<String, Object?> result) {
    final pointer = result['pointer'] as Map<String, Object?>?;
    if (pointer == null) return null;
    final tile = pointer['tile'] as List<Object?>;
    return (
      z: tile[0] as int,
      x: tile[1] as int,
      y: tile[2] as int,
      poiId: pointer['poi_id'] as int,
    );
  }

  /// A pointer result's place: its names, center and areas from the index, and the rest
  /// (address, tags, version) from its tile feature, if the tile is still there.
  static Place _pointerPlace(
    Map<String, Object?> result,
    PoiInTile pointer,
    Map<String, String>? properties,
    String? lang,
  ) {
    final base = placeFromResult(result);
    if (properties == null) return base;
    final tile = placeFromPoi(
      ClickedFeature(
        source: 'openmaptiles',
        sourceLayer: poiLayer,
        tileZ: pointer.z,
        tileX: pointer.x,
        tileY: pointer.y,
        id: BigInt.from(pointer.poiId),
        lng: base.center.lon,
        lat: base.center.lat,
        properties: properties,
      ),
      lang: lang,
    );
    return Place(
      id: base.id,
      kind: base.kind,
      version: tile.version,
      name: tile.name ?? base.name,
      countryCode: base.countryCode ?? tile.countryCode,
      center: base.center,
      bbox: base.bbox,
      shape: base.shape,
      address: tile.address,
      admins: base.admins,
      tags: tile.tags,
    );
  }
}
