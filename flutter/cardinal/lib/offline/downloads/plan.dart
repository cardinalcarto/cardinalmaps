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
    this.archive,
  });

  final ReleaseKind kind;
  final String release;
  final String path;
  final String installPath;
  final int? bytes;
  final String? sha256;

  /// The archive it comes out of, in ranges, rather than an object of its own.
  final String? archive;

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
    required this.cellsInside,
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
  /// mode's archive directory.
  final Map<String, List<(int, int, int)>> transferTiles;

  /// How many of the zoom 12 cells its stops are in intersect the polygon: how much
  /// of it is there.
  final int cellsInside;

  /// Its own files' size: its timetable and its stops' snaps for every mode, without
  /// the transfer tiles it may share with other feeds.
  int get fileBytes => files.fold(0, (sum, f) => sum + (f.bytes ?? 0));

  /// The transfer tiles' size.
  int get transferBytes => transferTiles.values.fold(
    0,
    (sum, tiles) => tiles.fold(sum, (sum, t) => sum + t.$3),
  );

  /// What the feed costs alone: its files and its transfer tiles.
  int get bytes => fileBytes + transferBytes;

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
    this.transferArchives = const {},
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

  /// Each mode's transfer archive, read as far as the candidate feeds' tiles need.
  final Map<String, TransferArchive> transferArchives;

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
              transferPath(mode, transferArchives[mode]!.zoom, x, y),
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

/// Where a transfer tile is under its release (and, under the offline folder, on the
/// phone; the bucket holds them in [transferArchivePath]).
String transferPath(String mode, int zoom, int x, int y) =>
    'transfers/$mode/$zoom/$x/$y.bin.zst';

/// The map archive in the bucket, under `releases/`.
String mapArchivePath(String release) => 'map/$release/planet.pmtiles';

/// The zoom the transit index gives feeds' stop cells at (`cell_zoom` in its
/// `feeds.json`): coarser than transfer tiles, whose zoom comes from their archive.
const cellZoom = 12;

/// A mode's transfer archive in the bucket, under `releases/`.
String transferArchivePath(String release, String mode) =>
    'transit/$release/transfers/$mode.pmtiles';

/// A mode's transfer archive, read as far as a plan's tiles need.
class TransferArchive {
  TransferArchive({
    required this.path,
    required this.mode,
    required this.etag,
    required this.zoom,
    required this.planning,
  });

  /// Under `releases/`.
  final String path;
  final String mode;

  /// The zoom its tiles are at (the transit build's tile zoom).
  final int zoom;

  /// What its ranges must match (`If-Match`).
  final String etag;
  final ArchivePlanning planning;
}

/// Where a transit release's file goes under the offline folder: `snaps/` under
/// `transfers/`, as the manifest names them; everything else at its own path.
String transitInstallPath(String release, String path) =>
    path.startsWith('snaps/')
    ? 'transit/$release/transfers/$path'
    : 'transit/$release/$path';

/// The mode, zoom, and tile of a transfer tile's path under its release, or null.
(String, int, int, int)? parseTransferPath(String path) {
  final match = RegExp(r'^transfers/([a-z]+)/(\d+)/(\d+)/(\d+)\.bin\.zst$')
      .firstMatch(path);
  if (match == null) return null;
  return (
    match[1]!,
    int.parse(match[2]!),
    int.parse(match[3]!),
    int.parse(match[4]!),
  );
}

/// Reading what a plan needs from the bucket.
class Planner {
  Planner(this.bucket, this.native);

  final Bucket bucket;
  final OfflineNative native;

