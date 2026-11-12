// A small release bucket, written as `cardinal-publish --target dir:` lays one out: real
// tile paths and index formats around Seattle, with made-up file contents.
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:cardinal/offline/downloads/geometry.dart';
import 'package:crypto/crypto.dart';

const routingDate = '2026-11-11';
const transitDate = '2026-11-11';
const mapDate = '2026-11-11';
const buildId = 57897;

/// Downtown Seattle.
final downtown = GeoPolygon(const [
  GeoPoint(47.595, -122.36),
  GeoPoint(47.595, -122.31),
  GeoPoint(47.62, -122.30),
  GeoPoint(47.63, -122.34),
]);

/// Overlapping [downtown] on its east side, and reaching the next level 2 tile east.
final capitolHill = GeoPolygon(const [
  GeoPoint(47.60, -122.33),
  GeoPoint(47.60, -122.20),
  GeoPoint(47.64, -122.20),
  GeoPoint(47.64, -122.33),
]);

/// Across the water, sharing only the big tiles with [downtown].
final bainbridge = GeoPolygon(const [
  GeoPoint(47.60, -122.56),
  GeoPoint(47.60, -122.50),
  GeoPoint(47.66, -122.50),
  GeoPoint(47.66, -122.56),
]);

class FakeRelease {
  FakeRelease(this.root);

  /// Holds `releases/` and `.meta/`.
  final String root;
  final _random = Random(7);

  /// Feeds: Metro (index 2140) with stops downtown and on Capitol Hill, Kitsap (2137)
  /// on Bainbridge, and a feed (2152) with stops only far away.
  static const feeds = {
    'us-wa-Metro-Transit': 2140,
    'us-wa-Kitsap-Transit': 2137,
    'us-wa-Far-Away': 2152,
  };

  /// Every L2 tile in [downtown], [capitolHill], and [bainbridge], and one more.
  late final List<String> routingTiles = {
    for (final p in [downtown, capitolHill, bainbridge])
      for (var level = 0; level < 3; level++)
        for (final id in ValhallaTiles.covering(level, p))
          'tiles/${ValhallaTiles.path(level, id)}',
    // Not in any of them.
    'tiles/${ValhallaTiles.path(2, ValhallaTiles.idAt(2, 47.9, -122.2))}',
  }.toList()..sort();

  Uint8List bytes(int n) =>
      Uint8List.fromList([for (var i = 0; i < n; i++) _random.nextInt(256)]);

  static String hex(List<int> bytes) => sha256.convert(bytes).toString();

  Future<void> put(
    String key,
    List<int> body, {
    String contentType = 'application/octet-stream',
    bool gzipped = false,
  }) async {
    final stored = gzipped ? gzip.encode(body) : body;
    final file = File('$root/releases/$key');
    await file.parent.create(recursive: true);
    await file.writeAsBytes(stored);
    final meta = File('$root/.meta/releases/$key.json');
    await meta.parent.create(recursive: true);
    await meta.writeAsString(
      jsonEncode({
        'size': stored.length,
        'sha256': hex(stored),
        'content_type': contentType,
        'content_encoding': gzipped ? 'gzip' : null,
      }),
    );
  }

  Future<void> putJson(String key, Object? json, {bool gzipped = true}) => put(
    key,
    utf8.encode(jsonEncode(json)),
    contentType: 'application/json',
    gzipped: gzipped,
  );

  /// The transfer tiles of each feed and mode, as `[x, y]`.
  final transferTiles = <String, Map<String, List<List<int>>>>{};

  /// Every transfer tile's bytes, by `(mode, x, y)`.
  final transferTileBytes = <(String, int, int), Uint8List>{};

  /// By archive (under `releases/`), where each tile is: `(start, length)`.
  final archives = <String, Map<(int, int, int), (int, int)>>{};

  /// The bytes before an archive's first tile.
  static const archiveHeader = 16384;

  /// The zoom of the transfer tiles (valinor's `TILE_ZOOM`).
  static const transferZoom = 14;

  /// Publishes another transit release, over another routing release, as latest.
  Future<void> publishOtherTransit(String date) async {
    await putJson('latest.json', {
      'map': mapDate,
      'routing': routingDate,
      'transit': date,
    }, gzipped: false);
    await putJson('transit/$date/release.json', {
      'kind': 'transit',
      'date': date,
      'manifest_id': '0000000000000001',
      'routing_release': '2026-10-01',
      'valhalla_build_id': buildId + 1,
    }, gzipped: false);
  }

