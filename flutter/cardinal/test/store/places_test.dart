import 'dart:io';

import 'package:cardinal/store/store.dart';
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';

/// A POI as a map pin knows it: center, version and the tile's tags.
const pin = Place(
  id: 'poi:97175',
  kind: 'poi',
  version: 44,
  name: 'Space Needle',
  center: LngLat(-122.3493, 47.6205),
  tags: {'tourism': 'attraction'},
);

/// The same POI as search knows it.
const searched = Place(
  id: 'poi:97175',
  kind: 'poi',
  version: 44,
  name: 'Space Needle',
  countryCode: 'US',
  center: LngLat(-122.3493, 47.6205),
  bbox: Bounds(
    west: -122.3496,
    south: 47.6203,
    east: -122.3491,
    north: 47.6207,
  ),
  shape: {
    'type': 'Polygon',
    'coordinates': [
      [
        [-122.3496, 47.6203],
        [-122.3491, 47.6203],
        [-122.3491, 47.6207],
        [-122.3496, 47.6203],
      ],
    ],
  },
  address: Address(
    housenumber: '400',
    street: 'Broad Street',
    postcode: '98109',
  ),
  admins: [
    AdminRef(id: 'wof:101730401', placetype: 'locality', name: 'Seattle'),
    AdminRef(
      id: 'wof:85688623',
      placetype: 'region',
      name: 'Washington',
      abbreviation: 'WA',
    ),
  ],
  tags: {'tourism': 'attraction', 'height': '184', 'wikidata': 'Q8475'},
);

const seattle = Place(
  id: 'wof:101730401',
  kind: 'admin',
  name: 'Seattle',
  center: LngLat(-122.331537, 47.673455),
  bbox: Bounds(
    west: -122.435937,
    south: 47.495514,
    east: -122.236044,
    north: 47.734165,
  ),
);

void main() {
  group('Place.merge', () {
    test('a newer version replaces the place whole', () {
      const newer = Place(
        id: 'poi:97175',
        kind: 'poi',
        version: 45,
        center: LngLat(0, 0),
      );
      expect(searched.merge(newer), newer);
    });

    test('an older version is ignored', () {
      const older = Place(
        id: 'poi:97175',
        kind: 'poi',
        version: 43,
        center: LngLat(0, 0),
      );
      expect(searched.merge(older), searched);
    });

    test('the same version fills in what is unknown and combines tags', () {
      const pinWithFee = Place(
        id: 'poi:97175',
        kind: 'poi',
        version: 44,
        center: LngLat(-122.3493, 47.6205),
        tags: {'tourism': 'attraction', 'fee': 'yes'},
      );
      final merged = searched.merge(pinWithFee);
      expect(merged.name, searched.name);
      expect(merged.shape, searched.shape);
      expect(merged.bbox, searched.bbox);
      expect(merged.address, searched.address);
      expect(merged.admins, searched.admins);
      expect(merged.tags, {...searched.tags, 'fee': 'yes'});
    });

    test('places without versions merge too', () {
      const pinned = Place(
        id: 'wof:101730401',
        kind: 'admin',
        name: 'Seattle',
        center: LngLat(-122.33, 47.67),
      );
      final merged = seattle.merge(pinned);
      expect(merged.center, pinned.center);
      expect(merged.bbox, seattle.bbox);
    });
  });

  group('PlaceStore', () {
    late DateTime now;
    late CardinalDatabase db;
    late PlaceStore places;

    setUp(() {
      now = DateTime.utc(2026, 11, 2);
      db = CardinalDatabase(NativeDatabase.memory());
      places = PlaceStore(db, now: () => now);
    });
    tearDown(() => db.close());

    /// Reads from the database, not the cache.
    Future<Place?> stored(String id) => PlaceStore(db).get(id);

    test('stores every field', () async {
      await places.put(searched);
      await places.put(seattle);
      expect(await stored(searched.id), searched);
      expect(await stored(seattle.id), seattle);
    });

    test('merges what is put with what is stored', () async {
      await places.put(searched);
      final merged = await places.put(pin);
      expect(merged, searched);
      expect(await stored(pin.id), searched);
    });

    test('has what was just put without waiting', () async {
      expect(places.cached(pin.id), isNull);
      await places.put(pin);
      expect(places.cached(pin.id), pin);
    });

    test('watch follows changes', () async {
      final seen = <Place?>[];
      final subscription = places.watch(pin.id).listen(seen.add);
      await pumpEventQueue();
      await places.put(pin);
      await pumpEventQueue();
      await places.put(searched);
      await pumpEventQueue();
      // Reading it again touches the row, but the place is the same.
      await places.get(pin.id);
      await pumpEventQueue();
      await subscription.cancel();
      expect(seen, [null, pin, searched]);
    });

    test('prunes places not used in a while', () async {
      await places.put(pin);
      await places.put(seattle);
      now = now.add(const Duration(days: 20));
      await places.get(seattle.id);
      now = now.add(const Duration(days: 20));
      expect(await places.prune(), 1);
      expect(await places.get(pin.id), isNull);
      expect(await places.get(seattle.id), seattle);
    });
  });

  test('places persist across opening the database', () async {
    final dir = await Directory.systemTemp.createTemp('cardinal_store');
    addTearDown(() => dir.delete(recursive: true));
    final file = File('${dir.path}/cardinal.sqlite');

    final first = Store(CardinalDatabase(NativeDatabase(file)));
    await first.places.put(searched);
    await first.close();

    final second = Store(CardinalDatabase(NativeDatabase(file)));
    addTearDown(second.close);
    expect(await second.places.get(searched.id), searched);
  });
}
