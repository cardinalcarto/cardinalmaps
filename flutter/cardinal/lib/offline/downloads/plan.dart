import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import 'bucket.dart';
import 'database.dart';
import 'geometry.dart';
import 'native.dart';

/// The releases a plan downloads from: the pinned ones, or, before the first download,
/// those `latest.json` names (pinned when that download starts).
@immutable
class PlanReleases {
  const PlanReleases({
    required this.map,
    required this.routing,
    required this.transit,
    required this.valhallaBuildId,
    required this.mapEtag,
    required this.pinned,
    this.manifestId,
  });

  final String? map;
  final String routing;
  final String? transit;
  final int valhallaBuildId;

  /// The transit build's manifest ID, which its transfer tiles carry.
  final String? manifestId;

  /// The map archive's ETag, which its ranges must match.
  final String? mapEtag;

  /// Whether these are the pinned releases (false: from `latest.json`, to pin).
  final bool pinned;
}

/// A file a region needs, where it is under its release, with its size and SHA-256
/// when the index gives them.
@immutable
class PlannedFile {
  const PlannedFile({
    required this.kind,
    required this.release,
    required this.path,
    required this.installPath,
    this.bytes,
    this.sha256,
  });

  final ReleaseKind kind;
  final String release;
  final String path;
  final String installPath;
  final int? bytes;
  final String? sha256;

  /// Small JSON the engine fetches itself rather than through the background
  /// downloader: stored gzipped, so only its stored bytes match the bucket's SHA-256.
  bool get foreground => path.endsWith('.json');
}

/// A transit feed with stops in the polygon, for the user to choose.
@immutable
class CandidateFeed {
  const CandidateFeed({
    required this.id,
    required this.index,
    required this.displayName,
    required this.agencies,
    required this.firstServiceDate,
    required this.lastServiceDate,
    required this.files,
    required this.transferTiles,
  });

  final String id;

  /// Its index in the transit build.
  final int index;
  final String displayName;
  final List<String> agencies;
  final String? firstServiceDate;
  final String? lastServiceDate;

  /// Its timetable, snaps, and manifest entry.
  final List<PlannedFile> files;

  /// By mode (`walking`, `cycling`, `wheelchair`), the transfer tiles with walks from
  /// its stops that intersect the polygon, as `(x, y, bytes)` at zoom 12, sizes from the
  /// index shards.
  final Map<String, List<(int, int, int)>> transferTiles;

  /// The transfer tiles' size.
  int get transferBytes => transferTiles.values.fold(
    0,
    (sum, tiles) => tiles.fold(sum, (sum, t) => sum + t.$3),
  );

  /// What the feed costs alone: its files and its transfer tiles.
  int get bytes =>
      files.fold(0, (sum, f) => sum + (f.bytes ?? 0)) + transferBytes;

  int get fileCount =>
      files.length +
      transferTiles.values.fold(0, (sum, tiles) => sum + tiles.length);
}

/// What downloading a polygon takes, for confirming before [OfflineDownloads.start].
@immutable
class OfflinePlan {
  const OfflinePlan({
    required this.polygon,
    required this.releases,
    required this.routingFiles,
    required this.transitShared,
    required this.feeds,
    required this.map,
    required this.mapHeader,
    required this.present,
  });

  final GeoPolygon polygon;
  final PlanReleases releases;

  /// Level 0 to 2 routing tiles that intersect the polygon.
  final List<PlannedFile> routingFiles;

  /// The transit release's files every feed needs: the manifest's shared fields and
  /// the real-time sources.
  final List<PlannedFile> transitShared;

  /// Feeds with stops in the polygon, by index.
  final List<CandidateFeed> feeds;

  /// The map's tiles and ranges, or null if there's no map release.
  final MapPlanResult? map;

  /// The archive's first bytes, for the basemap's metadata.
  final Uint8List? mapHeader;

  /// `kind/release/path` of files already downloaded for another region.
  final Set<String> present;

  static String key(ReleaseKind kind, String release, String path) =>
      '${kind.name}/$release/$path';

