import 'dart:io';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/offline/offline_area.dart';
import 'package:cardinal/offline/realtime.dart';
import 'package:cardinal/transit/nearby_source.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('an offline area without transit', () {
    late OfflineArea area;
    late List<String> logged;
    setUp(() async {
      final root = await Directory.systemTemp.createTemp('nearby_source_test');
      addTearDown(() => root.delete(recursive: true));
      // Seattle's square of streets, and no transit release.
      await File('${root.path}/routing/r/tiles/2/000/792/230.gph')
          .create(recursive: true);
      area = OfflineArea(routing: '${root.path}/routing/r');
      logged = [];
      final print = debugPrint;
      debugPrint = (message, {wrapWidth}) => logged.add(message ?? '');
      addTearDown(() => debugPrint = print);
    });

    test('is got ready for quietly', () async {
      OfflineNearbySource(() async => area, AreaRealtimeFeeds()).prepare();
      OfflineTransitSource(() async => area, AreaRealtimeFeeds()).prepare();
      await pumpEventQueue();
      expect(logged, isEmpty);
    });

    test('has no timetables to open, and no real-time data', () async {
      expect(area.hasTransit, isFalse);
      await expectLater(area.transit, throwsA(isA<NoOfflineTransit>()));
      expect(await area.realtimeSources, isEmpty);
    });
  });
}
