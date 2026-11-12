import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:drift/drift.dart';
import 'package:flutter/foundation.dart';

import 'bucket.dart';
import 'database.dart';
import 'geometry.dart';
import 'native.dart';
import 'plan.dart';
import 'transfers.dart';

/// A region and how far along it is, for the screen that lists them.
@immutable
class RegionProgress {
  const RegionProgress({
    required this.id,
    required this.kind,
    required this.name,
    required this.polygon,
    required this.state,
    required this.error,
    required this.bytesDone,
    required this.bytesTotal,
    required this.filesDone,
    required this.filesTotal,
    required this.failed,
  });

  final int id;
  final RegionKind kind;
  final String? name;
  final GeoPolygon? polygon;
  final RegionState state;
  final String? error;

  /// Bytes of files verified or installed and of map ranges unpacked, of all the
  /// region's (files whose size isn't known yet count once they arrive).
  final int bytesDone;
  final int bytesTotal;

  /// Files and map ranges done, of all.
  final int filesDone;
  final int filesTotal;

  /// Files and ranges that failed too often (retry the region to try them again).
  final int failed;

  @override
  String toString() =>
      'RegionProgress($id, ${state.name}, $filesDone/$filesTotal, $bytesDone/$bytesTotal'
      '${error == null ? '' : ', $error'})';
}

/// The offline area's releases once a region is ready, for opening the router.
typedef ReadyReleases = ({
  String routing,
  String? transit,
  String? map,
  int valhallaBuildId,
});

/// Something holding open what downloads change: the router and transit planner, told
/// when files arrive or go.
abstract interface class OfflineListener {
  /// Files were installed or deleted. [feedsChanged]: the transit feeds did too.
  /// [filesRemoved]: files were deleted, which an open router keeps mapped (and their
  /// space taken) until it is dropped.
  Future<void> offlineChanged({
    required bool feedsChanged,
    bool filesRemoved = false,
  });
}

/// Thrown by [OfflineDownloads.start] when a plan no longer fits what's on the device.
class StalePlan implements Exception {
  const StalePlan(this.message);

  final String message;

  @override
  String toString() => 'StalePlan($message)';
}

/// Offline downloads: regions of routing tiles, transit feeds, and map tiles, planned
/// from the bucket's releases, downloaded in the background, checked, and installed
/// under [root]:
///
/// - `routing/<date>/tiles/…`, loose, as the router reads them;
/// - `transit/<date>/`: `timetables/`, `transfers/` (with `snaps/` and the
///   `manifest.json` assembled from the installed feeds' entries), `realtime.json`;
/// - `map/<date>/basemap.mbtiles`, one for every region;
/// - `.incoming/`, where downloads land until they're checked and renamed into place
///   (the router maps tiles, so nothing is written in place).
///
/// Files and map tiles are shared between regions, and deleting a region deletes only
/// what no other region needs. Every change of state goes through the database first,
/// so a restart picks up where the app stopped.
class OfflineDownloads {
  OfflineDownloads({
    required this.db,
    required this.root,
    required this.bucket,
    required this.transfers,
    this.native = const RustOfflineNative(),
    this.maxAttempts = 3,
    this.maxQueued = 200,
    DateTime Function()? clock,
  }) : _clock = clock ?? DateTime.now;

  final OfflineDatabase db;

  /// The offline folder.
  final String root;
  final Bucket bucket;
  final Transfers transfers;
  final OfflineNative native;

  /// How many times a file or range is tried before it counts as failed.
  final int maxAttempts;

  /// The most files and ranges handed to the downloader at once; more are handed over
  /// as those end. Keeps the platform's queue (WorkManager, a URL session) small for
  /// regions of thousands of files.
  final int maxQueued;
  final DateTime Function() _clock;

  final _listeners = <OfflineListener>{};

  /// Time spent in each step, as `(count, microseconds)`, for measuring on a device:
  /// `sha256`, `transfer tile check`, `json check`, `unpack`, `install`.
  final timings = <String, (int, int)>{};

  Future<T> _timed<T>(String step, Future<T> Function() work) async {
    final watch = Stopwatch()..start();
    try {
      return await work();
    } finally {
      final (count, micros) = timings[step] ?? (0, 0);
      timings[step] = (count + 1, micros + watch.elapsedMicroseconds);
    }
  }

  StreamSubscription<TransferResult>? _results;
  Future<void> _last = Future.value();

  String get incoming => '$root/.incoming';

  String basemapPath(String mapRelease) =>
      '$root/map/$mapRelease/basemap.mbtiles';

  /// Runs [work] after everything before it: the database and the files change one
  /// step at a time.
  Future<T> _serially<T>(Future<T> Function() work) {
    final run = _last.then((_) => work());
    _last = run.then<void>((_) {}, onError: (_) {});
    return run;
  }

  /// Completes once the work queued so far (and any it queued) is done.
  @visibleForTesting
  Future<void> get settled async {
    Future<void> seen;
    do {
      seen = _last;
      await seen;
    } while (!identical(seen, _last));
  }

  void addListener(OfflineListener listener) => _listeners.add(listener);

  void removeListener(OfflineListener listener) => _listeners.remove(listener);

  Future<void> _notify({
    required bool feedsChanged,
    bool filesRemoved = false,
  }) async {
    for (final listener in List.of(_listeners)) {
      try {
        await listener.offlineChanged(
          feedsChanged: feedsChanged,
          filesRemoved: filesRemoved,
        );
      } on Object catch (e) {
        debugPrint('offline: refreshing after a change failed: $e');
      }
    }
  }

