// Fakes for the engine's downloader and Rust, over real HTTP to a FakeBucket.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:cardinal/offline/downloads/engine.dart';
import 'package:cardinal/offline/downloads/geometry.dart';
import 'package:cardinal/offline/downloads/native.dart';
import 'package:cardinal/offline/downloads/transfers.dart';
import 'package:crypto/crypto.dart';

/// [Transfers] that download over `dart:io` as soon as they're handed jobs (or when
/// [run], with [hold]), reporting results as `background_downloader` does.
class FakeTransfers implements Transfers {
  final _results = StreamController<TransferResult>.broadcast();
  final _client = HttpClient();

  /// Jobs not ended, by ID.
  final jobs = <String, TransferJob>{};

  /// Every job handed over, in order.
  final history = <TransferJob>[];

  /// Don't run jobs as they come; [run] them.
  bool hold = false;
  final _running = <Future<void>>{};

  @override
  Stream<TransferResult> get results => _results.stream;

  @override
  Future<void> start() async {}

  @override
  Future<void> enqueue(List<TransferJob> jobs) async {
    for (final job in jobs) {
      this.jobs[job.id] = job;
      history.add(job);
      if (!hold) _track(_run(job));
    }
  }

  void _track(Future<void> f) {
    _running.add(f);
    f.whenComplete(() => _running.remove(f));
  }

  /// Runs held jobs (those [ids], or all).
  void run([Iterable<String>? ids]) {
    for (final id in ids ?? jobs.keys.toList()) {
      if (jobs[id] case final job?) _track(_run(job));
    }
  }

  /// Waits for running jobs.
  Future<void> drain() async {
    while (_running.isNotEmpty) {
      await Future.wait(List.of(_running));
    }
  }

  Future<void> _run(TransferJob job) async {
    TransferResult result;
    try {
      final request = await _client.getUrl(job.url);
      job.headers.forEach(request.headers.set);
      final response = await request.close();
      final headers = <String, String>{};
      response.headers.forEach(
        (k, v) => headers[k.toLowerCase()] = v.join(','),
      );
      if (response.statusCode >= 400) {
        await response.drain<void>();
        result = TransferResult(
          job.id,
          response.statusCode == 404
              ? TransferOutcome.notFound
              : TransferOutcome.failed,
          statusCode: response.statusCode,
          error: 'status ${response.statusCode}',
        );
      } else {
        final part = File('${job.path}.part');
        await part.parent.create(recursive: true);
        final sink = part.openWrite();
        await response.pipe(sink);
        await part.rename(job.path);
        result = TransferResult(
          job.id,
          TransferOutcome.complete,
          statusCode: response.statusCode,
          headers: headers,
        );
      }
    } on Object catch (e) {
      result = TransferResult(job.id, TransferOutcome.failed, error: '$e');
    }
    if (jobs.remove(job.id) != null) _results.add(result);
  }

  @override
  Future<Set<String>> active() async => jobs.keys.toSet();

  @override
  Future<void> cancel(List<String> ids) async {
    for (final id in ids) {
      if (jobs.remove(id) != null) {
        _results.add(TransferResult(id, TransferOutcome.canceled));
      }
    }
  }

  void close() => _client.close(force: true);
}

/// The Rust side in Dart: SHA-256 with `package:crypto`, and a map whose "tiles" are
/// 0.02° cells of a polygon, each a kilobyte of the archive, kept in memory.
class FakeNative implements OfflineNative {
  FakeNative(this.archive);

  /// The map archive in the bucket, which ranges are checked against.
  final String archive;

  /// Basemap tiles, by path, and the regions holding each.
  final basemaps = <String, Map<String, Set<int>>>{};

  /// Called while a region claims its map tiles (as it starts).
  Future<void> Function(int region)? onClaim;

  @override
  Future<String> sha256File(String path) async =>
      sha256.convert(await File(path).readAsBytes()).toString();

  @override
  int get mapFirstFetch => 16384;

  @override
  Future<MapPlanning> mapPlanner(
    Uint8List firstBytes,
    GeoPolygon polygon,
  ) async => _FakePlanning(this, polygon);

  @override
  Future<void> basemapPrepare(
    String path,
    Uint8List header,
    String name,
  ) async {
    await File(path).create(recursive: true);
    basemaps.putIfAbsent(path, () => {});
  }