  bool _isPresent(PlannedFile f) =>
      present.contains(key(f.kind, f.release, f.path));

  /// The sizes to show before downloading, with [feeds] chosen (by ID).
  PlanTotals totals(Set<String> feeds) {
    final chosen = this.feeds.where((f) => feeds.contains(f.id)).toList();
    final files = [
      ...routingFiles,
      if (chosen.isNotEmpty) ...transitShared,
      for (final f in chosen) ...f.files,
    ];
    final missing = files.where((f) => !_isPresent(f));
    // Transfer tiles are shared between feeds: each counted once.
    final tiles = <(String, int, int), int>{};
    for (final feed in chosen) {
      for (final MapEntry(key: mode, value: list)
          in feed.transferTiles.entries) {
        for (final (x, y, bytes) in list) {
          if (!present.contains(
            key(
              ReleaseKind.transit,
              releases.transit!,
              transferPath(mode, x, y),
            ),
          )) {
            tiles[(mode, x, y)] = bytes;
          }
        }
      }
    }
    final transferBytes = tiles.values.fold(0, (a, b) => a + b);
    final routingBytes = routingFiles
        .where((f) => !_isPresent(f))
        .fold(0, (sum, f) => sum + (f.bytes ?? 0));
    final mapBytes = map?.rangeBytes ?? 0;
    return PlanTotals(
      downloadBytes:
          missing.fold(0, (sum, f) => sum + (f.bytes ?? 0)) +
          transferBytes +
          mapBytes,
      routingBytes: routingBytes,
      transitBytes:
          missing
              .where((f) => f.kind == ReleaseKind.transit)
              .fold(0, (sum, f) => sum + (f.bytes ?? 0)) +
          transferBytes,
      mapBytes: mapBytes,
      files: missing.length + tiles.length,
      mapRanges: map?.ranges.length ?? 0,
      mapTiles: map?.archiveTiles ?? 0,
    );
  }
}

/// The sizes of a plan's download.
@immutable
class PlanTotals {
  const PlanTotals({
    required this.downloadBytes,
    required this.routingBytes,
    required this.transitBytes,
    required this.mapBytes,
    required this.files,
    required this.mapRanges,
    required this.mapTiles,
  });

  /// Everything to download, less what other regions brought already.
  final int downloadBytes;
  final int routingBytes;
  final int transitBytes;

  /// The map's ranges, gaps between tiles included.
  final int mapBytes;

  /// Files to download (map ranges not included).
  final int files;
  final int mapRanges;
  final int mapTiles;
}

/// Where a transfer tile is under its release.
String transferPath(String mode, int x, int y) =>
    'transfers/$mode/12/$x/$y.bin.zst';

/// Where a transit release's file goes under the offline folder: `snaps/` under
/// `transfers/`, as the manifest names them; everything else at its own path.
String transitInstallPath(String release, String path) =>
    path.startsWith('snaps/')
    ? 'transit/$release/transfers/$path'
    : 'transit/$release/$path';

/// The mode and tile of a transfer tile's path under its release, or null.
(String, int, int)? parseTransferPath(String path) {
  final match = RegExp(r'^transfers/([a-z]+)/12/(\d+)/(\d+)\.bin\.zst$')
      .firstMatch(path);
  if (match == null) return null;
  return (match[1]!, int.parse(match[2]!), int.parse(match[3]!));
}

/// Reading what a plan needs from the bucket.
class Planner {
  Planner(this.bucket, this.native);

  final Bucket bucket;
  final OfflineNative native;

