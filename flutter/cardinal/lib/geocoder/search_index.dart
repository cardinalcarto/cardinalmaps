/// Offline search's index on the device: one index of the pinned map release, built whole
/// from the ready regions' map tiles and their Who's On First areas whenever the regions
/// change (see the offline geocoder's decisions log, "Device index lifecycle").
///
/// Laid out under `<offline folder>/search/`:
///
/// - `wof/<region>/<n>.json`: the WOF service's responses for the region's bounding box,
///   fetched once when the region is ready, and `wof/<region>/done` once they all are, so
///   rebuilds work offline;
/// - `index-<n>/`: an index, built into a new directory each time;
/// - `current.json`: the index search uses and what it was built from. It's replaced
///   (atomically) once a new index is built, so the old one stays searchable meanwhile.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:flutter/foundation.dart';
import 'package:geocoder/geocoder.dart';
import 'package:http/http.dart' as http;
import 'package:sparkle/offline.dart' as sparkle;

import '../offline/downloads/engine.dart';

/// The index search uses, and the basemap its pointers point into.
@immutable
class SearchIndexFiles {
  const SearchIndexFiles({required this.dir, required this.basemap});

  final String dir;
  final String basemap;

  @override
  bool operator ==(Object other) =>
      other is SearchIndexFiles && other.dir == dir && other.basemap == basemap;

  @override
  int get hashCode => Object.hash(dir, basemap);
}

/// What building an index takes: where, from which WOF responses, and the basemap's
/// regions to read the tiles of.
typedef IndexBuildJob = ({
  String dir,
  List<String> wofFiles,
  String basemap,
  List<int> regions,
});

/// Builds an index; returns a summary for logs.
typedef IndexBuildRunner = Future<String> Function(IndexBuildJob job);

/// The longest side, in degrees, of a box of WOF places fetched at once (the service
/// caps a box at 100 square degrees and is slow on big ones).
const wofBoxDegrees = 3.0;

/// The device's search index, kept in step with the ready regions.
class SearchIndex implements RegionIndexer {
  SearchIndex({
    required this.root,
    required this.wofUrl,
    http.Client? client,
    IndexBuildRunner? build,
    List<WofBox> Function(GeoRectBox box)? boxes,
  }) : _client = client ?? http.Client(),
       _build = build ?? buildInIsolate,
       _boxes = boxes ?? _wofBoxes;

  /// `<offline folder>/search`.
  final String root;

  /// The WOF service; empty for none, and the index has no areas.
  final String wofUrl;
  final http.Client _client;
  final IndexBuildRunner _build;
  final List<WofBox> Function(GeoRectBox box) _boxes;

  /// The index to search, or null while there's none.
  final current = ValueNotifier<SearchIndexFiles?>(null);

  /// Whether an index is being built (search uses the old one meanwhile).
  final building = ValueNotifier<bool>(false);

  File get _currentFile => File('$root/current.json');

  /// Reads [current] from disk; call once before searching.
  Future<void> load() async {
    final manifest = await _readManifest();
    current.value = manifest == null ? null : _files(manifest);
  }

  IndexSources? _wanted;
  Future<void>? _running;

  /// Builds the index again if [sources] (or their areas) changed since the last one.
  /// Returns at once; [settled] completes when it's done.
  @override
  void regionsChanged(IndexSources sources) {
    _wanted = sources;
    _running ??= _loop().whenComplete(() => _running = null);
  }

  /// Completes once nothing is left to build.
  Future<void> get settled async {
    for (var running = _running; running != null; running = _running) {
      await running;
    }
  }

  Future<void> _loop() async {
    for (var sources = _wanted; sources != null; sources = _wanted) {
      _wanted = null;
      try {
        await _sync(sources);
      } on Object catch (e, stack) {
        debugPrint('search index: $e\n$stack');
      }
    }
  }

  String _wofDir(int region) => '$root/wof/$region';