  @override
  Future<int> basemapUnpack(
    String path,
    String rangeFile,
    int length,
    Uint8List tiles,
    int region,
  ) async {
    final range = await File(rangeFile).readAsBytes();
    if (range.length != length) {
      throw MapRangeError(
        MapRangeProblem.length,
        '${range.length} bytes, not $length',
      );
    }
    final json = jsonDecode(utf8.decode(tiles)) as Map<String, Object?>;
    final start = json['start']! as int;
    final archive = await File(this.archive).readAsBytes();
    final list = (json['tiles']! as List<Object?>).cast<List<Object?>>();
    for (final [key, offset, size] in list) {
      final at = offset! as int;
      for (var i = 0; i < (size! as int); i++) {
        if (range[at + i] != archive[start + at + i]) {
          throw MapRangeError(MapRangeProblem.badTile, 'tile $key is corrupt');
        }
      }
    }
    final basemap = basemaps.putIfAbsent(path, () => {});
    for (final [key, _, _] in list) {
      basemap.putIfAbsent(key! as String, () => {}).add(region);
    }
    return list.length;
  }

  @override
  Future<int> basemapClaim(String path, int region, Uint8List tileKeys) async {
    await onClaim?.call(region);
    final basemap = basemaps.putIfAbsent(path, () => {});
    var missing = 0;
    for (final key
        in (jsonDecode(utf8.decode(tileKeys)) as List).cast<String>()) {
      if (basemap[key] case final regions?) {
        regions.add(region);
      } else {
        missing++;
      }
    }
    return missing;
  }

  @override
  Future<int> basemapRelease(String path, int region) async {
    final basemap = basemaps[path] ?? {};
    var removed = 0;
    for (final key in basemap.keys.toList()) {
      final regions = basemap[key]!..remove(region);
      if (regions.isEmpty) {
        basemap.remove(key);
        removed++;
      }
    }
    return removed;
  }
}

class _FakePlanning implements MapPlanning {
  _FakePlanning(this.native, this.polygon);

  final FakeNative native;
  final GeoPolygon polygon;

  @override
  List<(int, int)> neededLeaves() => const [];

  @override
  Future<void> addLeaf(int start, Uint8List bytes) async {}

  @override
  Future<MapPlanResult> finish(String? basemap) async {
    const size = 0.02;
    final b = polygon.bounds;
    final tiles = <(String, int)>[];
    for (var i = (b.west / size).floor(); i * size <= b.east; i++) {
      for (var j = (b.south / size).floor(); j * size <= b.north; j++) {
        final rect = (
          west: i * size,
          south: j * size,
          east: (i + 1) * size,
          north: (j + 1) * size,
        );
        if (!polygon.intersects(rect)) continue;
        final offset = 16384 + ((i * 7919 + j * 104729) % 380).abs() * 1000;
        tiles.add(('$i:$j', offset));
      }
    }
    final present = native.basemaps[basemap] ?? const {};
    final fetch = tiles.where((t) => !present.containsKey(t.$1)).toList()
      ..sort((a, c) => a.$2.compareTo(c.$2));
    final ranges = <MapRange>[];
    var group = <(String, int)>[];
    void flush() {
      if (group.isEmpty) return;
      final start = group.first.$2;
      final end = group.map((t) => t.$2 + 1000).reduce((a, c) => a > c ? a : c);
      ranges.add((
        start: start,
        length: end - start,
        tileCount: group.length,
        tiles: utf8.encode(
          jsonEncode({
            'start': start,
            'tiles': [
              for (final (key, offset) in group) [key, offset - start, 1000],
            ],
          }),
        ),
      ));
      group = [];
    }

    for (final t in fetch) {
      if (group.isNotEmpty &&
          t.$2 >
              group.map((g) => g.$2 + 1000).reduce((a, c) => a > c ? a : c) +
                  2000) {
        flush();
      }
      group.add(t);
    }
    flush();
    final unique = {for (final t in fetch) t.$2};
    return (
      coveringTiles: tiles.length,
      archiveTiles: tiles.length,
      presentTiles: tiles.length - fetch.length,
      tileBytes: unique.length * 1000,
      rangeBytes: ranges.fold(0, (sum, r) => sum + r.length),
      ranges: ranges,
      tileKeys: utf8.encode(jsonEncode([for (final t in tiles) t.$1])),
    );
  }
}

/// Records what downloads told it.
class RecordingListener implements OfflineListener {
  final changes = <bool>[];

  @override
  Future<void> offlineChanged({required bool feedsChanged}) async =>
      changes.add(feedsChanged);
}
