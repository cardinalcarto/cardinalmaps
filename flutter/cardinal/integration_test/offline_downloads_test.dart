// Offline downloads end to end on the desktop: the Seattle test release, served
// locally, downloaded by background_downloader, checked and unpacked by the Rust
// libraries, then routed and planned over.
//
// Needs a release published with `cardinal-publish --target dir:` (routing, map, and
// transit, Seattle-sized) and a folder to download into:
//
//   flutter test integration_test/offline_downloads_test.dart -d linux \
//     --dart-define=CARDINAL_TEST_BUCKET=/path/to/bucket \
//     --dart-define=CARDINAL_TEST_ROOT=/path/to/scratch
//
// It leaves the second region in CARDINAL_TEST_ROOT/offline, so the basemap can be
// checked with sparkle's reader (`offline_map`'s `reads_a_downloaded_basemap`).
import 'dart:convert';
import 'dart:io';

import 'package:cardinal/offline/downloads/bucket.dart';
import 'package:cardinal/offline/downloads/database.dart';
import 'package:cardinal/offline/downloads/engine.dart';
import 'package:cardinal/offline/downloads/geometry.dart';
import 'package:cardinal/offline/downloads/transfers.dart';
import 'package:cardinal/offline/offline_area.dart';
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:routing/routing.dart';

import '../test/offline/downloads/fake_bucket.dart';

const bucketDir = String.fromEnvironment('CARDINAL_TEST_BUCKET');
const scratch = String.fromEnvironment('CARDINAL_TEST_ROOT');

const metro = 'us-wa-Metro-Transit';
const soundTransit = 'us-wa-Sound-Transit';

/// Westlake to Capitol Hill: downtown and Cal Anderson Park.
final downtown = GeoPolygon(const [
  GeoPoint(47.600, -122.350),
  GeoPoint(47.600, -122.300),
  GeoPoint(47.630, -122.300),
  GeoPoint(47.630, -122.350),
]);

/// North of it, overlapping, and across into the next level 2 tile east.
final north = GeoPolygon(const [
  GeoPoint(47.620, -122.360),
  GeoPoint(47.620, -122.240),
  GeoPoint(47.680, -122.240),
  GeoPoint(47.680, -122.360),
]);

const westlake = (lat: 47.6114, lon: -122.3375);
const calAnderson = (lat: 47.6170378, lon: -122.3191692);
const greenLake = (lat: 47.6795, lon: -122.3260);

/// Across the lake, in the next level 2 tile east, which only the second region has.
const kirkland = (lat: 47.6769, lon: -122.2060);

Future<RegionProgress> waitFor(
  OfflineDownloads engine,
  int id,
  Set<RegionState> states,
) => engine
    .watch()
    .map((all) => all.where((r) => r.id == id).firstOrNull)
    .firstWhere((r) => r != null && states.contains(r.state))
    .then((r) => r!)
    .timeout(const Duration(minutes: 4));

