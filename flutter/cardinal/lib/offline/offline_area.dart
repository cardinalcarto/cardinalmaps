/// The device's offline area: a routing release's street tiles and a transit release's
/// timetables, opened once and shared by street directions and transit plans.
///
/// Its releases are the ones downloads pinned (`downloads/engine.dart`), once a region
/// is ready. A dev build can point at releases put in place by hand instead, with the
/// `CARDINAL_ROUTING_RELEASE` and `CARDINAL_TRANSIT_RELEASE` defines (see `config.dart`).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';
import 'package:routing/routing.dart';

import '../config.dart';
import 'downloads/bucket.dart';
import 'downloads/database.dart';
import 'downloads/engine.dart';
import 'downloads/transfers.dart';

/// One offline area, as the releases are laid out under the offline root, the same as
/// the bucket's `releases/`:
///
/// - `routing/<date>/tiles/`, the street tiles, loose: `{level}/nnn/nnn/nnn.gph`;
/// - `transit/<date>/`, a transit build over those tiles: `timetables/` and
///   `transfers/`, which writes its `prepared/` beside them on first open, and
///   `realtime.json`, the GTFS-RT URLs of its feeds.
///
/// Downloads tell it when files arrive or go ([offlineChanged]).
class OfflineArea implements OfflineListener {
  OfflineArea({
    required this.routing,
    required this.transitBuild,
    this.valhallaBuildId,
  });

  /// The routing release's directory, `routing/<date>`.
  final String routing;

  /// The transit release's directory, `transit/<date>`.
  final String transitBuild;

  /// The routing release's Valhalla build, when known: the router then opens with no
  /// tiles yet, and refuses tiles of another build.
  final int? valhallaBuildId;

  String get _tiles => '$routing/tiles';

  /// The street router over the area's tiles, opened on first use.
  Future<OfflineRouter> get router =>
      _router ??= _timed('street router open', () async {
        await initRouting();
        return OfflineRouter.open(
          tileSets: [_tiles],
          expectedBuildId: valhallaBuildId,
        );
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
          expectedBuildId: valhallaBuildId,
        );
      });
  Future<OfflineTransit>? _transit;

  /// The feeds the manifest lists whose timetables are in the area, by ID. (A build put
  /// in place by hand lists every feed it was built with; downloads list only theirs.)
  Future<List<String>> _feeds() async {
    final manifest = File('$transitBuild/transfers/manifest.json');
    final listed =
        jsonDecode(await manifest.readAsString()) as Map<String, Object?>;
    final ids = <String>[
      for (final feed in listed['feeds'] as List<Object?>? ?? const [])
        if (feed case {'id': final String id}) id,
    ];
    return [
      for (final id in ids)
        if (await File('$transitBuild/timetables/$id.timetable').exists()) id,
    ]..sort();
  }

  /// Makes files that arrived usable: the router and transit planner find new street
  /// tiles, and when the feeds changed, transit opens again on next use (its feeds are
  /// fixed when it opens). One that failed to open tries again on next use.
  @override
  Future<void> offlineChanged({required bool feedsChanged}) async {
    if (_router case final opening?) {
      final router = await _settled(opening);
      if (router == null) {
        _router = null;
      } else {
        await router.refreshTiles();
      }
    }
    if (_transit case final opening?) {
      final transit = await _settled(opening);
      if (transit == null || feedsChanged) {
        _transit = null;
      } else {
        await transit.refreshTiles();
      }
    }
  }

  static Future<T?> _settled<T extends Object>(Future<T> opening) async {
    try {
      return await opening;
    } on Object {
      return null;
    }
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

/// The app's offline downloads and the offline area they make, for directions and the
/// map. The area appears once a region is ready, so it is looked for on each use until
/// then.
class OfflineAreas {
  OfflineAreas(this.downloads);

  /// Downloads, opened (null if this build has no [releasesUrl]).
  final Future<OfflineDownloads?> downloads;

  /// Starts downloads on the app's database and folder: handling downloads that ended
  /// while the app was away, and finishing deletions.
  factory OfflineAreas.open() => OfflineAreas(() async {
    if (releasesUrl.isEmpty) return null;
    final downloads = OfflineDownloads(
      db: OfflineDatabase.open(),
      root: await offlineFolder(),
      bucket: Bucket(Uri.parse(releasesUrl)),
      transfers: BackgroundTransfers(),
    );
    try {
      await downloads.open();
    } on Object catch (e) {
      debugPrint('offline: downloads did not open: $e');
    }
    return downloads;
  }());

  OfflineArea? _area;

  /// The offline area, or null if there's none yet.
  Future<OfflineArea?> area() async =>
      _area ??= await configuredOfflineArea(await downloads);

  /// The downloaded map tiles, if there are some.
  Future<String?> basemap() async => (await downloads)?.basemap();
}

/// The offline area, or null if there's none yet: the releases [routingRelease] and
/// [transitRelease] if this build was pointed at some, or else those [downloads] pinned,
/// once a region is ready. It is under [offlineFolder].
Future<OfflineArea?> configuredOfflineArea(OfflineDownloads? downloads) async {
  final root = await offlineFolder();
  if (routingRelease.isNotEmpty && transitRelease.isNotEmpty) {
    return OfflineArea(
      routing: '$root/routing/$routingRelease',
      transitBuild: '$root/transit/$transitRelease',
    );
  }
  final releases = await downloads?.readyReleases();
  if (releases == null) return null;
  final area = OfflineArea(
    routing: '$root/routing/${releases.routing}',
    transitBuild: '$root/transit/${releases.transit ?? '-'}',
    valhallaBuildId: releases.valhallaBuildId,
  );
  downloads!.addListener(area);
  return area;
}

/// The offline folder: [offlineRoot] if set, or else the app's `offline/` folder in its
/// external files on Android (its support folder elsewhere).
Future<String> offlineFolder() async =>
    offlineRoot.isNotEmpty ? offlineRoot : await _appOfflineFolder();

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
