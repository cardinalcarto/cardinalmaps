// Offline downloads end to end on a phone, in profile mode: the Seattle test release,
// served from the host (`test/offline/downloads/serve_fake_bucket.dart`) through
// `adb reverse`, downloaded by background_downloader, checked and installed, then
// routed and planned over. It reports its timings as the driver's response data.
//
//   flutter drive --profile -d DEVICE --driver=test_driver/integration_test.dart \
//     --target=integration_test/offline_downloads_phone_test.dart \
//     --dart-define=CARDINAL_TEST_BUCKET_URL=http://127.0.0.1:PORT/releases \
//     --dart-define=CARDINAL_TEST_ROOT=/storage/emulated/0/Android/data/APP/files/NAME
//
// With CARDINAL_TEST_BACKGROUND=true it prints `E2E-MARK downloading` once the download
// starts and waits for it alone, so the app can be sent to the background meanwhile.
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

const bucketUrl = String.fromEnvironment('CARDINAL_TEST_BUCKET_URL');
const scratch = String.fromEnvironment('CARDINAL_TEST_ROOT');
const background = bool.fromEnvironment('CARDINAL_TEST_BACKGROUND');

const metro = 'us-wa-Metro-Transit';
const soundTransit = 'us-wa-Sound-Transit';

final downtown = GeoPolygon(const [
  GeoPoint(47.600, -122.350),
  GeoPoint(47.600, -122.300),
  GeoPoint(47.630, -122.300),
  GeoPoint(47.630, -122.350),
]);

final north = GeoPolygon(const [
  GeoPoint(47.620, -122.360),
  GeoPoint(47.620, -122.240),
  GeoPoint(47.680, -122.240),
  GeoPoint(47.680, -122.360),
]);

const westlake = (lat: 47.6114, lon: -122.3375);
const calAnderson = (lat: 47.6170378, lon: -122.3191692);
const greenLake = (lat: 47.6795, lon: -122.3260);
const kirkland = (lat: 47.6769, lon: -122.2060);

String route(({double lat, double lon}) from, ({double lat, double lon}) to) =>
    jsonEncode({
      'format': 'osrm',
      'costing': 'pedestrian',
      'locations': [
        {'lat': from.lat, 'lon': from.lon},
        {'lat': to.lat, 'lon': to.lon},
      ],
    });

