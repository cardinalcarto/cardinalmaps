/// The device's offline area: street tiles and transit timetables in one directory,
/// opened once and shared by street directions and transit plans.
///
/// There's no download yet: a dev build is pointed at a directory with the
/// `CARDINAL_OFFLINE_AREA` define (see `config.dart`).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';
import 'package:routing/routing.dart';

import '../config.dart';

/// One offline area's directory:
///
/// - `tiles.tar`, the street tiles (a mainline `build_extract` tarball);
/// - `timetables/` and `transfers/`, a transit build over those tiles, which writes its
///   `prepared/` beside them on first open;
/// - `realtime.json`, the GTFS-RT URLs of its feeds.
class OfflineArea {
  OfflineArea(this.directory);

  final String directory;

  String get _tiles => '$directory/tiles.tar';

  /// The street router over the area's tiles, opened on first use.
  Future<OfflineRouter> get router =>
      _router ??= _timed('street router open', () async {
        await initRouting();
        return OfflineRouter.open(tileSets: [_tiles]);
      });
  Future<OfflineRouter>? _router;

  /// Transit planning over the area's timetables and tiles, opened on first use. This
  /// takes seconds, and the first open of a build takes longer, as it prepares its
  /// transfer tables.
  ///
  /// The build's manifest lists every feed it was built with; the area holds only some
  /// of their timetables, and those are the feeds it loads.
  Future<OfflineTransit> get transit =>
      _transit ??= _timed('transit open', () async {
        await initRouting();
        final feeds = await _feeds();
        debugPrint('offline: transit feeds ${feeds.join(', ')}');
        return OfflineTransit.open(
          build: directory,
          feeds: feeds,
          tileSets: [_tiles],
        );
      });
  Future<OfflineTransit>? _transit;

  /// The feeds whose timetables are in the area, by ID.
  Future<List<String>> _feeds() async {
    const suffix = '.timetable';
    final names = [
      await for (final entry in Directory('$directory/timetables').list())
        if (entry is File && entry.path.endsWith(suffix))
          entry.uri.pathSegments.last,
    ]..sort();
    return [
      for (final name in names) name.substring(0, name.length - suffix.length),
    ];
  }

  /// The GTFS-RT messages of the area's feeds, as `realtime.json` lists them.
  Future<List<RealtimeSource>> get realtimeSources => _realtime ??= () async {
    final file = File('$directory/realtime.json');
    if (!await file.exists()) return const <RealtimeSource>[];
    final json = jsonDecode(await file.readAsString()) as Map<String, Object?>;
    return [
      for (final feed in json['feeds'] as List<Object?>? ?? const [])
        if (feed case {'id': final String id, 'url': final String url})
          RealtimeSource(feed: id, url: Uri.parse(url)),
    ];
  }();
  Future<List<RealtimeSource>>? _realtime;

  /// Whether the area's streets reach [lat], [lng]: whether the tiles hold the
  /// quarter-degree square it's in.
  Future<bool> covers(double lat, double lng) async {
    final squares = await (_coverage ??= _readCoverage());
    return squares.contains(_square(lat, lng));
  }

  Future<Set<int>>? _coverage;

  /// Valhalla's level 2 tiles, which are a quarter of a degree on a side, numbered by
  /// row from the south pole and column from the antimeridian.
  static int _square(double lat, double lng) {
    final row = ((lat + 90) * 4).floor().clamp(0, 719);
    final column = ((lng + 180) * 4).floor().clamp(0, 1439);
    return row * 1440 + column;
  }

  /// The level 2 tiles in the tarball, from its headers: `2/000/789/349.gph` is 789349.
  Future<Set<int>> _readCoverage() async {
    final squares = <int>{};
    final file = await File(_tiles).open();
    try {
      final length = await file.length();
      var offset = 0;
      while (offset + 512 <= length) {
        await file.setPosition(offset);
        final header = await file.read(512);
        if (header.every((byte) => byte == 0)) break;
        final name = _field(header, 0, 100);
        final size =
            int.tryParse(_field(header, 124, 12).trim(), radix: 8) ?? 0;
        final match = RegExp(r'^2/(\d{3})/(\d{3})/(\d{3})\.gph')
            .firstMatch(name);
        if (match != null) {
          squares.add(int.parse('${match[1]}${match[2]}${match[3]}'));
        }
        offset += 512 + (size + 511) ~/ 512 * 512;
      }
    } finally {
      await file.close();
    }
    return squares;
  }

  static String _field(Uint8List header, int start, int length) {
    final bytes = header.sublist(start, start + length);
    final end = bytes.indexOf(0);
    return latin1.decode(end < 0 ? bytes : bytes.sublist(0, end));
  }
}

/// One GTFS-RT message an area's feed publishes.
@immutable
class RealtimeSource {
  const RealtimeSource({required this.feed, required this.url});

  /// The feed's ID in the transit build.
  final String feed;
  final Uri url;

  /// What the message carries. `realtime.json` doesn't say, so it's read from the URL's
  /// file name or its `feed_type`, never the rest of the URL: King County's trip updates
  /// are in a bucket named `kcm-alerts-realtime-prod`. A message that's none of these, or
  /// several, is combined.
  TransitFeedKind get kind {
    final name = [
      url.pathSegments.lastOrNull ?? '',
      url.queryParameters['feed_type'] ?? '',
    ].join(' ').toLowerCase().replaceAll(RegExp('[^a-z ]'), '');
    final kinds = {
      if (name.contains('alert')) TransitFeedKind.alerts,
      if (name.contains('vehicle')) TransitFeedKind.vehiclePositions,
      if (name.contains('trip')) TransitFeedKind.tripUpdates,
    };
    return kinds.length == 1 ? kinds.single : TransitFeedKind.combined;
  }
}

/// The offline area this build was pointed at, or null if there's none.
///
/// [offlineAreaPath] is a directory, or a name under the app's `offline/` folder in its
/// external files on Android (its support folder elsewhere).
Future<OfflineArea?> configuredOfflineArea() async {
  if (offlineAreaPath.isEmpty) return null;
  if (offlineAreaPath.startsWith('/')) return OfflineArea(offlineAreaPath);
  Directory? base;
  try {
    base = await getExternalStorageDirectory();
  } on Object {
    base = null;
  }
  base ??= await getApplicationSupportDirectory();
  return OfflineArea('${base.path}/offline/$offlineAreaPath');
}

/// Runs [open], logging how long it took, for measuring on a device.
Future<T> _timed<T>(String what, Future<T> Function() open) async {
  final watch = Stopwatch()..start();
  try {
    return await open();
  } on TransitOpenError catch (e) {
    debugPrint('offline: $what failed: ${e.message}');
    rethrow;
  } on OpenError catch (e) {
    debugPrint('offline: $what failed: ${e.kind.name}: ${e.message}');
    rethrow;
  } finally {
    debugPrint('cardinal-timing: $what ${watch.elapsedMilliseconds} ms');
  }
}