  /// The releases to plan with: [pinned] if there are any, else `latest.json`'s.
  /// Throws [StateError] if the transit release isn't over the routing release.
  Future<PlanReleases> releases(
    Map<ReleaseKind, ReleaseRow> pinned, {
    Map<String, String> etags = const {},
  }) async {
    if (pinned[ReleaseKind.routing] case final routing?) {
      return PlanReleases(
        map: pinned[ReleaseKind.map]?.date,
        routing: routing.date,
        transit: pinned[ReleaseKind.transit]?.date,
        valhallaBuildId: routing.valhallaBuildId!,
        mapEtag: switch (pinned[ReleaseKind.map]) {
          final map? => etags[mapArchivePath(map.date)],
          null => null,
        },
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
    // A map release pinned on its own (before any download) stays; the rest is latest.
    final pinnedMap = pinned[ReleaseKind.map]?.date;
    return PlanReleases(
      map: pinnedMap ?? latest['map'] as String?,
      routing: routing,
      transit: transit,
      valhallaBuildId: build,
      mapEtag: pinnedMap == null ? null : etags[mapArchivePath(pinnedMap)],
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

  /// The feeds with stops in [polygon], with what each needs there, and each mode's
  /// transfer archive, read as far as those feeds' tiles need ([etags] the pinned ones).
  Future<(List<CandidateFeed>, Map<String, TransferArchive>)> feeds(
    String release,
    GeoPolygon polygon, {
    Map<String, String> etags = const {},
  }) async {
    // By feed, its stop cells that intersect the polygon.
    final inside = <String, int>{};
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
      for (final feed in document['feeds']! as List<Object?>) {
        final f = feed! as Map<String, Object?>;
        final cells = (f['cells']! as List<Object?>).cast<List<Object?>>();
        final count = cells
            .where(
              (c) => polygon.intersects(
                SlippyTiles.bounds(cellZoom, c[0]! as int, c[1]! as int),
              ),
            )
            .length;
        if (count > 0) {
          inside.update(
            f['id']! as String,
            (n) => n + count,
            ifAbsent: () => count,
          );
        }
      }
    }
    final entries = await Future.wait(
      inside.keys.map((id) => _entry(release, id)),
    );
    // Each mode's archive gives the zoom its tiles are at; the feeds' tiles inside the
    // polygon at that zoom are sized by its directory.
    final modes = {
      for (final (_, entry) in entries)
        ...(entry['transfer_tiles'] as Map<String, Object?>? ?? const {}).keys,
    };
    final archives = <String, TransferArchive>{
      for (final mode in modes)
        mode: await transferArchive(
          release,
          mode,
          const [],
          etag: etags[transferArchivePath(release, mode)],
        ),
    };
    final zooms = {for (final a in archives.values) a.mode: a.zoom};
    final sizes = <(String, int, int), int>{};
    for (final archive in archives.values) {
      final wanted = {
        for (final (_, entry) in entries)
          for (final (x, y)
              in _inside(entry, polygon, zooms)[archive.mode] ??
                  const <(int, int)>[])
            (archive.zoom, x, y),
      }.toList();
      archive.planning.want(wanted);
      await _loadLeaves(archive);
      for (final found in archive.planning.found()) {
        sizes[(archive.mode, found.tile.$2, found.tile.$3)] = found.length;
      }
    }
    final feeds = [
      for (final (summary, entry) in entries)
        _feed(release, summary, entry, polygon, sizes, zooms, inside),
    ];
    return (feeds..sort((a, b) => a.index.compareTo(b.index)), archives);
  }

  /// A mode's transfer archive, its directory read as far as [tiles] need.
  /// Throws [BucketError] (`changed`) if it isn't the one whose ETag ([etag]) was pinned.
  Future<TransferArchive> transferArchive(
    String release,
    String mode,
    List<TileKey> tiles, {
    String? etag,
  }) async {
    final key = transferArchivePath(release, mode);
    final first = await bucket.range(
      key,
      0,
      await native.archiveFirstFetch(),
      ifMatch: etag,
    );
    final archiveEtag = first.etag ?? etag;
    if (archiveEtag == null) throw StateError('$key has no ETag');
    final planning = await native.archivePlanner(key, first.bytes, tiles);
    final archive = TransferArchive(
      path: key,
      mode: mode,
      etag: archiveEtag,
      zoom: planning.zoom,
      planning: planning,
    );
    await _loadLeaves(archive);
    return archive;
  }

  /// Fetches the leaf directories the archive's wanted tiles need.
  Future<void> _loadLeaves(TransferArchive archive) async {
    final planning = archive.planning;
    for (var leaves = planning.neededLeaves(); leaves.isNotEmpty;) {
      for (final (start, length) in leaves) {
        final leaf = await bucket.range(
          archive.path,
          start,
          length,
          ifMatch: archive.etag,
        );
        await planning.addLeaf(start, leaf.bytes);
      }
      leaves = planning.neededLeaves();
    }
  }

  Future<(Map<String, Object?>, Map<String, Object?>)> _entry(
    String release,
    String id,
  ) async => (
    await bucket.json('transit/$release/feeds/$id.json')
        as Map<String, Object?>,
    await bucket.json('transit/$release/manifest/feeds/$id.json')
        as Map<String, Object?>,
  );

  /// The feed's transfer tiles, by mode, that intersect [polygon], each mode's at the
  /// zoom its archive is ([zooms]).
  static Map<String, List<(int, int)>> _inside(
    Map<String, Object?> entry,
    GeoPolygon polygon,
    Map<String, int> zooms,
  ) => {
    for (final MapEntry(key: mode, value: list)
        in (entry['transfer_tiles'] as Map<String, Object?>? ?? const {})
            .cast<String, List<Object?>>()
            .entries)
      if (zooms[mode] case final zoom?)
        mode: [
          for (final t in list.cast<List<Object?>>())
            if (polygon.intersects(
              SlippyTiles.bounds(zoom, t[0]! as int, t[1]! as int),
            ))
              (t[0]! as int, t[1]! as int),
        ],
  };

  CandidateFeed _feed(
    String release,
    Map<String, Object?> summary,
    Map<String, Object?> entry,
    GeoPolygon polygon,
    Map<(String, int, int), int> sizes,
    Map<String, int> zooms,
    Map<String, int> inside,
  ) {
    final id = summary['id']! as String;
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
    final tiles = {
      for (final MapEntry(key: mode, value: list) in _inside(
        entry,
        polygon,
        zooms,
      ).entries)
        mode: [
          for (final (x, y) in list)
            (
              x,
              y,
              sizes[(mode, x, y)] ??
                  (throw StateError(
                    'transfer tile $mode $x/$y of $id isn\'t in its archive',
                  )),
            ),
        ],
    };
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
      cellsInside: inside[id]!,
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
    final key = mapArchivePath(release);
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
