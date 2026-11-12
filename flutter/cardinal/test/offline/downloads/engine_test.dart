// The download engine's states and transitions, over a small fake release served
// locally, with fakes for the background downloader and the Rust side.
import 'dart:convert';
import 'dart:io';

import 'package:cardinal/offline/downloads/bucket.dart';
import 'package:cardinal/offline/downloads/database.dart';
import 'package:cardinal/offline/downloads/engine.dart';
import 'package:cardinal/offline/downloads/geometry.dart';
import 'package:cardinal/offline/downloads/plan.dart';
import 'package:drift/drift.dart' hide isNull, isNotNull;
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_bucket.dart';
import 'fake_release.dart';
import 'fakes.dart';

const metro = 'us-wa-Metro-Transit';
const kitsap = 'us-wa-Kitsap-Transit';

late Directory tmp;
late FakeRelease release;
late FakeBucket server;
late OfflineDatabase db;
late FakeTransfers transfers;
late FakeNative native;
late OfflineDownloads engine;
late RecordingListener listener;
late String root;

OfflineDownloads newEngine() => OfflineDownloads(
  db: db,
  root: root,
  bucket: Bucket(Uri.parse(server.releasesUrl)),
  transfers: transfers,
  native: native,
);

/// Lets downloads and their handling run until nothing is left to do.
Future<void> settle() async {
  for (var i = 0; i < 100; i++) {
    await transfers.drain();
    for (var j = 0; j < 5; j++) {
      await Future<void>.delayed(Duration.zero);
    }
    await engine.settled;
    if (transfers.jobs.isEmpty || transfers.hold) {
      await Future<void>.delayed(Duration.zero);
      await engine.settled;
      return;
    }
  }
}

Future<RegionProgress> region(int id) async =>
    (await engine.regions()).singleWhere((r) => r.id == id);

Future<List<FileRow>> files({FileState? state}) =>
    (db.select(db.files)..where(
          (f) =>
              state == null ? const Constant(true) : f.state.equalsValue(state),
        ))
        .get();

Future<int> download(GeoPolygon polygon, {Set<String> feeds = const {}}) async {
  final plan = await engine.plan(polygon);
  final id = await engine.start(plan, feeds: feeds);
  await settle();
  return id;
}

