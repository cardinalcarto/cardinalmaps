import 'dart:io';

import 'package:cardinal/store/store.dart';
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';

Place poi(int id) =>
    Place(id: 'poi:$id', kind: 'poi', center: const LngLat(0, 0));

void main() {
  test(
    'local history persists, deduplicates, and is cleared when disabled',
    () async {
      final dir = await Directory.systemTemp.createTemp('cardinal_history');
      addTearDown(() => dir.delete(recursive: true));
      final file = File('${dir.path}/cardinal.sqlite');
      var store = Store(CardinalDatabase(NativeDatabase(file)));
      Future<void> record(Place place) async {
        await store.places.put(place);
        await store.recentSearches.record(place);
      }

      Future<List<String>> ids() async =>
          (await store.recentSearches.watch().first).map((p) => p.id).toList();
      expect(await store.settings.get(saveSearchHistory), isTrue);
      await record(poi(1));
      await record(poi(2));
      await record(poi(1));
      await record(
        const Place(id: 'admin:1', kind: 'admin', center: LngLat(0, 0)),
      );
      expect(await ids(), ['poi:1', 'poi:2']);
      await store.close();
      store = Store(CardinalDatabase(NativeDatabase(file)));
      addTearDown(store.close);
      expect(await ids(), ['poi:1', 'poi:2']);
      await store.recentSearches.remove('poi:1');
      expect(await ids(), ['poi:2']);
      await store.settings.set(saveSearchHistory, false);
      expect(await ids(), isEmpty);
      await record(poi(3));
      expect(await ids(), isEmpty);
      await store.settings.set(saveSearchHistory, true);
      expect(await ids(), isEmpty);
      await record(poi(3));
      expect(await ids(), ['poi:3']);
    },
  );

  test('history is capped and protects its POIs from pruning', () async {
    final store = Store(CardinalDatabase(NativeDatabase.memory()));
    addTearDown(store.close);
    for (var i = 0; i <= RecentSearchStore.limit; i++) {
      await store.places.put(poi(i));
      await store.recentSearches.record(poi(i));
    }
    final recent = await store.recentSearches.watch().first;
    expect(recent.length, RecentSearchStore.limit);
    expect(recent.first.id, 'poi:50');
    expect(recent.last.id, 'poi:1');
    await store.places.prune(age: const Duration(days: -1));
    expect(await store.places.get('poi:0'), isNull);
    expect(await store.places.get('poi:1'), isNotNull);
  });
}