  /// Starts handling downloads, and picks up where the last run stopped: deletions
  /// finish, and downloads that ended while the app wasn't running are handled.
  Future<void> open() async {
    _results ??= transfers.results.listen(
      (result) => unawaited(_serially(() => _handle(result))),
    );
    await transfers.start();
    await _serially(() async {
      await _recover(await transfers.active());
      for (final region in await (db.select(
        db.regions,
      )..where((r) => r.state.equalsValue(RegionState.deleting))).get()) {
        await _delete(region);
      }
      await _pump();
    });
  }

  /// Picks up work a previous run left between states. Files and ranges the
  /// downloader no longer has ([active] lacks them) are checked if they arrived, or
  /// queued again; checks and installs that were cut short run again.
  Future<void> _recover(Set<String> active) async {
    bool arrived(String id) => File(_incomingFile(id)).existsSync();
    for (final f
        in await (db.select(db.files)..where(
              (f) => f.state.isInValues([
                FileState.queued,
                FileState.downloaded,
                FileState.verified,
              ]),
            ))
            .get()) {
      final id = 'f${f.id}';
      switch (f.state) {
        case FileState.queued when active.contains(id):
          break;
        case FileState.queued || FileState.downloaded:
          if (arrived(id)) {
            // It finished unseen.
            await _setFile(f.id, FileState.downloaded);
            await _verify(f);
          } else {
            await _setFile(f.id, FileState.pending);
          }
        case FileState.verified when !arrived(id):
          // Renamed into place just before the app stopped, or lost.
          await _setFile(
            f.id,
            File('$root/${f.installPath}').existsSync()
                ? FileState.installed
                : FileState.pending,
          );
        default:
          break;
      }
    }
    for (final c
        in await (db.select(db.mapChunks)..where(
              (c) => c.state.isInValues([
                ChunkState.queued,
                ChunkState.downloaded,
                ChunkState.unpacked,
              ]),
            ))
            .get()) {
      final id = 'c${c.id}';
      switch (c.state) {
        case ChunkState.queued when active.contains(id):
          break;
        case ChunkState.queued || ChunkState.downloaded:
          if (arrived(id)) {
            await _setChunk(c.id, ChunkState.downloaded);
            await _unpack(c);
          } else {
            await _setChunk(c.id, ChunkState.pending);
          }
        case ChunkState.unpacked:
          _deleteQuietly(_incomingFile(id));
          await _setChunk(c.id, ChunkState.deleted);
        default:
          break;
      }
    }
    for (final region in await (db.select(
      db.regions,
    )..where((r) => r.state.equalsValue(RegionState.installing))).get()) {
      await _install(region, await _regionFiles(region.id));
    }
  }

  Future<void> close() async {
    await _results?.cancel();
    _results = null;
    await _last;
  }

  /// The pinned releases, by kind.
  Future<Map<ReleaseKind, ReleaseRow>> pinned() async => {
    for (final row in await db.select(db.releases).get()) row.kind: row,
  };

  /// The releases the offline area opens, once some region is ready.
  Future<ReadyReleases?> readyReleases() async {
    final ready =
        await (db.select(db.regions)
              ..where((r) => r.state.equalsValue(RegionState.ready))
              ..limit(1))
            .getSingleOrNull();
    if (ready == null) return null;
    final releases = await pinned();
    final routing = releases[ReleaseKind.routing];
    if (routing == null) return null;
    return (
      routing: routing.date,
      transit: releases[ReleaseKind.transit]?.date,
      map: releases[ReleaseKind.map]?.date,
      valhallaBuildId: routing.valhallaBuildId!,
    );
  }

  /// The basemap, if a map release is pinned and it exists.
  Future<String?> basemap() async {
    final map = (await pinned())[ReleaseKind.map];
    if (map == null) return null;
    final path = basemapPath(map.date);
    return File(path).existsSync() ? path : null;
  }

  // Planning.

  /// What downloading [polygon] takes: its routing tiles, the transit feeds with stops
  /// in it (for the user to choose), and its map ranges, with sizes. Reads the bucket
  /// (in the foreground: a few small requests per kind) and downloads nothing.
  Future<OfflinePlan> plan(GeoPolygon polygon) async {
    final planner = Planner(bucket, native);
    final releases = await planner.releases(await pinned());
    final routing = await planner.routing(releases.routing, polygon);
    final feeds = releases.transit == null
        ? const <CandidateFeed>[]
        : await planner.feeds(releases.transit!, polygon);
    MapPlanResult? map;
    Uint8List? header;
    var mapEtag = releases.mapEtag;
    if (releases.map case final release?) {
      final (result, first, etag) = await planner.map(
        release,
        polygon,
        etag: releases.mapEtag,
        basemap: basemapPath(release),
      );
      (map, header, mapEtag) = (result, first, etag);
    }
    final present = {
      for (final f
          in await (db.select(db.files)..where(
                (f) => f.state.isInValues([
                  FileState.verified,
                  FileState.installed,
                ]),
              ))
              .get())
        OfflinePlan.key(f.kind, f.release, f.path),
    };
    return OfflinePlan(
      polygon: polygon,
      releases: PlanReleases(
        map: releases.map,
        routing: releases.routing,
        transit: releases.transit,
        valhallaBuildId: releases.valhallaBuildId,
        mapEtag: mapEtag,
        pinned: releases.pinned,
        manifestId: releases.manifestId,
      ),
      routingFiles: routing,
      transitShared: [
        if (releases.transit case final t?)
          for (final path in ['manifest/shared.json', 'realtime.json'])
            PlannedFile(
              kind: ReleaseKind.transit,
              release: t,
              path: path,
              installPath: transitInstallPath(t, path),
            ),
      ],
      feeds: feeds,
      map: map,
      mapHeader: header,
      present: present,
    );
  }