  Future<void> _sync(IndexSources sources) async {
    await Directory(root).create(recursive: true);
    // Areas for regions that have none yet; a failure leaves it for the next time.
    for (final region in sources.regions) {
      if (!await File('${_wofDir(region.id)}/done').exists()) {
        await _fetchWof(region);
      }
    }
    final wanted = <String, Object?>{
      'map': sources.mapRelease,
      'basemap': sources.basemap,
      'regions': [
        for (final r in sources.regions)
          {'id': r.id, 'wof': await File('${_wofDir(r.id)}/done').exists()},
      ],
    };
    final manifest = await _readManifest();
    if (manifest != null &&
        jsonEncode(manifest['sources']) == jsonEncode(wanted) &&
        await Directory(manifest['dir'] as String).exists()) {
      await _cleanUp(sources, keep: manifest['dir'] as String);
      return;
    }
    if (sources.regions.isEmpty || sources.basemap == null) {
      await _deleteQuietly(_currentFile);
      current.value = null;
      await _cleanUp(sources, keep: null);
      debugPrint('search index: no regions, no index');
      return;
    }
    final dir = '$root/index-${DateTime.now().microsecondsSinceEpoch}';
    final wofFiles = <String>[];
    for (final r in sources.regions) {
      final wof = Directory(_wofDir(r.id));
      if (!await wof.exists()) continue;
      await for (final f in wof.list()) {
        if (f is File && f.path.endsWith('.json')) wofFiles.add(f.path);
      }
    }
    wofFiles.sort();
    building.value = true;
    final watch = Stopwatch()..start();
    try {
      final summary = await _build((
        dir: dir,
        wofFiles: wofFiles,
        basemap: sources.basemap!,
        regions: [for (final r in sources.regions) r.id],
      ));
      debugPrint(
        'cardinal-timing: search index ${watch.elapsedMilliseconds} ms: $summary',
      );
    } on Object {
      await _deleteDir(dir);
      rethrow;
    } finally {
      building.value = false;
    }
    final next = {'dir': dir, 'sources': wanted};
    final part = File('${_currentFile.path}.part');
    await part.writeAsString(jsonEncode(next), flush: true);
    await part.rename(_currentFile.path);
    current.value = _files(next);
    await _cleanUp(sources, keep: dir);
  }

  /// Fetches the WOF places of [region]'s bounding box, in boxes, into its folder.
  Future<void> _fetchWof(IndexRegion region) async {
    if (wofUrl.isEmpty) return;
    final dir = Directory(_wofDir(region.id));
    await _deleteDir(dir.path);
    await dir.create(recursive: true);
    final b = region.bounds;
    final boxes = _boxes((
      west: b.west,
      south: b.south,
      east: b.east,
      north: b.north,
    ));
    final base = Uri.parse(wofUrl);
    final path = base.path.endsWith('/')
        ? base.path.substring(0, base.path.length - 1)
        : base.path;
    try {
      for (final (i, box) in boxes.indexed) {
        final uri = base.replace(
          path: '$path/v1/places',
          queryParameters: {
            'bbox': '${box.west},${box.south},${box.east},${box.north}',
          },
        );
        final response = await _client.get(uri);
        if (response.statusCode != 200) {
          throw HttpException('${response.statusCode} from $uri');
        }
        await File('${dir.path}/$i.json').writeAsBytes(response.bodyBytes);
      }
      await File('${dir.path}/done').writeAsString('${boxes.length}');
      debugPrint(
        'search index: areas of region ${region.id} in ${boxes.length} boxes',
      );
    } on Object catch (e) {
      // Indexed without areas for now; tried again when the regions next change or the
      // app starts.
      debugPrint('search index: no areas for region ${region.id} yet: $e');
      await _deleteDir(dir.path);
    }
  }

  /// Deletes indexes other than [keep] and the areas of regions that are gone.
  Future<void> _cleanUp(IndexSources sources, {required String? keep}) async {
    final regions = {for (final r in sources.regions) '${r.id}'};
    final rootDir = Directory(root);
    if (!await rootDir.exists()) return;
    await for (final entry in rootDir.list()) {
      final name = entry.uri.pathSegments.where((s) => s.isNotEmpty).last;
      if (entry is Directory &&
          name.startsWith('index-') &&
          entry.path != keep) {
        await _deleteDir(entry.path);
      }
    }
    final wof = Directory('$root/wof');
    if (await wof.exists()) {
      await for (final entry in wof.list()) {
        final name = entry.uri.pathSegments.where((s) => s.isNotEmpty).last;
        if (entry is Directory && !regions.contains(name)) {
          await _deleteDir(entry.path);
        }
      }
    }
  }

  Future<Map<String, Object?>?> _readManifest() async {
    try {
      return jsonDecode(await _currentFile.readAsString())
          as Map<String, Object?>;
    } on Object {
      return null;
    }
  }

