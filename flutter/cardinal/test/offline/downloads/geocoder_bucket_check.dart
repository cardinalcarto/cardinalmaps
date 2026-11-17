// Downloads the geocoder's models with the real engine from a bucket directory laid out
// as `cardinal-publish --target dir:` would (`releases/latest.json` naming a `geocoder`
// release, `releases/geocoder/<date>/{tagger.model,categorical/<lang>.amcats}` and their
// `.meta/`), served locally by [FakeBucket]. Not part of the suite; run by path:
//
//   GEOCODER_BUCKET=/mnt/maps/cardinal/offline-geocoder/fake-bucket \
//     timeout 45 flutter test --timeout 30s test/offline/downloads/geocoder_bucket_check.dart
import 'dart:io';

import 'package:cardinal/offline/downloads/bucket.dart';
import 'package:cardinal/offline/downloads/database.dart';
import 'package:cardinal/offline/downloads/engine.dart';
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_bucket.dart';
import 'fakes.dart';

void main() {
  final dir = Platform.environment['GEOCODER_BUCKET'];

  test('plans, downloads, checks and installs the models', () async {
    final server = await FakeBucket.serve(dir!);
    final tmp = await Directory.systemTemp.createTemp('geocoder_bucket_check');
    final db = OfflineDatabase(NativeDatabase.memory());
    final transfers = FakeTransfers();
    final engine = OfflineDownloads(
      db: db,
      root: tmp.path,
      bucket: Bucket(Uri.parse(server.releasesUrl)),
      transfers: transfers,
      native: FakeNative('', '$dir/releases'),
    );
    try {
      await engine.open();
      final plan = (await engine.planGeocoderModels('eng'))!;
      // ignore: avoid_print
      print(
        'plan: ${plan.release}, '
        '${[for (final f in plan.files) '${f.path} ${f.bytes}'].join(', ')}',
      );
      final id = await engine.startGeocoderModels(plan);
      for (var i = 0; i < 50; i++) {
        await transfers.drain();
        await Future<void>.delayed(const Duration(milliseconds: 20));
        await engine.settled;
        final region = (await engine.regions()).singleWhere((r) => r.id == id);
        if (region.state == RegionState.ready ||
            region.state == RegionState.failed) {
          break;
        }
      }
      final region = (await engine.regions()).singleWhere((r) => r.id == id);
      // ignore: avoid_print
      print('region: $region');
      expect(region.state, RegionState.ready);
      for (final f in plan.files) {
        expect(File('${tmp.path}/${f.installPath}').lengthSync(), f.bytes);
      }
      expect(await engine.planGeocoderModels('kli'), isNotNull);
    } finally {
      await engine.close();
      transfers.close();
      await server.close();
      await db.close();
      await tmp.delete(recursive: true);
    }
  }, skip: dir == null ? 'GEOCODER_BUCKET is not set' : null);
}