  // Starting.

  /// Downloads [plan] with the feeds [feeds] (by ID) as a new region, and returns its
  /// ID. Pins the plan's releases if none are pinned. Throws [StalePlan] if the
  /// releases were pinned otherwise since, or the basemap lost tiles the plan counted on.
  Future<int> start(
    OfflinePlan plan, {
    Set<String> feeds = const {},
    String? name,
  }) => _serially(() async {
    final now = _clock();
    final r = plan.releases;
    final chosen = plan.feeds.where((f) => feeds.contains(f.id)).toList();
    final transferFiles = [
      for (final feed in chosen)
        for (final MapEntry(key: mode, value: tiles)
            in feed.transferTiles.entries)
          for (final (x, y, bytes) in tiles)
            PlannedFile(
              kind: ReleaseKind.transit,
              release: r.transit!,
              path: transferPath(mode, x, y),
              bytes: bytes,
              installPath: transitInstallPath(
                r.transit!,
                transferPath(mode, x, y),
              ),
            ),
    ];
    final files = {
      for (final f in [
        ...plan.routingFiles,
        if (chosen.isNotEmpty) ...plan.transitShared,
        for (final feed in chosen) ...feed.files,
        ...transferFiles,
      ])
        OfflinePlan.key(f.kind, f.release, f.path): f,
    };
    final regionId = await db.transaction(() async {
      final pinned = await this.pinned();
      if (pinned.isEmpty) {
        await db.batch((b) {
          b.insertAll(db.releases, [
            ReleasesCompanion.insert(
              kind: ReleaseKind.routing,
              date: r.routing,
              valhallaBuildId: Value(r.valhallaBuildId),
              pinnedAt: now,
            ),
            if (r.transit case final t?)
              ReleasesCompanion.insert(
                kind: ReleaseKind.transit,
                date: t,
                valhallaBuildId: Value(r.valhallaBuildId),
                manifestId: Value(r.manifestId),
                pinnedAt: now,
              ),
            if (r.map case final m?)
              ReleasesCompanion.insert(
                kind: ReleaseKind.map,
                date: m,
                etag: Value(r.mapEtag),
                pinnedAt: now,
              ),
          ]);
        });
      } else if (pinned[ReleaseKind.routing]?.date != r.routing ||
          pinned[ReleaseKind.transit]?.date != r.transit ||
          pinned[ReleaseKind.map]?.date != r.map ||
          (r.map != null && pinned[ReleaseKind.map]?.etag != r.mapEtag)) {
        throw const StalePlan('the pinned releases changed; plan again');
      }
      final id = await db
          .into(db.regions)
          .insert(
            RegionsCompanion.insert(
              kind: RegionKind.explicit,
              name: Value(name),
              polygon: Value(jsonEncode(plan.polygon.toJson())),
              mapRelease: Value(r.map),
              routingRelease: Value(r.routing),
              transitRelease: Value(chosen.isEmpty ? null : r.transit),
              state: RegionState.planned,
              mapTiles: Value(plan.map?.tileKeys),
              mapBytes: Value(plan.map?.rangeBytes ?? 0),
              createdAt: now,
              updatedAt: now,
            ),
          );
      for (final f in files.values) {
        await db
            .into(db.files)
            .insert(
              FilesCompanion.insert(
                kind: f.kind,
                release: f.release,
                path: f.path,
                installPath: f.installPath,
                bytes: Value(f.bytes),
                expectedSha256: Value(f.sha256),
                state: FileState.pending,
              ),
              mode: InsertMode.insertOrIgnore,
            );
      }
      // Link every file, including those other regions brought.
      for (final f in files.values) {
        final row =
            await (db.select(db.files)..where(
                  (x) =>
                      x.kind.equalsValue(f.kind) &
                      x.release.equals(f.release) &
                      x.path.equals(f.path),
                ))
                .getSingle();
        await db
            .into(db.regionFiles)
            .insert(RegionFilesCompanion.insert(regionId: id, fileId: row.id));
      }
      await db.batch((b) {
        b.insertAll(db.regionFeeds, [
          for (final feed in chosen)
            RegionFeedsCompanion.insert(
              regionId: id,
              feedId: feed.id,
              feedIndex: feed.index,
            ),
        ]);
        b.insertAll(db.mapChunks, [
          for (final range in plan.map?.ranges ?? const <MapRange>[])
            MapChunksCompanion.insert(
              regionId: id,
              start: range.start,
              length: range.length,
              tileCount: range.tileCount,
              tiles: range.tiles,
              state: ChunkState.pending,
            ),
        ]);
      });
      return id;
    });
    if (plan.map case final map? when r.map != null) {
      final basemap = basemapPath(r.map!);
      await native.basemapPrepare(
        basemap,
        plan.mapHeader!,
        'Cardinal offline map',
      );
      // Tiles other regions brought are this region's too from now on, so deleting
      // those regions keeps them.
      final missing = await native.basemapClaim(
        basemap,
        regionId,
        map.tileKeys,
      );
      final planned = map.archiveTiles - map.presentTiles;
      if (missing != planned) {
        await _setRegion(
          regionId,
          RegionState.failed,
          error:
              'the map has $missing tiles to download, not $planned; plan again',
        );
        throw const StalePlan('the basemap changed since planning; plan again');
      }
    }
    await _setRegion(regionId, RegionState.downloading);
    await _pump();
    return regionId;
  });

