import 'dart:typed_data';

import 'package:routing/routing.dart' as routing;
import 'package:sparkle/offline.dart' as sparkle;

import 'geometry.dart';

/// One byte range of the map archive to download, with its tiles (opaque).
typedef MapRange = ({int start, int length, int tileCount, Uint8List tiles});

/// What a region's map needs, from [MapPlanning.finish].
typedef MapPlanResult = ({
  int coveringTiles,
  int archiveTiles,
  int presentTiles,
  int tileBytes,
  int rangeBytes,
  List<MapRange> ranges,
  Uint8List tileKeys,
});

/// Why a map range can't be unpacked.
enum MapRangeProblem {
  /// Shorter or longer than asked for.
  length,

  /// A tile isn't a whole gzip stream.
  badTile,

  /// The basemap or the range's file can't be read or written.
  storage,
}

class MapRangeError implements Exception {
  const MapRangeError(this.problem, this.message);

  final MapRangeProblem problem;
  final String message;

  @override
  String toString() => 'MapRangeError(${problem.name}: $message)';
}

/// A tile of an archive, as `(z, x, y)`.
typedef TileKey = (int, int, int);

/// A tile an archive has, and where: `start` from the archive's start.
typedef ArchiveTileAt = ({TileKey tile, int start, int length});

/// A range of an archive to download, with its tiles (`start` from the range's start).
typedef ArchiveRangePlan = ({int start, int length, List<ArchiveTileAt> tiles});

/// A transfer tile in a downloaded range, and the file to write it to.
typedef RangeTransferItem = ({
  int start,
  int length,
  String dest,
  String mode,
  int x,
  int y,
});

/// Finding a list of tiles in an archive (a transfer archive), given its first bytes.
abstract class ArchivePlanning {
  /// Leaf directories to fetch next, as `(start, length)`; empty when done.
  List<(int, int)> neededLeaves();

  Future<void> addLeaf(int start, Uint8List bytes);

  /// Looks for [tiles] instead, keeping the leaves fetched so far.
  void want(List<TileKey> tiles);

  /// The one zoom the archive holds.
  int get zoom;

  /// The wanted tiles the archive has, with their sizes.
  List<ArchiveTileAt> found();

  /// The wanted tiles the archive has, in ranges to download.
  List<ArchiveRangePlan> ranges();
}

/// Finding a polygon's tiles in the archive, given its first bytes.
abstract class MapPlanning {
  /// Leaf directories to fetch next, as `(start, length)`; empty when done.
  List<(int, int)> neededLeaves();

  Future<void> addLeaf(int start, Uint8List bytes);

  /// The plan, less tiles [basemap] (if given and there) already has.
  Future<MapPlanResult> finish(String? basemap);
}

/// The Rust the engine calls, behind what it needs, so tests can fake it.
abstract class OfflineNative {
  /// The SHA-256 of a file, in lowercase hexadecimal.
  Future<String> sha256File(String path);

  /// How many bytes of an archive to fetch first, for [archivePlanner].
  Future<int> archiveFirstFetch();

  /// A planner for [tiles] of the archive [key] (under `releases/`), from its first bytes.
  Future<ArchivePlanning> archivePlanner(
    String key,
    Uint8List firstBytes,
    List<TileKey> tiles,
  );

  /// Checks every transfer tile in a downloaded range (decoded whole: its CRC-32 covers
  /// the whole tile; its header names its mode, tile and build [manifestId]) and writes
  /// each to its `dest`; returns what's wrong (and writes nothing), or null.
  Future<String?> unpackTransferRange(
    String rangeFile,
    int length,
    List<RangeTransferItem> items,
    String manifestId,
  );

  /// How many bytes of the archive to fetch first, for [mapPlanner].
  Future<int> mapFirstFetch();

  Future<MapPlanning> mapPlanner(Uint8List firstBytes, GeoPolygon polygon);

  /// Creates the basemap if needed, with metadata from the archive's header.
  Future<void> basemapPrepare(String path, Uint8List header, String name);

  /// Checks a downloaded range and writes its tiles as in [region]; returns how many.
  /// Throws [MapRangeError].
  Future<int> basemapUnpack(
    String path,
    String rangeFile,
    int length,
    Uint8List tiles,
    int region,
  );

  /// Adds [region] to the tiles of [tileKeys] the basemap has; returns how many it lacks.
  Future<int> basemapClaim(String path, int region, Uint8List tileKeys);

  /// Takes [region] off its tiles, deleting those no region holds; returns how many.
  Future<int> basemapRelease(String path, int region);
}

/// [OfflineNative] on `routing_flutter` (hashing) and `sparkle_flutter` (the map: it
/// writes the basemap with the SQLite the map reads it with).
class RustOfflineNative implements OfflineNative {
  const RustOfflineNative();

  @override
  Future<String> sha256File(String path) async {
    await routing.initRouting();
    return routing.sha256File(path: path);
  }

  @override
  Future<int> archiveFirstFetch() async {
    await routing.initRouting();
    return routing.archiveFirstFetch();
  }

  @override
  Future<ArchivePlanning> archivePlanner(
    String key,
    Uint8List firstBytes,
    List<TileKey> tiles,
  ) async {
    await routing.initRouting();
    try {
      return _RustArchivePlanning(
        await routing.ArchivePlanner.newInstance(
          firstBytes: firstBytes,
          tiles: _archiveTiles(tiles),
        ),
      );
    } on routing.ArchiveError catch (e) {
      throw StateError('$key: ${e.message}');
    }
  }

