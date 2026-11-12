/// The device's offline area: a routing release's street tiles and a transit release's
/// timetables, opened once and shared by street directions and transit plans.
///
/// There's no download yet: a dev build is pointed at its releases with the
/// `CARDINAL_OFFLINE_ROOT`, `CARDINAL_ROUTING_RELEASE` and `CARDINAL_TRANSIT_RELEASE`
/// defines (see `config.dart`).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';
import 'package:routing/routing.dart';

import '../config.dart';

/// One offline area, as the releases are laid out under the offline root, the same as
/// the bucket's `releases/`:
///
/// - `routing/<date>/tiles/`, the street tiles, loose: `{level}/nnn/nnn/nnn.gph`;
/// - `transit/<date>/`, a transit build over those tiles: `timetables/` and
///   `transfers/`, which writes its `prepared/` beside them on first open, and
///   `realtime.json`, the GTFS-RT URLs of its feeds.
class OfflineArea {
  OfflineArea({required this.routing, required this.transitBuild});

  /// The routing release's directory, `routing/<date>`.
  final String routing;

  /// The transit release's directory, `transit/<date>`.
  final String transitBuild;

  String get _tiles => '$routing/tiles';

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
          build: transitBuild,
          feeds: feeds,
          tileSets: [_tiles],
        );
      });
  Future<OfflineTransit>? _transit;

  /// The feeds whose timetables are in the area, by ID.
  Future<List<String>> _feeds() async {
    const suffix = '.timetable';
    final names = [
      await for (final entry in Directory('$transitBuild/timetables').list())
        if (entry is File && entry.path.endsWith(suffix))
          entry.uri.pathSegments.last,
    ]..sort();
    return [
      for (final name in names) name.substring(0, name.length - suffix.length),
    ];
  }

  /// The GTFS-RT messages of the area's feeds, as `realtime.json` lists them.
  Future<List<RealtimeSource>> get realtimeSources => _realtime ??= () async {
    final file = File('$transitBuild/realtime.json');
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
  Future<bool> covers(double lat, double lng) =>
      File('$_tiles/${_levelTwoTile(lat, lng)}').exists();

  /// The path of the level 2 tile holding [lat], [lng], under the tiles. Level 2 tiles are
  /// a quarter of a degree on a side, numbered by row from the south pole and column from
  /// the antimeridian, and tile 789349 is `2/000/789/349.gph`.
  static String _levelTwoTile(double lat, double lng) {
    final row = ((lat + 90) * 4).floor().clamp(0, 719);
    final column = ((lng + 180) * 4).floor().clamp(0, 1439);
    final id = (row * 1440 + column).toString().padLeft(9, '0');
    return '2/${id.substring(0, 3)}/${id.substring(3, 6)}/${id.substring(6)}.gph';
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

/// The offline area this build was pointed at, or null if there's none: the routing
/// and transit releases [routingRelease] and [transitRelease] under [offlineRoot], or
/// under the app's `offline/` folder in its external files on Android (its support
/// folder elsewhere) if that's empty.
Future<OfflineArea?> configuredOfflineArea() async {
  if (routingRelease.isEmpty || transitRelease.isEmpty) return null;
  final root = offlineRoot.isNotEmpty ? offlineRoot : await _appOfflineFolder();
  return OfflineArea(
    routing: '$root/routing/$routingRelease',
    transitBuild: '$root/transit/$transitRelease',
  );
}

Future<String> _appOfflineFolder() async {
  Directory? base;
  try {
    base = await getExternalStorageDirectory();
  } on Object {
    base = null;
  }
  base ??= await getApplicationSupportDirectory();
  return '${base.path}/offline';
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