void main() {
  setUp(() async {
    tmp = await Directory.systemTemp.createTemp('offline_engine_test');
    release = FakeRelease('${tmp.path}/bucket');
    await release.write();
    server = await FakeBucket.serve(release.root);
    db = OfflineDatabase(NativeDatabase.memory());
    transfers = FakeTransfers();
    native =
        FakeNative(
            '${release.root}/releases/map/$mapDate/planet.pmtiles',
            '${release.root}/releases',
          )
          ..archives = release.archives
          ..transferTiles = release.transferTileBytes;
    root = '${tmp.path}/offline';
    engine = newEngine();
    listener = RecordingListener();
    engine.addListener(listener);
    await engine.open();
  });

  tearDown(() async {
    await engine.close();
    transfers.close();
    await server.close();
    await db.close();
    await tmp.delete(recursive: true);
  });

  test(
    'a plan has the polygon\'s tiles, feeds and map ranges, and sizes',
    () async {
      final plan = await engine.plan(downtown);
      expect(plan.releases.pinned, isFalse);
      expect(plan.releases.valhallaBuildId, buildId);
      final paths = plan.routingFiles.map((f) => f.path).toSet();
      expect(paths, contains('tiles/0/003/074.gph'));
      expect(paths, contains('tiles/1/049/377.gph'));
      // Only tiles that intersect it.
      for (final f in plan.routingFiles) {
        final (level, id) = ValhallaTiles.parsePath(f.path)!;
        expect(downtown.intersects(ValhallaTiles.bounds(level, id)), isTrue);
        expect(f.bytes, isNotNull);
        expect(f.sha256, hasLength(64));
      }
      expect(paths.length, lessThan(release.routingTiles.length));
      expect(plan.feeds.map((f) => f.id), [metro]);
      final feed = plan.feeds.single;
      expect(feed.index, 2140);
      expect(feed.files.map((f) => f.installPath), [
        'transit/$transitDate/timetables/$metro.timetable',
        'transit/$transitDate/transfers/snaps/walking/2140.bin',
        'transit/$transitDate/transfers/snaps/cycling/2140.bin',
        'transit/$transitDate/manifest/feeds/$metro.json',
      ]);
      // Transfer tiles in the polygon, from the feed's manifest entry.
      for (final MapEntry(key: mode, value: tiles)
          in feed.transferTiles.entries) {
        expect(tiles, isNotEmpty);
        for (final (x, y, bytes) in tiles) {
          // Sized by the archive's directory.
          expect(bytes, release.transferTileBytes[(mode, x, y)]!.length);
          expect(downtown.intersects(SlippyTiles.bounds(12, x, y)), isTrue);
        }
      }
      expect(feed.transferBytes, greaterThan(0));
      expect(plan.map!.ranges, isNotEmpty);

      final none = plan.totals({});
      final all = plan.totals({metro});
      expect(none.transitBytes, 0);
      expect(all.transitBytes, greaterThan(feed.files.first.bytes!));
      // Exact: the files and the tiles, each once.
      final tileBytes = {
        for (final MapEntry(key: mode, value: tiles)
            in feed.transferTiles.entries)
          for (final (x, y, bytes) in tiles) (mode, x, y): bytes,
      }.values.fold(0, (a, b) => a + b);
      expect(
        all.transitBytes,
        feed.files.fold(0, (sum, f) => sum + (f.bytes ?? 0)) + tileBytes,
      );
      expect(
        none.routingBytes,
        plan.routingFiles.fold(0, (sum, f) => sum + f.bytes!),
      );
      expect(none.mapBytes, plan.map!.rangeBytes);
      expect(all.downloadBytes, none.downloadBytes + all.transitBytes);
      // Nothing downloaded or pinned yet.
      expect(await engine.pinned(), isEmpty);
      expect(await engine.regions(), isEmpty);
      expect(transfers.history, isEmpty);
    },
  );

  test(
    'a region goes from planned to downloading to installing to ready',
    () async {
      final plan = await engine.plan(downtown);
      RegionState? atClaim;
      native.onClaim = (id) async {
        final row = await (db.select(
          db.regions,
        )..where((r) => r.id.equals(id))).getSingle();
        atClaim = row.state;
      };
      final transitions = <RegionState>[];
      engine.onRegionState = (_, state) => transitions.add(state);
      // Files are hashed once downloaded, and ranges unpacked once downloaded.
      final hashed = <FileState>{}, unpacked = <ChunkState>{};
      native.onHash = (path) async {
        final id = int.parse(path.split('/').last.substring(1));
        hashed.add(
          (await (db.select(
            db.files,
          )..where((f) => f.id.equals(id))).getSingle()).state,
        );
      };
      native.onUnpack = (path) async {
        final id = int.parse(path.split('/').last.substring(1));
        unpacked.add(
          (await (db.select(
            db.chunks,
          )..where((c) => c.id.equals(id))).getSingle()).state,
        );
      };
      // Held, so the downloading state can be seen.
      transfers.hold = true;
      final id = await engine.start(plan, feeds: {metro});
      expect(atClaim, RegionState.planned);
      expect((await region(id)).state, RegionState.downloading);
      expect(await files(state: FileState.queued), isNotEmpty);
      // Small JSON is fetched in the foreground and checked straight away.
      final verified = await files(state: FileState.verified);
      expect(verified.map((f) => f.path).toSet(), {
        'manifest/shared.json',
        'realtime.json',
        'manifest/feeds/$metro.json',
      });
      final states = <RegionState>[];
      final watching = engine.watch().listen(
        (regions) => states.add(regions.single.state),
      );
      transfers.hold = false;
      transfers.run();
      await settle();
      await watching.cancel();
      expect(
        states,
        containsAllInOrder([RegionState.downloading, RegionState.ready]),
      );

      expect(transitions, [
        RegionState.downloading,
        RegionState.installing,
        RegionState.ready,
      ]);
      expect(hashed, {FileState.downloaded});
      expect(unpacked, {ChunkState.downloaded});
      final progress = await region(id);
      expect(progress.state, RegionState.ready);
      expect(progress.filesDone, progress.filesTotal);
      expect(progress.bytesDone, progress.bytesTotal);
      expect(progress.failed, 0);
      expect(progress.polygon!.points, downtown.points);
      // Every file installed where it belongs, with its published bytes.
      for (final f in await files()) {
        expect(f.state, FileState.installed, reason: f.path);
        final stored = File(
          '${release.root}/releases/${f.kind.name}/${f.release}/${f.path}',
        );
        final installed = File('$root/${f.installPath}');
        if (f.path.endsWith('.json')) {
          // Checked by reading it; stored as JSON, as the bucket serves it inflated.
          expect(f.expectedSha256, isNull);
          expect(
            jsonDecode(await installed.readAsString()),
            jsonDecode(utf8.decode(gzip.decode(await stored.readAsBytes()))),
          );
        } else if (f.archive != null) {
          // Out of a range of its archive, loose as the build wrote it.
          final (mode, x, y) = parseTransferPath(f.path)!;
          expect(
            await installed.readAsBytes(),
            release.transferTileBytes[(mode, x, y)],
          );
        } else {
          if (f.expectedSha256 != null) {
            expect(f.verifiedSha256, f.expectedSha256);
          }
          expect(await installed.readAsBytes(), await stored.readAsBytes());
        }
      }
      expect(Directory('$root/.incoming').listSync(), isEmpty);
      // Transfer tiles have their sizes from the archive, no SHA-256, and were checked
      // by decoding them as their ranges were unpacked; none was fetched on its own.
      final tiles = (await files()).where(
        (f) => f.path.startsWith('transfers/'),
      );
      expect(tiles, isNotEmpty);
      expect(
        tiles.every(
          (f) =>
              f.bytes != null &&
              f.expectedSha256 == null &&
              f.archive ==
                  transferArchivePath(transitDate, f.path.split('/')[1]),
        ),
        isTrue,
      );
      expect(native.checkedTiles.length, tiles.length);
      expect(server.heads, isEmpty);
      expect(
        server.requestsFor(
          (p) => p.contains('/transfers/') && p.endsWith('.zst'),
        ),
        0,
      );
      final transferRanges = (await db.select(db.chunks).get()).where(
        (c) => c.kind == ChunkKind.transfers,
      );
      expect(transferRanges, isNotEmpty);
      expect(
        transferRanges.every((c) => c.state == ChunkState.deleted),
        isTrue,
      );
      // Map ranges unpacked and deleted.
      final chunks = await db.select(db.chunks).get();
      expect(chunks, isNotEmpty);
      expect(chunks.every((c) => c.state == ChunkState.deleted), isTrue);
      final basemap = engine.basemapPath(mapDate);
      expect(await engine.basemap(), basemap);
      expect(
        native.basemaps[basemap]!.values.every((r) => r.contains(id)),
        isTrue,
      );
      // The releases are pinned.
      final pinned = await engine.pinned();
      expect(pinned[ReleaseKind.routing]!.date, routingDate);
      expect(pinned[ReleaseKind.routing]!.valhallaBuildId, buildId);
      expect((await engine.archiveEtags())[mapArchivePath(mapDate)], isNotNull);
      // The transit manifest: shared fields without feed IDs, and the region's feed.
      final manifest = jsonDecode(
        await File('$root/transit/$transitDate/transfers/manifest.json')
            .readAsString(),
      ) as Map<String, Object?>;
      expect(manifest['feed_count'], 2170);
      expect(manifest.containsKey('feed_ids'), isFalse);
      expect((manifest['feeds']! as List).map((f) => (f as Map)['id']), [
        metro,
      ]);
      expect(
        jsonDecode(
          await File('$root/transit/$transitDate/realtime.json').readAsString(),
        ),
        containsPair('feeds', isNotEmpty),
      );
      expect(listener.changes, [true]);
      expect(await engine.readyReleases(), (
        routing: routingDate,
        transit: transitDate,
        map: mapDate,
        valhallaBuildId: buildId,
      ));
    },
  );

  test('a corrupt file fails its check, is retried, and fails the region; retry downloads it again', () async {
    final plan = await engine.plan(downtown);
    final tile = plan.routingFiles.first.path;
    server.corrupt.add('routing/$routingDate/$tile');
    final id = await engine.start(plan);
    await settle();
    final row = (await files()).singleWhere((f) => f.path == tile);
    expect(row.state, FileState.failed);
    expect(row.attempts, engine.maxAttempts);
    expect(row.error, startsWith('SHA-256'));
    expect(row.verifiedSha256, isNull);
    final failed = await region(id);
    expect(failed.state, RegionState.failed);
    expect(failed.error, contains(tile));
    expect(failed.failed, 1);
    // Not installed, and nothing left behind.
    expect(File('$root/${row.installPath}').existsSync(), isFalse);
    expect(
      transfers.history.where((j) => j.id == 'f${row.id}'),
      hasLength(engine.maxAttempts),
    );

    final transitions = <RegionState>[];
    engine.onRegionState = (_, state) => transitions.add(state);
    server.corrupt.clear();
    await engine.retry(id);
    expect(
      (await region(id)).state,
      isIn([RegionState.downloading, RegionState.ready]),
    );
    await settle();
    expect((await region(id)).state, RegionState.ready);
    expect(transitions, [
      RegionState.downloading,
      RegionState.installing,
      RegionState.ready,
    ]);
    final fixed = (await files()).singleWhere((f) => f.path == tile);
    expect(fixed.state, FileState.installed);
    expect(fixed.verifiedSha256, plan.routingFiles.first.sha256);
  });

  test(
    'a file corrupt once is downloaded again and the region still gets ready',
    () async {
      final plan = await engine.plan(downtown);
      final tile = plan.routingFiles.last.path;
      server.corrupt.add('routing/$routingDate/$tile');
      transfers.hold = true;
      final id = await engine.start(plan);
      final row = (await files()).singleWhere((f) => f.path == tile);
      transfers.run(['f${row.id}']);
      await settle();
      final retried = (await files()).singleWhere((f) => f.path == tile);
      expect(retried.attempts, 1);
      expect(retried.state, FileState.queued);
      server.corrupt.clear();
      transfers.hold = false;
      transfers.run();
      await settle();
      expect((await region(id)).state, RegionState.ready);
    },
  );

  test('an interrupted download is retried', () async {
    final plan = await engine.plan(downtown);
    final tile = plan.routingFiles.first.path;
    server.cutOnce.add('routing/$routingDate/$tile');
    final id = await engine.start(plan);
    await settle();
    expect((await region(id)).state, RegionState.ready);
    final row = (await files()).singleWhere((f) => f.path == tile);
    expect(row.attempts, 1);
    expect(row.state, FileState.installed);
  });

  test('a bad map tile fails its range, which is downloaded again', () async {
    const archive = 'map/$mapDate/planet.pmtiles';
    final plan = await engine.plan(downtown);
    transfers.hold = true;
    final id = await engine.start(plan);
    final chunk = (await db.select(db.chunks).get()).first;
    server.corruptAt[archive] = chunk.start + 5;
    transfers.run(['c${chunk.id}']);
    await settle();
    var after = await (db.select(
      db.chunks,
    )..where((c) => c.id.equals(chunk.id))).getSingle();
    expect(after.attempts, 1);
    expect(after.error, contains('corrupt'));
    // Queued again.
    expect(after.state, ChunkState.queued);
    expect(native.basemaps[engine.basemapPath(mapDate)], isEmpty);
    server.corruptAt.clear();
    transfers.hold = false;
    transfers.run();
    await settle();
    expect((await region(id)).state, RegionState.ready);
    after = await (db.select(
      db.chunks,
    )..where((c) => c.id.equals(chunk.id))).getSingle();
    expect(after.state, ChunkState.deleted);

    // A range that's always bad fails the region.
    final second = await engine.plan(bainbridge);
    server.corruptAt[archive] = second.map!.ranges.first.start + 5;
    final id2 = await engine.start(second);
    await settle();
    final bad =
        await (db.select(db.chunks)..where(
              (c) =>
                  c.regionId.equals(id2) &
                  c.start.equals(second.map!.ranges.first.start),
            ))
            .getSingle();
    expect(bad.state, ChunkState.failed);
    expect(bad.attempts, engine.maxAttempts);
    final failed = await region(id2);
    expect(failed.state, RegionState.failed);
    expect(failed.error, contains('map range'));
  });

  test('a map archive that changed since it was pinned answers 412, which fails the region for good', () async {
    final plan = await engine.plan(downtown);
    server.etagOverride = '"replaced"';
    final id = await engine.start(plan);
    await settle();
    final chunks = await db.select(db.chunks).get();
    expect(
      chunks.every((c) => c.state == ChunkState.failed && c.attempts == 1),
      isTrue,
    );
    expect(chunks.first.error, contains('412'));
    final failed = await region(id);
    expect(failed.state, RegionState.failed);
    expect(failed.error, contains('412'));
    // The other files downloaded.
    expect((await files(state: FileState.verified)), isNotEmpty);
  });

  test('a file the bucket lacks fails at once', () async {
    final plan = await engine.plan(downtown);
    final tile = plan.routingFiles.first;
    File('${release.root}/releases/routing/$routingDate/${tile.path}')
        .deleteSync();
    final id = await engine.start(plan);
    await settle();
    final row = (await files()).singleWhere((f) => f.path == tile.path);
    expect(row.state, FileState.failed);
    expect(row.attempts, 1);
    expect((await region(id)).state, RegionState.failed);
  });

  test('a bad transfer tile fails its range, which is downloaded again, and a 412 fails it for good', () async {
    final plan = await engine.plan(downtown);
    final (mode, x, y, _) = plan.feeds.single.transferTiles.entries
        .map((e) => (e.key, e.value.first.$1, e.value.first.$2, 0))
        .first;
    final archive = transferArchivePath(transitDate, mode);
    final (start, _) = release.archives[archive]![(12, x, y)]!;
    // A byte of the tile changed once: the range fails its check and comes again.
    server.corruptOnceAt[archive] = start + 3;
    final id = await engine.start(plan, feeds: {metro});
    await settle();
    expect((await region(id)).state, RegionState.ready);
    final ranges = (await db.select(db.chunks).get())
        .where((c) => c.archive == archive)
        .toList();
    final retried = ranges.where((c) => c.attempts == 1).single;
    expect(retried.error, contains('checksum mismatch'));
    expect(retried.state, ChunkState.deleted);
    final row = (await files()).singleWhere(
      (f) => f.path == transferPath(mode, x, y),
    );
    expect(row.state, FileState.installed);

    // Another region's ranges of the archive, after it "changed": 412, for good.
    final second = await engine.plan(bainbridge);
    server.etagOverride = '"replaced"';
    final id2 = await engine.start(second, feeds: {kitsap});
    await settle();
    final failed = await (db.select(
      db.chunks,
    )..where((c) => c.regionId.equals(id2))).get();
    final transfers = failed.where((c) => c.kind == ChunkKind.transfers);
    expect(transfers, isNotEmpty);
    expect(
      transfers.every((c) => c.state == ChunkState.failed && c.attempts == 1),
      isTrue,
    );
    expect(transfers.first.error, contains('412'));
    expect((await region(id2)).state, RegionState.failed);
  });

  test(
    'a manifest entry that disagrees with the feed summary fails for good',
    () async {
      final plan = await engine.plan(downtown);
      final key = 'transit/$transitDate/manifest/feeds/$metro.json';
      final entry = jsonDecode(
        utf8.decode(
          gzip.decode(File('${release.root}/releases/$key').readAsBytesSync()),
        ),
      ) as Map<String, Object?>;
      await release.putJson(key, {...entry, 'index': 7});
      final id = await engine.start(plan, feeds: {metro});
      await settle();
      final row = (await files()).singleWhere(
        (f) => f.path == 'manifest/feeds/$metro.json',
      );
      expect(row.state, FileState.failed);
      expect(row.attempts, 1);
      expect(row.error, 'index 7, not 2140');
      final failed = await region(id);
      expect(failed.state, RegionState.failed);
      expect(failed.error, contains('index 7'));
    },
  );

  test(
    'no more than maxQueued files and ranges are with the downloader at once',
    () async {
      await engine.close();
      engine = OfflineDownloads(
        db: db,
        root: root,
        bucket: Bucket(Uri.parse(server.releasesUrl)),
        transfers: transfers,
        native: native,
        maxQueued: 4,
      );
      await engine.open();
      final plan = await engine.plan(capitolHill);
      transfers.hold = true;
      final id = await engine.start(plan, feeds: {metro});
      final total =
          (await files())
              .where((f) => !f.path.endsWith('.json') && f.archive == null)
              .length +
          (await db.select(db.chunks).get()).length;
      expect(total, greaterThan(4 * 3));
      var most = 0;
      var rounds = 0;
      while (transfers.jobs.isNotEmpty) {
        most = transfers.jobs.length > most ? transfers.jobs.length : most;
        expect(
          (await files(state: FileState.queued)).length +
              (await (db.select(db.chunks)
                        ..where((c) => c.state.equalsValue(ChunkState.queued)))
                      .get())
                  .length,
          transfers.jobs.length,
        );
        // Halfway, the app stops and the downloader keeps what it has.
        if (rounds == 2) {
          await engine.close();
          engine = OfflineDownloads(
            db: db,
            root: root,
            bucket: Bucket(Uri.parse(server.releasesUrl)),
            transfers: transfers,
            native: native,
            maxQueued: 4,
          );
          await engine.open();
          expect(transfers.jobs.length, lessThanOrEqualTo(4));
        }
        transfers.run([transfers.jobs.keys.first]);
        await settle();
        rounds++;
      }
      expect(most, 4);
      expect(transfers.history.length, total);
      expect((await region(id)).state, RegionState.ready);
    },
  );

  test('overlapping regions share files and tiles, and deleting one keeps what the other needs', () async {
    final first = await download(capitolHill, feeds: {metro});
    final firstFiles = {for (final f in await files()) f.path};
    final installed = {for (final f in await files()) f.path: f.installPath};
    final plan = await engine.plan(downtown);
    // What the first brought isn't downloaded again.
    final shared = plan.routingFiles.where((f) => firstFiles.contains(f.path));
    expect(shared, isNotEmpty);
    expect(
      plan.totals({metro}).routingBytes,
      plan.routingFiles
          .where((f) => !firstFiles.contains(f.path))
          .fold(0, (sum, f) => sum + f.bytes!),
    );
    expect(plan.map!.presentTiles, greaterThan(0));
    final before = transfers.history.length;
    final second = await engine.start(plan, feeds: {metro});
    await settle();
    expect((await region(second)).state, RegionState.ready);
    final again = transfers.history.skip(before).map((j) => j.url.path);
    for (final f in shared) {
      expect(again.any((p) => p.endsWith(f.path)), isFalse, reason: f.path);
    }
    final basemap = native.basemaps[engine.basemapPath(mapDate)]!;
    final both = basemap.values.where((r) => r.containsAll([first, second]));
    expect(both, isNotEmpty);

    await engine.delete(first);
    expect((await engine.regions()).map((r) => r.id), [second]);
    final left = await files();
    for (final f in left) {
      expect(
        File('$root/${f.installPath}').existsSync(),
        isTrue,
        reason: f.path,
      );
    }
    // Shared files stayed; the first region's own went.
    for (final f in shared) {
      expect(left.any((x) => x.path == f.path), isTrue);
    }
    final onlyFirst = firstFiles.difference({for (final f in left) f.path});
    expect(onlyFirst, isNotEmpty);
    for (final path in onlyFirst) {
      expect(File('$root/${installed[path]}').existsSync(), isFalse);
    }
    expect(
      basemap.values.every((r) => r.length == 1 && r.contains(second)),
      isTrue,
    );
    // The manifest still lists Metro, for the second region.
    final manifest = jsonDecode(
      await File('$root/transit/$transitDate/transfers/manifest.json')
          .readAsString(),
    ) as Map<String, Object?>;
    expect((manifest['feeds']! as List).map((f) => (f as Map)['id']), [metro]);

    await engine.delete(second);
    expect(await engine.regions(), isEmpty);
    expect(await files(), isEmpty);
    expect(basemap, isEmpty);
    expect(
      File('$root/transit/$transitDate/transfers/manifest.json').existsSync(),
      isFalse,
    );
    expect(listener.changes.length, 4);
    // Installs add files; deletions remove them.
    expect(listener.removals, [false, false, true, true]);
  });

  test('feeds of two regions are in the manifest by index', () async {
    await download(downtown, feeds: {metro});
    await download(bainbridge, feeds: {kitsap});
    final manifest = jsonDecode(
      await File('$root/transit/$transitDate/transfers/manifest.json')
          .readAsString(),
    ) as Map<String, Object?>;
    final feeds = (manifest['feeds']! as List).cast<Map<String, Object?>>();
    expect(feeds.map((f) => f['id']), [kitsap, metro]);
    expect(feeds.map((f) => f['index']), [2137, 2140]);
  });

  test('deleting a region that is downloading cancels its downloads', () async {
    final plan = await engine.plan(downtown);
    transfers.hold = true;
    final id = await engine.start(plan);
    expect(transfers.jobs, isNotEmpty);
    final transitions = <RegionState>[];
    engine.onRegionState = (_, state) => transitions.add(state);
    await engine.delete(id);
    expect(transitions, [RegionState.deleting]);
    await settle();
    expect(transfers.jobs, isEmpty);
    expect(await engine.regions(), isEmpty);
    expect(await files(), isEmpty);
    expect(await db.select(db.chunks).get(), isEmpty);
  });

  test(
    'a restart queues again what the downloader lost, and finishes a deletion',
    () async {
      final plan = await engine.plan(downtown);
      transfers.hold = true;
      final id = await engine.start(plan);
      final queued = (await files(state: FileState.queued)).length;
      expect(queued, greaterThan(0));
      // The app ends; the downloader lost its jobs.
      await engine.close();
      transfers = FakeTransfers();
      engine = newEngine();
      await engine.open();
      expect(
        transfers.history.length,
        queued + (await db.select(db.chunks).get()).length,
      );
      await settle();
      expect((await region(id)).state, RegionState.ready);

      // A deletion the app didn't finish.
      await (db.update(db.regions)..where((r) => r.id.equals(id))).write(
        const RegionsCompanion(state: Value(RegionState.deleting)),
      );
      await engine.close();
      engine = newEngine();
      await engine.open();
      expect(await engine.regions(), isEmpty);
      expect(await files(), isEmpty);
    },
  );

  test('a restart checks what arrived while the app was away, without downloading it again', () async {
    final plan = await engine.plan(downtown);
    transfers.silent = true;
    final id = await engine.start(plan, feeds: {metro});
    await settle();
    expect((await region(id)).state, RegionState.downloading);
    expect(transfers.jobs, isEmpty);
    await engine.close();
    transfers = FakeTransfers();
    engine = newEngine();
    await engine.open();
    await settle();
    expect(transfers.history, isEmpty);
    expect((await region(id)).state, RegionState.ready);
    // Transfer tiles' SHA-256s came from the bucket, asked for.
    final tiles = (await files()).where((f) => f.path.startsWith('transfers/'));
    expect(tiles, isNotEmpty);
    // Checked from what arrived: no requests to the bucket for them.
    expect(server.heads, isEmpty);
    expect(native.checkedTiles.length, tiles.length);
  });

  test('a restart finishes unpacking and installing', () async {
    final id = await download(downtown, feeds: {metro});
    final chunk = (await db.select(db.chunks).get()).first;
    final file = (await files()).first;
    // As if the app stopped after a range was unpacked, and after a file was renamed
    // into place but before its state was written.
    await (db.update(db.chunks)..where((c) => c.id.equals(chunk.id))).write(
      const ChunksCompanion(state: Value(ChunkState.unpacked)),
    );
    await File('$root/.incoming/c${chunk.id}').writeAsString('left over');
    await (db.update(db.files)..where((f) => f.id.equals(file.id))).write(
      const FilesCompanion(state: Value(FileState.verified)),
    );
    await (db.update(db.regions)..where((r) => r.id.equals(id))).write(
      const RegionsCompanion(state: Value(RegionState.installing)),
    );
    await engine.close();
    engine = newEngine();
    await engine.open();
    await settle();
    expect((await region(id)).state, RegionState.ready);
    expect(
      (await (db.select(
        db.chunks,
      )..where((c) => c.id.equals(chunk.id))).getSingle()).state,
      ChunkState.deleted,
    );
    expect(File('$root/.incoming/c${chunk.id}').existsSync(), isFalse);
    expect(
      (await (db.select(
        db.files,
      )..where((f) => f.id.equals(file.id))).getSingle()).state,
      FileState.installed,
    );
  });

  test('later downloads use the pinned releases, not latest', () async {
    await download(downtown);
    await release.publishOtherTransit('2026-12-01');
    final plan = await engine.plan(capitolHill);
    expect(plan.releases.pinned, isTrue);
    expect(plan.releases.transit, transitDate);
  });

  test(
    'a transit release that is not over the routing release is refused',
    () async {
      await release.publishOtherTransit('2026-12-01');
      await expectLater(engine.plan(downtown), throwsStateError);
    },
  );

  test(
    'a plan made before the releases were pinned otherwise is stale',
    () async {
      final plan = await engine.plan(downtown);
      await db
          .into(db.releases)
          .insert(
            ReleasesCompanion.insert(
              kind: ReleaseKind.routing,
              date: '2026-10-01',
              valhallaBuildId: const Value(1),
              pinnedAt: DateTime(2026),
            ),
          );
      await expectLater(engine.start(plan), throwsA(isA<StalePlan>()));
    },
  );

  group('on-demand files', () {
    /// An on-demand region holding installed files [paths], used at [used] (by index).
    Future<int> onDemand(List<String> paths, List<DateTime?> used) async {
      final id = await db
          .into(db.regions)
          .insert(
            RegionsCompanion.insert(
              kind: RegionKind.onDemand,
              routingRelease: const Value(routingDate),
              state: RegionState.ready,
              createdAt: DateTime(2026),
              updatedAt: DateTime(2026),
            ),
          );
      for (final (i, path) in paths.indexed) {
        final file = File('$root/routing/$routingDate/$path');
        await file.parent.create(recursive: true);
        await file.writeAsBytes(List.filled(1000, i));
        final row = await (db.select(
          db.files,
        )..where((f) => f.path.equals(path))).getSingleOrNull();
        final fileId =
            row?.id ??
            await db
                .into(db.files)
                .insert(
                  FilesCompanion.insert(
                    kind: ReleaseKind.routing,
                    release: routingDate,
                    path: path,
                    installPath: 'routing/$routingDate/$path',
                    bytes: const Value(1000),
                    state: FileState.installed,
                    lastUsed: Value(used[i]),
                  ),
                );
        await db
            .into(db.regionFiles)
            .insert(
              RegionFilesCompanion.insert(regionId: id, fileId: fileId),
              mode: InsertMode.insertOrIgnore,
            );
      }
      return id;
    }

    test('only files no explicit region holds are evicted, least recently used first', () async {
      await download(downtown);
      final explicit = (await files()).first.path;
      await onDemand(
        [
          'tiles/2/000/000/001.gph',
          'tiles/2/000/000/002.gph',
          'tiles/2/000/000/003.gph',
          explicit,
        ],
        [DateTime(2026, 3), DateTime(2026, 1), null, DateTime(2025)],
      );
      final changes = listener.changes.length;
      // The never-used file, then the oldest; the explicit region's file, though older,
      // stays.
      expect(await engine.evictOnDemand(1500), 2000);
      var left = {for (final f in await files()) f.path};
      expect(left, isNot(contains('tiles/2/000/000/003.gph')));
      expect(left, isNot(contains('tiles/2/000/000/002.gph')));
      expect(left, contains('tiles/2/000/000/001.gph'));
      expect(
        File('$root/routing/$routingDate/tiles/2/000/000/002.gph').existsSync(),
        isFalse,
      );
      expect(listener.changes.length, changes + 1);
      expect(listener.removals.last, isTrue);

      expect(await engine.evictOnDemand(1 << 30), 1000);
      left = {for (final f in await files()) f.path};
      expect(left, contains(explicit));
      expect(File('$root/routing/$routingDate/$explicit').existsSync(), isTrue);
      expect(await engine.evictOnDemand(1000), 0);
    });

    test('touching a route marks its tiles used', () async {
      final path =
          'tiles/${ValhallaTiles.path(2, ValhallaTiles.idAt(2, 47.61, -122.33))}';
      final level1 =
          'tiles/${ValhallaTiles.path(1, ValhallaTiles.idAt(1, 47.61, -122.33))}';
      await db
          .into(db.releases)
          .insert(
            ReleasesCompanion.insert(
              kind: ReleaseKind.routing,
              date: routingDate,
              valhallaBuildId: const Value(buildId),
              pinnedAt: DateTime(2026),
            ),
          );
      await onDemand(
        [path, level1, 'tiles/2/000/000/009.gph'],
        [null, null, null],
      );
      await engine.touchRoute(const [
        GeoPoint(47.61, -122.33),
        GeoPoint(47.611, -122.331),
      ]);
      final used = {for (final f in await files()) f.path: f.lastUsed};
      expect(used[path], isNotNull);
      expect(used[level1], isNotNull);
      expect(used['tiles/2/000/000/009.gph'], isNull);
      // The unused one goes first.
      await engine.evictOnDemand(1);
      expect({for (final f in await files()) f.path}, {path, level1});
    });
  });

  test('geometry: tile paths and polygons', () {
    expect(ValhallaTiles.path(0, 3074), '0/003/074.gph');
    expect(ValhallaTiles.path(1, 49377), '1/049/377.gph');
    expect(ValhallaTiles.path(2, 792230), '2/000/792/230.gph');
    expect(ValhallaTiles.parsePath('tiles/2/000/792/230.gph'), (2, 792230));
    expect(ValhallaTiles.idAt(2, 47.6062, -122.3321), 792230);
    expect(ValhallaTiles.covering(0, downtown), [3074]);
    final triangle = GeoPolygon(const [
      GeoPoint(0, 0),
      GeoPoint(0, 1),
      GeoPoint(1, 0),
    ]);
    expect(
      triangle.intersects((west: 0.8, south: 0.8, east: 0.9, north: 0.9)),
      isFalse,
    );
    expect(
      triangle.intersects((west: 0.4, south: 0.4, east: 0.6, north: 0.6)),
      isTrue,
    );
    expect(
      () => GeoPolygon(const [GeoPoint(0, 0), GeoPoint(1, 1)]),
      throwsArgumentError,
    );
    expect(SlippyTiles.at(12, 47.6062, -122.3321), (656, 1430));
    expect(
      transferPath('walking', 656, 1430),
      'transfers/walking/12/656/1430.bin.zst',
    );
  });
}