  @override
  Future<String?> unpackTransferRange(
    String rangeFile,
    int length,
    List<RangeTransferItem> items,
    String manifestId,
  ) async {
    await routing.initRouting();
    try {
      await routing.unpackTransferRange(
        rangeFile: rangeFile,
        expectedLength: length,
        tiles: [
          for (final i in items)
            routing.RangeTransferTile(
              offset: i.start,
              length: i.length,
              dest: i.dest,
              mode: i.mode,
              x: i.x,
              y: i.y,
            ),
        ],
        manifestId: manifestId,
      );
      return null;
    } on routing.TileCheckError catch (e) {
      return e.message;
    }
  }

  @override
  Future<int> mapFirstFetch() async {
    await sparkle.initSparkle();
    return sparkle.mapFirstFetch();
  }

  @override
  Future<MapPlanning> mapPlanner(
    Uint8List firstBytes,
    GeoPolygon polygon,
  ) async {
    await sparkle.initSparkle();
    try {
      return _RustMapPlanning(
        await sparkle.MapPlanner.newInstance(
          firstBytes: firstBytes,
          polygon: [
            for (final p in polygon.points)
              sparkle.LngLat(lng: p.lng, lat: p.lat),
          ],
          minZoom: 0,
          maxZoom: 14,
        ),
      );
    } on sparkle.MapError catch (e) {
      throw StateError('map archive: ${e.kind.name}: ${e.message}');
    }
  }

  @override
  Future<void> basemapPrepare(
    String path,
    Uint8List header,
    String name,
  ) async {
    await sparkle.initSparkle();
    await _mapCall(
      'preparing the basemap',
      () => sparkle.basemapPrepare(path: path, headerBytes: header, name: name),
    );
  }

  @override
  Future<int> basemapUnpack(
    String path,
    String rangeFile,
    int length,
    Uint8List tiles,
    int region,
  ) async {
    await sparkle.initSparkle();
    try {
      final unpacked = await sparkle.basemapUnpack(
        path: path,
        chunkFile: rangeFile,
        expectedLength: length,
        tiles: tiles,
        region: region,
      );
      return unpacked.tiles;
    } on sparkle.MapError catch (e) {
      throw MapRangeError(switch (e.kind) {
        sparkle.MapErrorKind.chunkLength => MapRangeProblem.length,
        sparkle.MapErrorKind.badTile => MapRangeProblem.badTile,
        _ => MapRangeProblem.storage,
      }, e.message);
    }
  }

  @override
  Future<int> basemapClaim(String path, int region, Uint8List tileKeys) async {
    await sparkle.initSparkle();
    return _mapCall(
      'claiming tiles',
      () =>
          sparkle.basemapClaim(path: path, region: region, tileKeys: tileKeys),
    );
  }

  @override
  Future<int> basemapRelease(String path, int region) async {
    await sparkle.initSparkle();
    return _mapCall(
      'releasing tiles',
      () => sparkle.basemapRelease(path: path, region: region),
    );
  }
}

/// Runs a map operation, turning Rust's [sparkle.MapError] (which doesn't print its
/// message) into a [StateError] that does.
Future<T> _mapCall<T>(String what, Future<T> Function() call) async {
  try {
    return await call();
  } on sparkle.MapError catch (e) {
    throw StateError('$what: ${e.kind.name}: ${e.message}');
  }
}

class _RustMapPlanning implements MapPlanning {
  _RustMapPlanning(this._planner);

  final sparkle.MapPlanner _planner;

  @override
  List<(int, int)> neededLeaves() => [
    for (final range in _planner.neededLeaves()) (range.offset, range.length),
  ];

  @override
  Future<void> addLeaf(int start, Uint8List bytes) => _mapCall(
    'reading a leaf directory',
    () => _planner.addLeaf(offset: start, bytes: bytes),
  );

  @override
  Future<MapPlanResult> finish(String? basemap) async {
    final plan = await _mapCall(
      'planning the map',
      () => _planner.finish(basemap: basemap),
    );
    return (
      coveringTiles: plan.coveringTiles,
      archiveTiles: plan.archiveTiles,
      presentTiles: plan.presentTiles,
      tileBytes: plan.tileBytes,
      rangeBytes: plan.rangeBytes,
      ranges: [
        for (final c in plan.chunks)
          (
            start: c.offset,
            length: c.length,
            tileCount: c.tileCount,
            tiles: c.tiles,
          ),
      ],
      tileKeys: plan.tileKeys,
    );
  }
}

List<routing.ArchiveTile> _archiveTiles(List<TileKey> tiles) => [
  for (final (z, x, y) in tiles) routing.ArchiveTile(z: z, x: x, y: y),
];

class _RustArchivePlanning implements ArchivePlanning {
  _RustArchivePlanning(this._planner);

  final routing.ArchivePlanner _planner;

  @override
  List<(int, int)> neededLeaves() => [
    for (final range in _planner.neededLeaves()) (range.offset, range.length),
  ];

  @override
  Future<void> addLeaf(int start, Uint8List bytes) => _mapCall(
    'reading a leaf directory',
    () => _planner.addLeaf(offset: start, bytes: bytes),
  );

  @override
  void want(List<TileKey> tiles) => _planner.want(tiles: _archiveTiles(tiles));

  @override
  int get zoom {
    try {
      return _planner.zoom();
    } on routing.ArchiveError catch (e) {
      throw StateError(e.message);
    }
  }

  @override
  List<ArchiveTileAt> found() => [
    for (final t in _planner.found())
      (tile: (t.z, t.x, t.y), start: t.offset, length: t.length),
  ];

  @override
  List<ArchiveRangePlan> ranges() => [
    for (final r in _planner.ranges(
      gap: routing.archiveGapBytes(),
      max: routing.archiveMaxRangeBytes(),
    ))
      (
        start: r.offset,
        length: r.length,
        tiles: [
          for (final t in r.tiles)
            (tile: (t.z, t.x, t.y), start: t.offset, length: t.length),
        ],
      ),
  ];
}