  /// The releases to plan with: [pinned] if there are any, else `latest.json`'s.
  /// Throws [StateError] if the transit release isn't over the routing release.
  Future<PlanReleases> releases(Map<ReleaseKind, ReleaseRow> pinned) async {
    if (pinned[ReleaseKind.routing] case final routing?) {
      return PlanReleases(
        map: pinned[ReleaseKind.map]?.date,
        routing: routing.date,
        transit: pinned[ReleaseKind.transit]?.date,
        valhallaBuildId: routing.valhallaBuildId!,
        mapEtag: pinned[ReleaseKind.map]?.etag,
        pinned: true,
        manifestId: pinned[ReleaseKind.transit]?.manifestId,
      );
    }
    final latest = await bucket.json('latest.json') as Map<String, Object?>;
    final routing = latest['routing'] as String?;
    if (routing == null) {
      throw StateError('latest.json names no routing release');
    }
    final routingRelease = await bucket.json(
      'routing/$routing/release.json',
    ) as Map<String, Object?>;
    final build = routingRelease['valhalla_build_id']! as int;
    final transit = latest['transit'] as String?;
    String? manifestId;
    if (transit != null) {
      final transitRelease = await bucket.json(
        'transit/$transit/release.json',
      ) as Map<String, Object?>;
      if (transitRelease['routing_release'] != routing ||
          transitRelease['valhalla_build_id'] != build) {
        throw StateError(
          'transit $transit is over routing '
          '${transitRelease['routing_release']} (build '
          '${transitRelease['valhalla_build_id']}), not routing $routing '
          '(build $build)',
        );
      }
      manifestId = transitRelease['manifest_id']! as String;
    }
    return PlanReleases(
      map: latest['map'] as String?,
      routing: routing,
      transit: transit,
      valhallaBuildId: build,
      mapEtag: null,
      pinned: false,
      manifestId: manifestId,
    );
  }

  /// The routing tiles of [polygon]: its level 0 cells' index shards (those the release
  /// has), filtered to tiles that intersect it.
  Future<List<PlannedFile>> routing(String release, GeoPolygon polygon) async {
    final info = await bucket.json(
      'routing/$release/release.json',
    ) as Map<String, Object?>;
    final cells = {
      for (final c in info['index_cells'] as List<Object?>) c! as int,
    };
    final files = <PlannedFile>[];
    for (final cell in ValhallaTiles.covering(0, polygon)) {
      if (!cells.contains(cell)) continue;
      final shard = await bucket.json(
        'routing/$release/index/$cell.json',
      ) as List<Object?>;
      for (final entry in shard.cast<Map<String, Object?>>()) {
        final path = entry['path']! as String;
        final tile = ValhallaTiles.parsePath(path);
        if (tile == null) continue;
        if (!polygon.intersects(ValhallaTiles.bounds(tile.$1, tile.$2))) {
          continue;
        }
        files.add(
          PlannedFile(
            kind: ReleaseKind.routing,
            release: release,
            path: path,
            installPath: 'routing/$release/$path',
            bytes: entry['bytes']! as int,
            sha256: entry['sha256']! as String,
          ),
        );
      }
    }
    return files;
  }

  /// The feeds with stops in [polygon], with what each needs there.
  Future<List<CandidateFeed>> feeds(String release, GeoPolygon polygon) async {
    final ids = <String>{};
    // Every transfer tile's size, by mode and tile, from the shards.
    final sizes = <(String, int, int), int>{};
    for (final (x, y) in SlippyTiles.covering(6, polygon)) {
      final Object? shard;
      try {
        shard = await bucket.json('transit/$release/index/$x/$y.json');
      } on BucketError catch (e) {
        // Zoom 6 tiles without stops have no shard.
        if (e.status == 404) continue;
        rethrow;
      }
      final document = shard! as Map<String, Object?>;
      final transfers = document['transfers'];
      if (transfers is! Map<String, Object?>) {
        throw StateError(
          'transit $release\'s index has no transfer tile sizes; it needs '
          'the feed index from valinor-transit-build with them',
        );
      }
      for (final MapEntry(key: mode, value: tiles) in transfers.entries) {
        for (final t in (tiles! as List<Object?>).cast<List<Object?>>()) {
          sizes[(mode, t[0]! as int, t[1]! as int)] = t[2]! as int;
        }
      }
      for (final feed in document['feeds']! as List<Object?>) {
        final f = feed! as Map<String, Object?>;
        final cells = (f['cells']! as List<Object?>).cast<List<Object?>>();
        if (cells.any(
          (c) => polygon.intersects(
            SlippyTiles.bounds(12, c[0]! as int, c[1]! as int),
          ),
        )) {
          ids.add(f['id']! as String);
        }
      }
    }
    final feeds = await Future.wait(
      ids.map((id) => _feed(release, id, polygon, sizes)),
    );
    return feeds..sort((a, b) => a.index.compareTo(b.index));
  }