  Future<void> write() async {
    await putJson('latest.json', {
      'map': mapDate,
      'routing': routingDate,
      'transit': transitDate,
    }, gzipped: false);

    // Routing: index shards by level 0 cell.
    final shards = <int, List<Map<String, Object?>>>{};
    for (final path in routingTiles) {
      final (level, id) = ValhallaTiles.parsePath(path)!;
      final body = bytes(2000 + level * 1000);
      await put('routing/$routingDate/$path', body);
      final b = ValhallaTiles.bounds(level, id);
      final cell = ValhallaTiles.idAt(
        0,
        (b.south + b.north) / 2,
        (b.west + b.east) / 2,
      );
      shards.putIfAbsent(cell, () => []).add({
        'path': path,
        'bytes': body.length,
        'sha256': hex(body),
      });
    }
    for (final MapEntry(key: cell, value: entries) in shards.entries) {
      await putJson('routing/$routingDate/index/$cell.json', entries);
    }
    await putJson('routing/$routingDate/release.json', {
      'kind': 'routing',
      'date': routingDate,
      'valhalla_build_id': buildId,
      'index_cells': shards.keys.toList()..sort(),
    }, gzipped: false);

    // Transit.
    // Each feed's stops: the index lists their zoom 12 cells, and transfer tiles are at
    // zoom [transferZoom].
    final stopsOf = <String, List<(double, double)>>{
      'us-wa-Metro-Transit': [
        (47.61, -122.335),
        (47.62, -122.31),
        (47.63, -122.30),
      ],
      'us-wa-Kitsap-Transit': [(47.63, -122.52)],
      'us-wa-Far-Away': [(47.2, -122.9)],
    };
    final byIndexTile = <(int, int), List<Map<String, Object?>>>{};
    final shared = <String, Object?>{
      'manifest_id': 'bd1c61618c8c9ac9',
      'valhalla_build_id': buildId,
      'tile_format_version': 1,
      'zoom': transferZoom,
      'walking_bound_seconds': 900,
      'cycling_bound_seconds': 900,
      'feed_count': 2170,
      'feed_ids': ['a', 'b'],
    };
    for (final MapEntry(key: id, value: index) in feeds.entries) {
      final cells = {
        for (final (lat, lng) in stopsOf[id]!) SlippyTiles.at(12, lat, lng),
      };
      final stopTiles = [
        for (final (lat, lng) in stopsOf[id]!)
          SlippyTiles.at(transferZoom, lat, lng),
      ];
      for (final (x, y) in cells) {
        byIndexTile.putIfAbsent((x >> 6, y >> 6), () => []).add({
          'id': id,
          'cells': [
            [x, y],
          ],
        });
      }
      final timetable = bytes(5000);
      await put('transit/$transitDate/timetables/$id.timetable', timetable);
      final modes = <String, Object?>{};
      final snaps = <String, Object?>{};
      final tiles = <String, List<List<int>>>{};
      for (final mode in ['walking', 'cycling']) {
        final snap = bytes(300);
        await put('transit/$transitDate/snaps/$mode/$index.bin', snap);
        var transferBytes = 0;
        // A tile per stop cell, and its neighbour to the east.
        final modeTiles = <List<int>>[
          for (final (x, y) in stopTiles) ...[
            [x, y],
            [x + 1, y],
          ],
        ];
        for (final [x, y] in modeTiles) {
          final tile = transferTileBytes.putIfAbsent((
            mode,
            x,
            y,
          ), () => bytes(700 + (x + y) % 200));
          transferBytes += tile.length;
        }
        tiles[mode] = modeTiles;
        modes[mode] = {
          'transfer_tiles': modeTiles.length,
          'transfer_bytes': transferBytes,
          'snaps': {
            'path': 'snaps/$mode/$index.bin',
            'bytes': snap.length,
            'sha256': hex(snap),
          },
        };
        snaps[mode] = {
          'file': 'transfers/snaps/$mode/$index.bin',
          'sha256': hex(snap),
        };
      }
      transferTiles[id] = tiles;
      await putJson('transit/$transitDate/feeds/$id.json', {
        'id': id,
        'index': index,
        'display_name': id.replaceFirst('us-wa-', ''),
        'agencies': [id],
        'timetable': {
          'path': 'timetables/$id.timetable',
          'bytes': timetable.length,
          'sha256': hex(timetable),
        },
        'first_service_date': '2026-08-31',
        'last_service_date': '2027-12-31',
        'modes': modes,
        'realtime': <String>[],
      });
      await putJson('transit/$transitDate/manifest/feeds/$id.json', {
        'id': id,
        'index': index,
        'timetable': 'timetables/$id.timetable',
        'timetable_sha256': hex(timetable),
        'stop_count': 10,
        'transfer_tiles': tiles,
        'stop_snaps': snaps,
      });
    }
    for (final MapEntry(key: (x, y), value: feeds) in byIndexTile.entries) {
      await putJson('transit/$transitDate/index/$x/$y.json', {
        'tile': [x, y],
        'feeds': feeds,
      });
    }
    // Each mode's transfer tiles in one archive: [archiveHeader] bytes (where a real
    // archive's header and directories are), then the tiles, in any order; where each
    // tile is goes in [archives] for FakeNative's planner.
    for (final mode in ['walking', 'cycling']) {
      final body = BytesBuilder()..add(Uint8List(archiveHeader));
      final at = <(int, int, int), (int, int)>{};
      for (final MapEntry(key: (m, x, y), value: tile)
          in transferTileBytes.entries) {
        if (m != mode) continue;
        at[(transferZoom, x, y)] = (body.length, tile.length);
        body.add(tile);
      }
      final key = 'transit/$transitDate/transfers/$mode.pmtiles';
      archives[key] = at;
      await put(key, body.takeBytes(), contentType: 'application/vnd.pmtiles');
    }
    await putJson('transit/$transitDate/manifest/shared.json', shared);
    await putJson('transit/$transitDate/realtime.json', {
      'feeds': [
        {
          'id': 'us-wa-Metro-Transit',
          'url': 'https://example.org/tripupdates.pb',
        },
      ],
    });
    await putJson('transit/$transitDate/release.json', {
      'kind': 'transit',
      'date': transitDate,
      'manifest_id': 'bd1c61618c8c9ac9',
      'routing_release': routingDate,
      'valhalla_build_id': buildId,
    }, gzipped: false);

    // The map: bytes the fake planner hands out ranges of.
    await put(
      'map/$mapDate/planet.pmtiles',
      bytes(400 * 1000),
      contentType: 'application/vnd.pmtiles',
    );
    await putJson('map/$mapDate/release.json', {
      'kind': 'map',
      'date': mapDate,
    }, gzipped: false);
  }
}