  Future<void> _setRegion(int id, RegionState state, {String? error}) async {
    await (db.update(db.regions)..where((r) => r.id.equals(id))).write(
      RegionsCompanion(
        state: Value(state),
        error: Value(error),
        updatedAt: Value(_clock()),
      ),
    );
    onRegionState?.call(id, state);
  }

  /// Told every region state written, in order, for tests.
  @visibleForTesting
  void Function(int region, RegionState state)? onRegionState;

  // Downloading.

  String _incomingFile(String id) => '$incoming/$id';

  Uri _fileUrl(FileRow f) =>
      bucket.url('${f.kind.name}/${f.release}/${f.path}');

  /// Hands pending work of regions that are downloading to the downloader, up to
  /// [maxQueued] at once, and fetches small JSON files itself. Runs serially.
  Future<void> _pump() async {
    final downloading = db.regions.state.equalsValue(RegionState.downloading);
    final queuedCount =
        (await (db.selectOnly(db.files)
              ..addColumns([db.files.id.count()])
              ..where(db.files.state.equalsValue(FileState.queued)))
            .map((row) => row.read(db.files.id.count()) ?? 0)
            .getSingle()) +
        (await (db.selectOnly(db.mapChunks)
              ..addColumns([db.mapChunks.id.count()])
              ..where(db.mapChunks.state.equalsValue(ChunkState.queued)))
            .map((row) => row.read(db.mapChunks.id.count()) ?? 0)
            .getSingle());
    var room = maxQueued - queuedCount;
    final pending =
        await (db.select(db.files).join([
                innerJoin(
                  db.regionFiles,
                  db.regionFiles.fileId.equalsExp(db.files.id),
                ),
                innerJoin(
                  db.regions,
                  db.regions.id.equalsExp(db.regionFiles.regionId),
                ),
              ])
              ..where(
                db.files.state.equalsValue(FileState.pending) & downloading,
              )
              ..groupBy([db.files.id])
              ..orderBy([OrderingTerm(expression: db.files.id)]))
            .map((row) => row.readTable(db.files))
            .get();
    final jobs = <TransferJob>[];
    for (final f in pending) {
      if (f.path.endsWith('.json')) {
        await _fetchForeground(f);
      } else if (room > 0) {
        room--;
        jobs.add(
          TransferJob(
            id: 'f${f.id}',
            url: _fileUrl(f),
            path: _incomingFile('f${f.id}'),
          ),
        );
      }
    }
    final chunks =
        await (db.select(db.mapChunks).join([
                innerJoin(
                  db.regions,
                  db.regions.id.equalsExp(db.mapChunks.regionId),
                ),
              ])
              ..where(
                db.mapChunks.state.equalsValue(ChunkState.pending) &
                    downloading,
              )
              ..orderBy([OrderingTerm(expression: db.mapChunks.id)])
              ..limit(room < 0 ? 0 : room))
            .get();
    final etag = (await pinned())[ReleaseKind.map]?.etag;
    for (final row in chunks) {
      final c = row.readTable(db.mapChunks);
      final region = row.readTable(db.regions);
      jobs.add(
        TransferJob(
          id: 'c${c.id}',
          url: bucket.url('map/${region.mapRelease}/planet.pmtiles'),
          path: _incomingFile('c${c.id}'),
          headers: {
            'Range': 'bytes=${c.start}-${c.start + c.length - 1}',
            'If-Match': ?etag,
          },
        ),
      );
    }
    if (jobs.isEmpty) {
      await _advanceAll();
      return;
    }
    await Directory(incoming).create(recursive: true);
    // Marked queued before handing them over, so a result can't arrive for work that's
    // still pending.
    await db.batch((b) {
      for (final job in jobs) {
        final id = int.parse(job.id.substring(1));
        if (job.id.startsWith('f')) {
          b.update(
            db.files,
            const FilesCompanion(state: Value(FileState.queued)),
            where: (f) => f.id.equals(id),
          );
        } else {
          b.update(
            db.mapChunks,
            const MapChunksCompanion(state: Value(ChunkState.queued)),
            where: (c) => c.id.equals(id),
          );
        }
      }
    });
    await transfers.enqueue(jobs);
    await _advanceAll();
  }

  /// Fetches a small JSON file over HTTPS (inflated): `downloaded`, then checked by
  /// reading it ([_checkJson]); the bucket publishes no hash of these.
  Future<void> _fetchForeground(FileRow f) async {
    final Object? json;
    try {
      json = await bucket.json('${f.kind.name}/${f.release}/${f.path}');
    } on BucketError catch (e) {
      await _failFile(f, '$e', permanent: e.status == 404);
      return;
    }
    final to = File(_incomingFile('f${f.id}'));
    await to.parent.create(recursive: true);
    await to.writeAsString(jsonEncode(json), flush: true);
    await _setFile(f.id, FileState.downloaded);
    await _verify(f);
  }

