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

  /// What's wrong with a downloaded transfer tile, or null if it decodes whole (its
  /// CRC-32 covers the whole tile) as [mode] tile [x], [y] of build [manifestId].
  Future<String?> checkTransferTile(
    String path,
    String mode,
    int x,
    int y,
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
  Future<String?> checkTransferTile(
    String path,
    String mode,
    int x,
    int y,
    String manifestId,
  ) async {
    await routing.initRouting();
    try {
      await routing.checkTransferTile(
        path: path,
        mode: mode,
        x: x,
        y: y,
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
    await sparkle.basemapPrepare(path: path, headerBytes: header, name: name);
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
    return sparkle.basemapClaim(path: path, region: region, tileKeys: tileKeys);
  }

  @override
  Future<int> basemapRelease(String path, int region) async {
    await sparkle.initSparkle();
    return sparkle.basemapRelease(path: path, region: region);
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
  Future<void> addLeaf(int start, Uint8List bytes) =>
      _planner.addLeaf(offset: start, bytes: bytes);

  @override
  Future<MapPlanResult> finish(String? basemap) async {
    final plan = await _planner.finish(basemap: basemap);
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