String route(({double lat, double lon}) from, ({double lat, double lon}) to) =>
    jsonEncode({
      'format': 'osrm',
      'costing': 'pedestrian',
      'locations': [
        {'lat': from.lat, 'lon': from.lon},
        {'lat': to.lat, 'lon': to.lon},
      ],
    });

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('downloads Seattle, routes and plans over it, and survives faults', (
    tester,
  ) async {
    expect(bucketDir, isNotEmpty, reason: 'needs CARDINAL_TEST_BUCKET');
    expect(scratch, isNotEmpty, reason: 'needs CARDINAL_TEST_ROOT');
    await tester.runAsync(() async {
      final root = '$scratch/offline';
      if (Directory(root).existsSync()) {
        await Directory(root).delete(recursive: true);
      }
      await Directory(root).create(recursive: true);
      final server = await FakeBucket.serve(bucketDir);
      final db = OfflineDatabase(NativeDatabase(File('$root/offline.sqlite')));
      final transfers = BackgroundTransfers();
      final engine = OfflineDownloads(
        db: db,
        root: root,
        bucket: Bucket(Uri.parse(server.releasesUrl)),
        transfers: transfers,
      );
      await engine.open();
      final log = <String>[];

      // Plan.
      final watch = Stopwatch()..start();
      final plan = await engine.plan(downtown);
      final totals = plan.totals({metro, soundTransit});
      log.add(
        'plan: ${watch.elapsedMilliseconds} ms; ${plan.routingFiles.length} routing '
        'tiles (${totals.routingBytes} B), feeds '
        '${plan.feeds.map((f) => f.id).join(', ')}, transit ${totals.transitBytes} B '
        '(estimated: ${totals.transferBytesEstimated}), map ${plan.map!.archiveTiles} '
        'tiles in ${plan.map!.ranges.length} ranges: ${plan.map!.rangeBytes} B for '
        '${plan.map!.tileBytes} B of tiles; ${totals.files} files, '
        '${totals.downloadBytes} B in all',
      );
      expect(plan.feeds.map((f) => f.id), containsAll([metro, soundTransit]));
      expect(plan.routingFiles.map((f) => f.path), [
        'tiles/0/003/074.gph',
        'tiles/1/049/377.gph',
        'tiles/2/000/792/230.gph',
      ]);

      // Faults: a level 2 tile always corrupt (until cleared), a level 1 tile cut
      // off once, and one map tile corrupt once.
      server.corrupt.add(
        'routing/${plan.releases.routing}/tiles/2/000/792/230.gph',
      );
      server.cutOnce.add(
        'routing/${plan.releases.routing}/tiles/1/049/377.gph',
      );
      final firstRange = plan.map!.ranges.first;
      final archive = 'map/${plan.releases.map}/planet.pmtiles';
      server.corruptOnceAt[archive] = firstRange.start + 20;

      watch.reset();
      final a = await engine.start(plan, feeds: {metro, soundTransit});
      var progress = await waitFor(engine, a, {
        RegionState.failed,
        RegionState.ready,
      });
      log.add(
        'region a: ${progress.state.name} after ${watch.elapsedMilliseconds} ms: ${progress.error}',
      );
      expect(progress.state, RegionState.failed);
      expect(progress.error, contains('792/230.gph'));
      final fileRows = await db.select(db.files).get();
      final bad = fileRows.singleWhere((f) => f.path.endsWith('792/230.gph'));
      expect(bad.state, FileState.failed);
      expect(bad.attempts, engine.maxAttempts);
      final cut = fileRows.singleWhere((f) => f.path.endsWith('049/377.gph'));
      log.add(
        'cut-off tile: ${cut.state.name}, ${cut.attempts} failed attempts, ${cut.error}',
      );
      expect(cut.state, isIn([FileState.verified, FileState.installed]));
      final chunks = await db.select(db.mapChunks).get();
      final first = chunks.singleWhere((c) => c.start == firstRange.start);
      log.add(
        'corrupt map range: ${first.state.name}, ${first.attempts} failed attempts, ${first.error}',
      );
      expect(first.attempts, 1);
      expect(first.error, contains('gzip'));
      expect(first.state, ChunkState.deleted);

      server.corrupt.clear();
      watch.reset();
      await engine.retry(a);
      progress = await waitFor(engine, a, {
        RegionState.failed,
        RegionState.ready,
      });
      log.add(
        'region a after retry: ${progress.state.name} in ${watch.elapsedMilliseconds} ms, $progress',
      );
      expect(progress.state, RegionState.ready);
      expect(progress.failed, 0);

      // Route and plan over it, opened as the app opens them.
      final releases = (await engine.readyReleases())!;
      final area = OfflineArea(
        routing: '$root/routing/${releases.routing}',
        transitBuild: '$root/transit/${releases.transit}',
        valhallaBuildId: releases.valhallaBuildId,
      );
      engine.addListener(area);
      final router = await area.router;
      final walk = await router.route(
        body: utf8.encode(route(westlake, calAnderson)),
      );
      expect(walk.status, 200, reason: utf8.decode(walk.body));
      expect(utf8.decode(walk.body), contains('"code":"Ok"'));
      final transit = await area.transit;
      final cancel = TransitCancel();
      final trip = await transit.plan(
        request: utf8.encode(
          jsonEncode({
            'origin': {'lat': westlake.lat, 'lon': westlake.lon},
            'destination': {'lat': calAnderson.lat, 'lon': calAnderson.lon},
            'mode': 'walking',
          }),
        ),
        feeds: const [],
        cancel: cancel,
      );
      final body = jsonDecode(utf8.decode(trip.body)) as Map<String, Object?>;
      log.add(
        'transit plan: ${trip.status}, ${(body['journeys'] as List?)?.length} journeys, ${body['no_journeys_reason']}',
      );
      expect(trip.status, 200, reason: utf8.decode(trip.body));
      expect(body['journeys'], isNotEmpty);
      // Kirkland's level 2 tile isn't here yet.
      expect(await area.covers(kirkland.lat, kirkland.lon), isFalse);
      final across = await router.route(
        body: utf8.encode(route(greenLake, kirkland)),
      );
      expect(across.status, isNot(200));

      // A second, overlapping region, whose ranges get 412 because the archive
      // "changed" after planning.
      final planB = await engine.plan(north);
      log.add(
        'plan b: ${planB.routingFiles.length} routing tiles, '
        '${planB.present.length} files already here, map ${planB.map!.presentTiles} '
        'of ${planB.map!.archiveTiles} tiles here, ${planB.map!.ranges.length} ranges, '
        'download ${planB.totals({metro}).downloadBytes} B',
      );
      expect(planB.map!.presentTiles, greaterThan(0));
      server.etagOverride = '"replaced"';
      final b = await engine.start(planB, feeds: {metro});
      progress = await waitFor(engine, b, {
        RegionState.failed,
        RegionState.ready,
      });
      log.add('region b: ${progress.state.name}: ${progress.error}');
      expect(progress.state, RegionState.failed);
      expect(progress.error, contains('412'));
      // Planning again says so too.
      await expectLater(
        engine.plan(north),
        throwsA(isA<BucketError>().having((e) => e.changed, 'changed', isTrue)),
      );
      server.etagOverride = null;
      await engine.retry(b);
      progress = await waitFor(engine, b, {
        RegionState.failed,
        RegionState.ready,
      });
      expect(progress.state, RegionState.ready, reason: progress.error);

      // Routes reach the new tiles once the router refreshes (the engine told it).
      final northWalk = await router.route(
        body: utf8.encode(route(greenLake, kirkland)),
      );
      expect(northWalk.status, 200, reason: utf8.decode(northWalk.body));

      // Deleting the first region keeps what the second needs.
      final before = {for (final f in await db.select(db.files).get()) f.id: f};
      final aFiles = {
        for (final r in await (db.select(
          db.regionFiles,
        )..where((x) => x.regionId.equals(a))).get())
          r.fileId,
      };
      final bFiles = {
        for (final r in await (db.select(
          db.regionFiles,
        )..where((x) => x.regionId.equals(b))).get())
          r.fileId,
      };
      await engine.delete(a);
      final after = {for (final f in await db.select(db.files).get()) f.id: f};
      expect(after.keys.toSet(), bFiles);
      for (final id in aFiles.difference(bFiles)) {
        expect(File('$root/${before[id]!.installPath}').existsSync(), isFalse);
      }
      for (final f in after.values) {
        expect(
          File('$root/${f.installPath}').existsSync(),
          isTrue,
          reason: f.path,
        );
      }
      log.add(
        'deleted a: ${aFiles.difference(bFiles).length} of its ${aFiles.length} '
        'files went, ${aFiles.intersection(bFiles).length} stayed for b',
      );
      // The router still has (and maps) what b needs.
      final again = await router.route(
        body: utf8.encode(route(greenLake, kirkland)),
      );
      expect(again.status, 200);
      final reopened = await OfflineRouter.open(
        tileSets: ['${area.routing}/tiles'],
        expectedBuildId: releases.valhallaBuildId,
      );
      expect(
        (await reopened.route(body: utf8.encode(route(greenLake, kirkland))))
            .status,
        200,
      );

      await File('$scratch/e2e.log').writeAsString('${log.join('\n')}\n');
      await File('$scratch/basemap.path')
          .writeAsString(engine.basemapPath(releases.map!));
      await engine.close();
      await db.close();
      await server.close();
    });
  }, timeout: const Timeout(Duration(minutes: 10)));
}