  /// What's wrong with a transit JSON file, or null: it must be the release's, and
  /// agree with what the region downloads with it.
  Future<String?> _checkJson(FileRow f, Object? json) async {
    if (json is! Map<String, Object?>) return 'not a JSON object';
    final transit = (await pinned())[ReleaseKind.transit];
    if (f.path == 'realtime.json') {
      return json['feeds'] is List ? null : 'no feeds';
    }
    if (f.path == 'manifest/shared.json') {
      if (transit?.manifestId case final id? when json['manifest_id'] != id) {
        return 'manifest ${json['manifest_id']}, not $id';
      }
      if (json['valhalla_build_id'] != transit?.valhallaBuildId) {
        return 'Valhalla build ${json['valhalla_build_id']}, '
            'not ${transit?.valhallaBuildId}';
      }
      return json['feed_count'] is int ? null : 'no feed_count';
    }
    final feed = RegExp(r'^manifest/feeds/(.+)\.json$').firstMatch(f.path)?[1];
    if (feed == null) return 'not a file this checks';
    if (json['id'] != feed) return 'the entry of ${json['id']}, not $feed';
    final index = await (db.select(
      db.regionFeeds,
    )..where((r) => r.feedId.equals(feed))).get();
    if (index.isNotEmpty && json['index'] != index.first.feedIndex) {
      return 'index ${json['index']}, not ${index.first.feedIndex}';
    }
    // Its files' hashes are those the summary gave, which they're checked against.
    Future<String?> expected(String path) async =>
        (await (db.select(db.files)..where(
                  (x) => x.release.equals(f.release) & x.path.equals(path),
                ))
                .getSingleOrNull())
            ?.expectedSha256;
    if (json['timetable'] case final String timetable) {
      final sha = await expected(timetable);
      if (sha != null && sha != json['timetable_sha256']) {
        return 'its timetable is $sha in the summary, '
            '${json['timetable_sha256']} here';
      }
    }
    final snaps = json['stop_snaps'];
    if (snaps is Map<String, Object?>) {
      for (final MapEntry(key: mode, value: snap) in snaps.entries) {
        if (snap case {'file': final String file, 'sha256': final String sha}) {
          final published = file.replaceFirst('transfers/', '');
          if (await expected(published) case final summary?
              when summary != sha) {
            return 'its $mode snaps are $summary in the summary, $sha here';
          }
        }
      }
    }
    return null;
  }

  Future<void> _handle(TransferResult result) async {
    final id = int.tryParse(result.id.substring(1));
    if (id == null) return;
    if (result.id.startsWith('f')) {
      final f = await (db.select(
        db.files,
      )..where((x) => x.id.equals(id))).getSingleOrNull();
      if (f == null || f.state != FileState.queued) return;
      switch (result.outcome) {
        case TransferOutcome.complete:
          if (result.statusCode case final status?
              when status != HttpStatus.ok) {
            await _failFile(f, 'status $status');
          } else {
            await _setFile(f.id, FileState.downloaded);
            await _verify(f);
          }
        case TransferOutcome.notFound:
          await _failFile(f, 'not in the bucket (404)', permanent: true);
        case TransferOutcome.failed:
          await _failFile(f, result.error ?? 'status ${result.statusCode}');
        case TransferOutcome.canceled:
          await _setFile(f.id, FileState.pending);
      }
    } else if (result.id.startsWith('c')) {
      final c = await (db.select(
        db.mapChunks,
      )..where((x) => x.id.equals(id))).getSingleOrNull();
      if (c == null || c.state != ChunkState.queued) return;
      switch (result.outcome) {
        case TransferOutcome.complete:
          if (result.statusCode case final status?
              when status != HttpStatus.partialContent) {
            await _failChunk(c, 'status $status, not a range');
          } else {
            await _setChunk(c.id, ChunkState.downloaded);
            await _unpack(c);
          }
        case TransferOutcome.failed || TransferOutcome.notFound:
          final changed = result.statusCode == HttpStatus.preconditionFailed;
          await _failChunk(
            c,
            changed
                ? 'the map archive changed since its release was pinned (412)'
                : result.error ?? 'status ${result.statusCode}',
            permanent: changed || result.outcome == TransferOutcome.notFound,
          );
        case TransferOutcome.canceled:
          await _setChunk(c.id, ChunkState.pending);
      }
    }
    await _pump();
  }

  Future<void> _setFile(int id, FileState state) => (db.update(
    db.files,
  )..where((f) => f.id.equals(id))).write(FilesCompanion(state: Value(state)));

  Future<void> _setChunk(int id, ChunkState state) =>
      (db.update(db.mapChunks)..where((c) => c.id.equals(id))).write(
        MapChunksCompanion(state: Value(state)),
      );

  /// A file is whole in `.incoming/` (`downloaded`): checks it, and it's `verified`.
  ///
  /// - Files the index gives a SHA-256 for (routing tiles, timetables, snaps) must
  ///   hash to it.
  /// - Transfer tiles must be the size their index shard gives, and decode whole: the
  ///   CRC-32 in their header covers every byte of the decompressed tile, and the header
  ///   must name the tile and the build's manifest (in Rust, `checkTransferTile`).
  /// - JSON must read as the release's ([_checkJson]).
  Future<void> _verify(FileRow f) async {
    final path = _incomingFile('f${f.id}');
    String? found;
    try {
      final length = File(path).lengthSync();
      if (f.bytes case final bytes? when length != bytes) {
        await _failFile(f, '$length bytes, not $bytes');
        return;
      }
      if (parseTransferPath(f.path) case (final mode, final x, final y)) {
        final manifest = (await pinned())[ReleaseKind.transit]?.manifestId;
        if (manifest == null) {
          await _failFile(f, 'no manifest ID to check it against');
          return;
        }
        final problem = await _timed(
          'transfer tile check',
          () => native.checkTransferTile(path, mode, x, y, manifest),
        );
        if (problem != null) {
          await _failFile(f, problem);
          return;
        }
      } else if (f.path.endsWith('.json')) {
        final problem = await _timed(
          'json check',
          () async =>
              _checkJson(f, jsonDecode(await File(path).readAsString())),
        );
        if (problem != null) {
          await _failFile(f, problem, permanent: true);
          return;
        }
        found = await native.sha256File(path);
      } else if (f.expectedSha256 case final expected?) {
        found = await _timed('sha256', () => native.sha256File(path));
        if (found != expected.toLowerCase()) {
          await _failFile(f, 'SHA-256 $found, not $expected');
          return;
        }
      } else {
        await _failFile(f, 'nothing to check it against', permanent: true);
        return;
      }
    } on Object catch (e) {
      await _failFile(f, 'unreadable: $e');
      return;
    }
    await (db.update(db.files)..where((x) => x.id.equals(f.id))).write(
      FilesCompanion(
        state: const Value(FileState.verified),
        // For JSON, the hash of what was stored, for the record; none is published.
        verifiedSha256: Value(found),
        error: const Value(null),
      ),
    );
  }