  Future<CandidateFeed> _feed(
    String release,
    String id,
    GeoPolygon polygon,
    Map<(String, int, int), int> sizes,
  ) async {
    final summary = await bucket.json(
      'transit/$release/feeds/$id.json',
    ) as Map<String, Object?>;
    final entry = await bucket.json(
      'transit/$release/manifest/feeds/$id.json',
    ) as Map<String, Object?>;
    PlannedFile file(Map<String, Object?> ref) => PlannedFile(
      kind: ReleaseKind.transit,
      release: release,
      path: ref['path']! as String,
      installPath: transitInstallPath(release, ref['path']! as String),
      bytes: ref['bytes']! as int,
      sha256: ref['sha256']! as String,
    );
    final modes = (summary['modes'] as Map<String, Object?>? ?? const {})
        .cast<String, Map<String, Object?>>();
    final entryTiles =
        (entry['transfer_tiles'] as Map<String, Object?>? ?? const {})
            .cast<String, List<Object?>>();
    final tiles = <String, List<(int, int, int)>>{};
    for (final MapEntry(key: mode, value: list) in entryTiles.entries) {
      tiles[mode] = [
        for (final t in list.cast<List<Object?>>())
          if (polygon.intersects(
            SlippyTiles.bounds(12, t[0]! as int, t[1]! as int),
          ))
            (
              t[0]! as int,
              t[1]! as int,
              sizes[(mode, t[0]! as int, t[1]! as int)] ??
                  (throw StateError(
                    'transfer tile $mode ${t[0]}/${t[1]} of $id is in no shard',
                  )),
            ),
      ];
    }
    final manifestPath = 'manifest/feeds/$id.json';
    return CandidateFeed(
      id: id,
      index: summary['index']! as int,
      displayName: summary['display_name'] as String? ?? id,
      agencies: (summary['agencies'] as List<Object?>? ?? const [])
          .cast<String>(),
      firstServiceDate: summary['first_service_date'] as String?,
      lastServiceDate: summary['last_service_date'] as String?,
      files: [
        file(summary['timetable']! as Map<String, Object?>),
        for (final mode in modes.values)
          file(mode['snaps']! as Map<String, Object?>),
        PlannedFile(
          kind: ReleaseKind.transit,
          release: release,
          path: manifestPath,
          installPath: transitInstallPath(release, manifestPath),
        ),
      ],
      transferTiles: tiles,
    );
  }

  /// The map's tiles over [polygon] and their ranges, less those [basemap] has.
  /// Throws [BucketError] (`changed`) if the archive isn't the one whose ETag was pinned.
  Future<(MapPlanResult, Uint8List, String?)> map(
    String release,
    GeoPolygon polygon, {
    String? etag,
    String? basemap,
  }) async {
    final key = 'map/$release/planet.pmtiles';
    final first = await bucket.range(
      key,
      0,
      await native.mapFirstFetch(),
      ifMatch: etag,
    );
    final archiveEtag = first.etag ?? etag;
    final planning = await native.mapPlanner(first.bytes, polygon);
    for (var leaves = planning.neededLeaves(); leaves.isNotEmpty;) {
      for (final (start, length) in leaves) {
        final leaf = await bucket.range(
          key,
          start,
          length,
          ifMatch: archiveEtag,
        );
        await planning.addLeaf(start, leaf.bytes);
      }
      leaves = planning.neededLeaves();
    }
    return (
      await planning.finish(basemap),
      Uint8List.sublistView(first.bytes, 0, math.min(127, first.bytes.length)),
      archiveEtag,
    );
  }
}