  static SearchIndexFiles? _files(Map<String, Object?> manifest) {
    final sources = manifest['sources'] as Map<String, Object?>?;
    final basemap = sources?['basemap'] as String?;
    final dir = manifest['dir'] as String?;
    if (dir == null || basemap == null) return null;
    return SearchIndexFiles(dir: dir, basemap: basemap);
  }

  static Future<void> _deleteDir(String path) async {
    try {
      await Directory(path).delete(recursive: true);
    } on FileSystemException {
      // Not there.
    }
  }

  static Future<void> _deleteQuietly(File file) async {
    try {
      await file.delete();
    } on FileSystemException {
      // Not there.
    }
  }

  void close() => _client.close();
}

/// A bounding box, in degrees.
typedef GeoRectBox = ({double west, double south, double east, double north});

List<WofBox> _wofBoxes(GeoRectBox b) => wofBoxes(
  west: b.west,
  south: b.south,
  east: b.east,
  north: b.north,
  side: wofBoxDegrees,
);

/// Tantivy's indexing memory for a build, in MB, and its threads.
const _heapMb = 64;
const _threads = 2;

/// Tiles read from the basemap at once.
const _page = 256;

/// Builds an index in a background isolate, so the UI isolate isn't busy with the tiles'
/// bytes: they're read through sparkle's SQLite and handed to the geocoder there.
Future<String> buildInIsolate(IndexBuildJob job) =>
    Isolate.run(() => buildIndex(job), debugName: 'search index');

/// Builds an index on this isolate.
Future<String> buildIndex(IndexBuildJob job) async {
  await sparkle.initSparkle();
  await initGeocoder();
  final builder = await IndexBuilder.create(
    dir: job.dir,
    wofFiles: job.wofFiles,
    heapMb: _heapMb,
    threads: _threads,
  );
  final pages = tilesInOrder<sparkle.BasemapTile>(
    [
      for (final region in job.regions)
        (sparkle.BasemapTile? after) => sparkle.basemapReadRegionTiles(
          path: job.basemap,
          region: region,
          zoom: 14,
          after: after == null
              ? null
              : sparkle.BasemapTileAfter(x: after.x, y: after.y),
          limit: _page,
        ),
    ],
    x: (t) => t.x,
    y: (t) => t.y,
    page: _page,
  );
  await for (final page in pages) {
    await builder.addTiles(
      tiles: [
        for (final t in page)
          GeocoderTile(z: t.z, x: t.x, y: t.y, data: t.data),
      ],
    );
  }
  final s = await builder.finish();
  return '${s.tiles} tiles (${s.repeatedTiles} repeated), ${s.features} features, '
      '${s.pois} POIs (${s.pointers} pointers), ${s.addresses} addresses, '
      '${s.streets} streets, ${s.admins} areas, ${s.postcodes} postcodes; '
      '${s.wofPlaces} WOF places';
}

/// The tiles of several regions, each read page by page in order of column then row
/// ([read] gives the page after a tile, or the first after null), as one sequence in
/// that order, in pages of at most [page]. The index build needs that order to finish
/// each street once the tiles have passed it, rather than hold them all. A tile two
/// regions share comes from each, one after the other (the build skips the second).
Stream<List<T>> tilesInOrder<T>(
  List<Future<List<T>> Function(T? after)> read, {
  required int Function(T) x,
  required int Function(T) y,
  required int page,
}) async* {
  final buffers = [for (final _ in read) <T>[]];
  final next = [for (final _ in read) 0];
  final done = [for (final _ in read) false];
  // Whether region [i] has a tile to give, reading its next page if it needs to.
  Future<bool> ready(int i) async {
    if (next[i] < buffers[i].length) return true;
    if (done[i]) return false;
    final more = await read[i](buffers[i].lastOrNull);
    if (more.isEmpty) {
      done[i] = true;
      return false;
    }
    buffers[i] = more;
    next[i] = 0;
    return true;
  }

  bool before(T a, T b) => x(a) < x(b) || (x(a) == x(b) && y(a) < y(b));
  var out = <T>[];
  while (true) {
    int? best;
    for (var i = 0; i < read.length; i++) {
      if (!await ready(i)) continue;
      if (best == null ||
          before(buffers[i][next[i]], buffers[best][next[best]])) {
        best = i;
      }
    }
    if (best == null) break;
    out.add(buffers[best][next[best]++]);
    if (out.length >= page) {
      yield out;
      out = <T>[];
    }
  }
  if (out.isNotEmpty) yield out;
}