  /// Counts a failed attempt: the file goes back to `pending`, or after the last
  /// attempt (or a [permanent] failure, such as a 404) to `failed`.
  Future<void> _failFile(
    FileRow f,
    String error, {
    bool permanent = false,
  }) async {
    debugPrint('offline: ${f.path}: $error');
    _deleteQuietly(_incomingFile('f${f.id}'));
    final attempts = f.attempts + 1;
    await (db.update(db.files)..where((x) => x.id.equals(f.id))).write(
      FilesCompanion(
        state: Value(
          permanent || attempts >= maxAttempts
              ? FileState.failed
              : FileState.pending,
        ),
        attempts: Value(attempts),
        error: Value(error),
      ),
    );
  }

  Future<void> _failChunk(
    MapChunkRow c,
    String error, {
    bool permanent = false,
  }) async {
    debugPrint('offline: map range ${c.start}+${c.length}: $error');
    _deleteQuietly(_incomingFile('c${c.id}'));
    final attempts = c.attempts + 1;
    await (db.update(db.mapChunks)..where((x) => x.id.equals(c.id))).write(
      MapChunksCompanion(
        state: Value(
          permanent || attempts >= maxAttempts
              ? ChunkState.failed
              : ChunkState.pending,
        ),
        attempts: Value(attempts),
        error: Value(error),
      ),
    );
  }

  /// Writes a downloaded range's tiles into the basemap: `unpacked`, then its file goes:
  /// `deleted`. A bad tile or a short range fails it, to be downloaded again.
  Future<void> _unpack(MapChunkRow c) async {
    final region = await (db.select(
      db.regions,
    )..where((r) => r.id.equals(c.regionId))).getSingle();
    final file = _incomingFile('c${c.id}');
    try {
      await _timed(
        'unpack',
        () => native.basemapUnpack(
          basemapPath(region.mapRelease!),
          file,
          c.length,
          c.tiles,
          c.regionId,
        ),
      );
    } on MapRangeError catch (e) {
      await _failChunk(c, e.message);
      return;
    }
    await _setChunk(c.id, ChunkState.unpacked);
    _deleteQuietly(file);
    await _setChunk(c.id, ChunkState.deleted);
  }

  static void _deleteQuietly(String path) {
    try {
      File(path).deleteSync();
    } on FileSystemException {
      // Not there.
    }
  }

  // Installing.

  Future<void> _advanceAll() async {
    final regions = await (db.select(
      db.regions,
    )..where((r) => r.state.equalsValue(RegionState.downloading))).get();
    for (final region in regions) {
      await _advance(region);
    }
  }

  /// A downloading region whose files are all checked and ranges unpacked installs;
  /// one with a file or range that failed for good fails.
  Future<void> _advance(RegionRow region) async {
    final files = await _regionFiles(region.id);
    final chunks = await (db.select(
      db.mapChunks,
    )..where((c) => c.regionId.equals(region.id))).get();
    final failedFile = files
        .where((f) => f.state == FileState.failed)
        .firstOrNull;
    final failedChunk = chunks
        .where((c) => c.state == ChunkState.failed)
        .firstOrNull;
    if (failedFile != null || failedChunk != null) {
      await _setRegion(
        region.id,
        RegionState.failed,
        error: failedFile != null
            ? '${failedFile.path}: ${failedFile.error}'
            : 'map range ${failedChunk!.start}: ${failedChunk.error}',
      );
      return;
    }
    final done =
        files.every(
          (f) =>
              f.state == FileState.verified || f.state == FileState.installed,
        ) &&
        chunks.every((c) => c.state == ChunkState.deleted);
    if (!done) return;
    await _setRegion(region.id, RegionState.installing);
    await _timed('install', () => _install(region, files));
  }

  Future<List<FileRow>> _regionFiles(int region) =>
      (db.select(db.files).join([
            innerJoin(
              db.regionFiles,
              db.regionFiles.fileId.equalsExp(db.files.id),
            ),
          ])..where(db.regionFiles.regionId.equals(region)))
          .map((row) => row.readTable(db.files))
          .get();

  /// Renames the region's checked files into place (`installed`), assembles the transit
  /// manifest from every ready region's feeds, and tells listeners: `ready`.
  Future<void> _install(RegionRow region, List<FileRow> files) async {
    final now = _clock();
    for (final f in files.where((f) => f.state == FileState.verified)) {
      final to = File('$root/${f.installPath}');
      await to.parent.create(recursive: true);
      await File(_incomingFile('f${f.id}')).rename(to.path);
      await (db.update(db.files)..where((x) => x.id.equals(f.id))).write(
        FilesCompanion(
          state: const Value(FileState.installed),
          lastUsed: Value(now),
        ),
      );
    }
    final feeds = await (db.select(
      db.regionFeeds,
    )..where((f) => f.regionId.equals(region.id))).get();
    if (region.transitRelease case final release? when feeds.isNotEmpty) {
      await _assembleTransit(release, including: region.id);
    }
    await _setRegion(region.id, RegionState.ready);
    await _notify(feedsChanged: feeds.isNotEmpty);
  }