Future<RegionProgress> waitFor(OfflineDownloads engine, int id) => engine
    .watch()
    .map((all) => all.where((r) => r.id == id).firstOrNull)
    .firstWhere(
      (r) =>
          r != null &&
          (r.state == RegionState.ready || r.state == RegionState.failed),
    )
    .then((r) => r!)
    .timeout(const Duration(minutes: 15));

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('downloads Seattle on the phone, and routes and plans over it', (
    tester,
  ) async {
    expect(bucketUrl, isNotEmpty, reason: 'needs CARDINAL_TEST_BUCKET_URL');
    expect(scratch, isNotEmpty, reason: 'needs CARDINAL_TEST_ROOT');
    final report = <String, Object?>{};
    final log = <String>[];
    void note(String line) {
      log.add(line);
      // ignore: avoid_print
      print('E2E $line');
    }

    await tester.runAsync(() async {
      final root = '$scratch/offline';
      if (Directory(root).existsSync()) {
        await Directory(root).delete(recursive: true);
      }
      await Directory(root).create(recursive: true);
      final db = OfflineDatabase(NativeDatabase(File('$root/offline.sqlite')));
      final engine = OfflineDownloads(
        db: db,
        root: root,
        bucket: Bucket(Uri.parse(bucketUrl)),
        transfers: BackgroundTransfers(),
      );
      await engine.open();
      final watch = Stopwatch()..start();
      final plan = await engine.plan(downtown);
      final totals = plan.totals({metro, soundTransit});
      report['plan_ms'] = watch.elapsedMilliseconds;
      note(
        'plan: ${watch.elapsedMilliseconds} ms; ${totals.files} files, '
        '${totals.mapRanges} map ranges, ${totals.downloadBytes} B',
      );

      watch.reset();
      final a = await engine.start(plan, feeds: {metro, soundTransit});
      report['start_ms'] = watch.elapsedMilliseconds;
      note('E2E-MARK downloading');
      final progress = await waitFor(engine, a);
      report['download_to_ready_ms'] = watch.elapsedMilliseconds;
      note(
        'region a: ${progress.state.name} after ${watch.elapsedMilliseconds} ms: '
        '$progress',
      );
      expect(progress.state, RegionState.ready, reason: progress.error);
      report['timings_a'] = {
        for (final MapEntry(key: k, value: (n, us)) in engine.timings.entries)
          k: {'count': n, 'ms': us ~/ 1000},
      };
      note('timings: ${report['timings_a']}');
      final ranges = await db.select(db.chunks).get();
      final transferRanges = ranges.where((c) => c.kind == ChunkKind.transfers);
      note(
        'ranges: ${ranges.length - transferRanges.length} map, '
        '${transferRanges.length} transfer '
        '(${transferRanges.fold(0, (sum, c) => sum + c.tileCount)} tiles)',
      );
      expect(transferRanges, isNotEmpty);
      expect(ranges.every((c) => c.state == ChunkState.deleted), isTrue);
      final retried = (await db.select(db.files).get()).where(
        (f) => f.attempts > 0,
      );
      note(
        'files that needed another attempt: '
        '${[for (final f in retried) '${f.path} (${f.attempts}: ${f.error})']}',
      );

      final releases = (await engine.readyReleases())!;
      final area = OfflineArea(
        routing: '$root/routing/${releases.routing}',
        transitBuild: '$root/transit/${releases.transit}',
        valhallaBuildId: releases.valhallaBuildId,
      );
      engine.addListener(area);
      watch.reset();
      final walk = await area.useRouter(
        (router) =>
            router.route(body: utf8.encode(route(westlake, calAnderson))),
      );
      report['first_route_ms'] = watch.elapsedMilliseconds;
      note('first route: ${walk.status} in ${watch.elapsedMilliseconds} ms');
      expect(walk.status, 200, reason: utf8.decode(walk.body));
      watch.reset();
      final again = await area.useRouter(
        (router) =>
            router.route(body: utf8.encode(route(westlake, calAnderson))),
      );
      note('second route: ${again.status} in ${watch.elapsedMilliseconds} ms');
      watch.reset();
      final trip = await area.useTransit(
        (transit) => transit.plan(
          request: utf8.encode(
            jsonEncode({
              'origin': {'lat': westlake.lat, 'lon': westlake.lon},
              'destination': {'lat': calAnderson.lat, 'lon': calAnderson.lon},
              'mode': 'walking',
            }),
          ),
          feeds: const [],
          cancel: TransitCancel(),
        ),
      );
      report['first_transit_plan_ms'] = watch.elapsedMilliseconds;
      final body = jsonDecode(utf8.decode(trip.body)) as Map<String, Object?>;
      note(
        'first transit plan (open included): ${trip.status} in '
        '${watch.elapsedMilliseconds} ms, ${(body['journeys'] as List?)?.length} '
        'journeys',
      );
      expect(trip.status, 200, reason: utf8.decode(trip.body));
      expect(body['journeys'], isNotEmpty);

      if (!background) {
        // An overlapping region, then deleting the first.
        watch.reset();
        final planB = await engine.plan(north);
        note('plan b: ${watch.elapsedMilliseconds} ms');
        watch.reset();
        final b = await engine.start(planB, feeds: {metro});
        final progressB = await waitFor(engine, b);
        note(
          'region b: ${progressB.state.name} in ${watch.elapsedMilliseconds} ms',
        );
        expect(progressB.state, RegionState.ready, reason: progressB.error);
        final across = await area.useRouter(
          (router) =>
              router.route(body: utf8.encode(route(greenLake, kirkland))),
        );
        expect(across.status, 200);
        watch.reset();
        await engine.delete(a);
        note('delete a: ${watch.elapsedMilliseconds} ms');
        final mapped = File('/proc/self/maps')
            .readAsLinesSync()
            .where((l) => l.contains(root) && l.endsWith('(deleted)'))
            .length;
        note('deleted files still mapped: $mapped');
        expect(mapped, 0);
        final still = await area.useRouter(
          (router) =>
              router.route(body: utf8.encode(route(greenLake, kirkland))),
        );
        expect(still.status, 200);
      }
      report['log'] = log;
      await File('$scratch/e2e.log').writeAsString('${log.join('\n')}\n');
      await engine.close();
      await db.close();
    });
    binding.reportData = report;
  }, timeout: const Timeout(Duration(minutes: 20)));
}