  /// Writes `transit/<release>/transfers/manifest.json`: the shared fields (less
  /// `feed_ids`) and, as `feeds`, the entries of every feed a ready region (or
  /// [including]) holds, by index. Removes the manifest when no
  /// feeds are left.
  Future<void> _assembleTransit(String release, {int? including}) async {
    final rows =
        await (db.select(db.regionFeeds).join([
              innerJoin(
                db.regions,
                db.regions.id.equalsExp(db.regionFeeds.regionId),
              ),
            ])..where(
              db.regions.transitRelease.equals(release) &
                  (db.regions.state.equalsValue(RegionState.ready) |
                      db.regions.id.equals(including ?? -1)),
            ))
            .map((row) => row.readTable(db.regionFeeds))
            .get();
    final feeds = {for (final f in rows) f.feedId: f.feedIndex};
    final manifest = File('$root/transit/$release/transfers/manifest.json');
    if (feeds.isEmpty) {
      _deleteQuietly(manifest.path);
      return;
    }
    Future<Map<String, Object?>> read(String path) async =>
        jsonDecode(await File('$root/$path').readAsString())
            as Map<String, Object?>;
    final shared =
        await read(transitInstallPath(release, 'manifest/shared.json'))
          ..remove('feed_ids');
    final ordered = feeds.entries.toList()
      ..sort((a, b) => a.value.compareTo(b.value));
    shared['feeds'] = [
      for (final f in ordered)
        await read(transitInstallPath(release, 'manifest/feeds/${f.key}.json')),
    ];
    await _writeAtomically(manifest, utf8.encode(jsonEncode(shared)));
  }

  Future<void> _writeAtomically(File to, List<int> bytes) async {
    await Directory(incoming).create(recursive: true);
    final part = File(
      '$incoming/${to.uri.pathSegments.last}.${_clock().microsecondsSinceEpoch}',
    );
    await part.writeAsBytes(bytes, flush: true);
    await to.parent.create(recursive: true);
    await part.rename(to.path);
  }

  // Retrying and deleting.

  /// Tries a failed region's failed files and ranges again.
  Future<void> retry(int region) => _serially(() async {
    final row = await (db.select(
      db.regions,
    )..where((r) => r.id.equals(region))).getSingleOrNull();
    if (row == null || row.state != RegionState.failed) return;
    final files = await _regionFiles(region);
    await db.batch((b) {
      for (final f in files.where((f) => f.state == FileState.failed)) {
        b.update(
          db.files,
          const FilesCompanion(
            state: Value(FileState.pending),
            attempts: Value(0),
          ),
          where: (x) => x.id.equals(f.id),
        );
      }
      b.update(
        db.mapChunks,
        const MapChunksCompanion(
          state: Value(ChunkState.pending),
          attempts: Value(0),
        ),
        where: (c) =>
            c.regionId.equals(region) & c.state.equalsValue(ChunkState.failed),
      );
    });
    await _setRegion(region, RegionState.downloading);
    await _pump();
  });

  /// Deletes a region, and the files and map tiles no other region needs.
  Future<void> delete(int region) => _serially(() async {
    final row = await (db.select(
      db.regions,
    )..where((r) => r.id.equals(region))).getSingleOrNull();
    if (row == null) return;
    await _setRegion(region, RegionState.deleting);
    await _delete(row);
  });

  Future<void> _delete(RegionRow region) async {
    final files = await _regionFiles(region.id);
    final others = {
      for (final row
          in await (db.select(db.regionFiles)..where(
                (x) =>
                    x.fileId.isIn(files.map((f) => f.id)) &
                    x.regionId.equals(region.id).not(),
              ))
              .get())
        row.fileId,
    };
    final orphans = files.where((f) => !others.contains(f.id)).toList();
    final chunks = await (db.select(
      db.mapChunks,
    )..where((c) => c.regionId.equals(region.id))).get();
    await transfers.cancel([
      for (final f in orphans)
        if (f.state == FileState.queued) 'f${f.id}',
      for (final c in chunks)
        if (c.state == ChunkState.queued) 'c${c.id}',
    ]);
    if (region.mapRelease case final map?) {
      await native.basemapRelease(basemapPath(map), region.id);
    }
    final hadFeeds = await (db.select(
      db.regionFeeds,
    )..where((f) => f.regionId.equals(region.id))).get();
    // The region leaves the manifest before its files go.
    await (db.delete(
      db.regionFeeds,
    )..where((f) => f.regionId.equals(region.id))).go();
    if (region.transitRelease case final release? when hadFeeds.isNotEmpty) {
      await _assembleTransit(release);
    }
    for (final f in orphans) {
      _deleteQuietly('$root/${f.installPath}');
      _deleteQuietly(_incomingFile('f${f.id}'));
    }
    for (final c in chunks) {
      _deleteQuietly(_incomingFile('c${c.id}'));
    }
    await db.transaction(() async {
      await (db.delete(
        db.files,
      )..where((f) => f.id.isIn(orphans.map((f) => f.id)))).go();
      await (db.delete(db.regions)..where((r) => r.id.equals(region.id))).go();
    });
    await _notify(feedsChanged: hadFeeds.isNotEmpty, filesRemoved: true);
  }

  // On-demand files.

  /// Deletes installed files that only on-demand regions hold, least recently used
  /// first, until [bytesToFree] bytes are freed or none are left; returns the bytes
  /// freed. Files any explicitly downloaded region holds are never evicted.
  ///
  /// An open router keeps a deleted tile mapped (and its space taken) until it's
  /// dropped (see `valinor-router`'s `deleted_tiles_stay_mapped_until_the_router_is_dropped`),
  /// so listeners are told files were removed, and the offline area reopens its router
  /// and transit planner once their running calls finish.
  Future<int> evictOnDemand(int bytesToFree) => _serially(() async {
    final explicit = db.alias(db.regions, 'explicit_region');
    final explicitRefs = db.alias(db.regionFiles, 'explicit_refs');
    final candidates =
        await (db.select(db.files)
              ..where(
                (f) =>
                    f.state.equalsValue(FileState.installed) &
                    existsQuery(
                      db.select(db.regionFiles)
                        ..where((x) => x.fileId.equalsExp(f.id)),
                    ) &
                    notExistsQuery(
                      db.select(explicitRefs).join([
                        innerJoin(
                          explicit,
                          explicit.id.equalsExp(explicitRefs.regionId),
                        ),
                      ])..where(
                        explicitRefs.fileId.equalsExp(f.id) &
                            explicit.kind.equalsValue(RegionKind.explicit),
                      ),
                    ),
              )
              ..orderBy([
                (f) => OrderingTerm(
                  expression: f.lastUsed,
                  nulls: NullsOrder.first,
                ),
                (f) => OrderingTerm(expression: f.id),
              ]))
            .get();
    var freed = 0;
    final evicted = <int>[];
    for (final f in candidates) {
      if (freed >= bytesToFree) break;
      final file = File('$root/${f.installPath}');
      final size = file.existsSync() ? file.lengthSync() : (f.bytes ?? 0);
      _deleteQuietly(file.path);
      freed += size;
      evicted.add(f.id);
    }
    if (evicted.isEmpty) return 0;
    await (db.delete(db.files)..where((f) => f.id.isIn(evicted))).go();
    await _notify(feedsChanged: false, filesRemoved: true);
    return freed;
  });

  /// Marks installed files (by path under the offline folder) as just used, for
  /// eviction's least-recently-used order. One statement, however many paths.
  Future<void> touch(Iterable<String> installPaths) async {
    final paths = installPaths.toSet();
    if (paths.isEmpty) return;
    await (db.update(db.files)..where((f) => f.installPath.isIn(paths))).write(
      FilesCompanion(lastUsed: Value(_clock())),
    );
  }

  /// Marks the routing tiles under a route's [shape] as just used. The router doesn't
  /// report the tiles its search read, so this takes those the route passes through
  /// (levels 0 to 2): the tiles a route needs again if it's asked for again.
  Future<void> touchRoute(List<GeoPoint> shape) async {
    final routing = (await pinned())[ReleaseKind.routing];
    if (routing == null || shape.isEmpty) return;
    final paths = <String>{};
    for (final p in shape) {
      for (var level = 0; level < 3; level++) {
        paths.add(
          'routing/${routing.date}/tiles/'
          '${ValhallaTiles.path(level, ValhallaTiles.idAt(level, p.lat, p.lng))}',
        );
      }
    }
    await touch(paths);
  }

  // Watching.

  /// Every region with its progress, as it changes.
  Stream<List<RegionProgress>> watch() {
    final query = db.customSelect(
      '''
      SELECT r.id, r.kind, r.name, r.polygon, r.state, r.error,
        COALESCE(f.done_bytes, 0) + COALESCE(c.done_bytes, 0) AS done_bytes,
        COALESCE(f.total_bytes, 0) + COALESCE(c.total_bytes, 0) AS total_bytes,
        COALESCE(f.done, 0) + COALESCE(c.done, 0) AS done,
        COALESCE(f.total, 0) + COALESCE(c.total, 0) AS total,
        COALESCE(f.failed, 0) + COALESCE(c.failed, 0) AS failed
      FROM regions r
      LEFT JOIN (
        SELECT rf.region_id,
          SUM(CASE WHEN files.state IN ('verified', 'installed')
            THEN COALESCE(files.bytes, 0) ELSE 0 END) AS done_bytes,
          SUM(COALESCE(files.bytes, 0)) AS total_bytes,
          SUM(files.state IN ('verified', 'installed')) AS done,
          COUNT(*) AS total,
          SUM(files.state = 'failed') AS failed
        FROM region_files rf JOIN files ON files.id = rf.file_id
        GROUP BY rf.region_id
      ) f ON f.region_id = r.id
      LEFT JOIN (
        SELECT region_id,
          SUM(CASE WHEN state IN ('unpacked', 'deleted') THEN length ELSE 0 END)
            AS done_bytes,
          SUM(length) AS total_bytes,
          SUM(state IN ('unpacked', 'deleted')) AS done,
          COUNT(*) AS total,
          SUM(state = 'failed') AS failed
        FROM map_chunks GROUP BY region_id
      ) c ON c.region_id = r.id
      ORDER BY r.id
      ''',
      readsFrom: {db.regions, db.files, db.regionFiles, db.mapChunks},
    );
    return query.watch().map(
      (rows) => [
        for (final row in rows)
          RegionProgress(
            id: row.read<int>('id'),
            kind: RegionKind.values.byName(row.read<String>('kind')),
            name: row.readNullable<String>('name'),
            polygon: switch (row.readNullable<String>('polygon')) {
              final json? => GeoPolygon.fromJson(
                jsonDecode(json) as List<Object?>,
              ),
              null => null,
            },
            state: RegionState.values.byName(row.read<String>('state')),
            error: row.readNullable<String>('error'),
            bytesDone: row.read<int>('done_bytes'),
            bytesTotal: row.read<int>('total_bytes'),
            filesDone: row.read<int>('done'),
            filesTotal: row.read<int>('total'),
            failed: row.read<int>('failed'),
          ),
      ],
    );
  }

  /// Every region with its progress, now.
  Future<List<RegionProgress>> regions() => watch().first;
}
