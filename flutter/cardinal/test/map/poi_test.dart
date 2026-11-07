import 'package:cardinal/map/poi.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:sparkle/sparkle.dart';

ClickedFeature _feature(Map<String, String> properties) => ClickedFeature(
  source: 'openmaptiles',
  sourceLayer: poiLayer,
  tileZ: 14,
  tileX: 2624,
  tileY: 5722,
  id: BigInt.from(1234),
  lng: -122.3,
  lat: 47.6,
  properties: properties,
);

const _seattle = WofArea({'name': 'Seattle', 'jpn': 'シアトル'});
const _washington = WofArea({'name': 'Washington', 'abbr': 'WA'});
const _us = WofArea({'name': 'United States', 'fra': 'États-Unis', 'country': 'US'});
const _puertoRico = WofArea({'name': 'Puerto Rico', 'country': 'PR'});

const _areas = {
  1: _seattle,
  2: _washington,
  3: _us,
  4: _puertoRico,
};

WofArea? _area(String placetype, int id) => _areas[id];

void main() {
  test('a POI is the place the geocoder calls by its ID', () {
    final place = placeFromPoi(
      _feature({
        'name': 'Elliott Bay Book Company',
        'name:es': 'Librería',
        'name_en': 'Elliott Bay',
        'class': 'shop',
        'subclass': 'books',
        'rank': '2',
        'cardinal:version': '7',
        'cardinal:admin:locality': '1',
        'shop': 'books',
        'website': 'https://example.com',
        'addr:housenumber': '1521',
        'addr:street': '10th Avenue',
        'addr:postcode': '98122',
      }),
      lang: 'en',
    );
    expect(place.id, 'poi:1234');
    expect(place.kind, 'poi');
    expect(place.version, 7);
    expect(place.name, 'Elliott Bay Book Company');
    expect(place.center, const LngLat(-122.3, 47.6));
    expect(
      place.address,
      const Address(housenumber: '1521', street: '10th Avenue', postcode: '98122'),
    );
    expect(place.tags, {
      'name': 'Elliott Bay Book Company',
      'name:es': 'Librería',
      'shop': 'books',
      'website': 'https://example.com',
      'addr:housenumber': '1521',
      'addr:street': '10th Avenue',
      'addr:postcode': '98122',
    });
  });

  test('its name in the language asked for, when it has one', () {
    final place = placeFromPoi(_feature({'name': 'Book Company', 'name:es': 'Librería'}), lang: 'es');
    expect(place.name, 'Librería');
  });

  test('a POI without a name or address has neither', () {
    final place = placeFromPoi(_feature({'amenity': 'toilets'}));
    expect(place.name, isNull);
    expect(place.address, isNull);
    expect(place.version, isNull);
  });

  group('the areas it is in', () {
    final properties = {
      'cardinal:admin:country': '3',
      'cardinal:admin:region': '2',
      'cardinal:admin:locality': '1',
    };

    test('come smallest first, with their abbreviations and the country code', () {
      final place = placeFromPoi(_feature(properties), area: _area);
      expect(place.admins, const [
        AdminRef(id: 'wof:1', placetype: 'locality', name: 'Seattle'),
        AdminRef(id: 'wof:2', placetype: 'region', name: 'Washington', abbreviation: 'WA'),
        AdminRef(id: 'wof:3', placetype: 'country', name: 'United States'),
      ]);
      expect(place.countryCode, 'US');
    });

    test('are named in the language asked for, or by default', () {
      List<String?> names(String lang) => placeFromPoi(
        _feature(properties),
        lang: lang,
        area: _area,
      ).admins!.map((a) => a.name).toList();
      expect(names('ja'), ['シアトル', 'Washington', 'United States']);
      expect(names('fr'), ['Seattle', 'Washington', 'États-Unis']);
      expect(names('xx'), ['Seattle', 'Washington', 'United States']);
    });

    test('a dependency gives the country code over its country', () {
      final place = placeFromPoi(
        _feature({'cardinal:admin:dependency': '4', 'cardinal:admin:country': '3'}),
        area: _area,
      );
      expect(place.countryCode, 'PR');
    });

    test('are left out if unknown', () {
      final place = placeFromPoi(
        _feature({'cardinal:admin:locality': '99', 'cardinal:admin:country': '3'}),
        area: _area,
      );
      expect(place.admins!.map((a) => a.id), ['wof:3']);
    });

    test('its address tag says the country when the areas do not', () {
      final place = placeFromPoi(_feature({'addr:country': 'ca'}));
      expect(place.countryCode, 'CA');
      expect(place.admins, isEmpty);
    });
  });

  test("tiles' version of a place agrees with a search's", () {
    const stored = Place(
      id: 'poi:1234',
      kind: 'poi',
      version: 7,
      center: LngLat(-122.3, 47.6),
      bbox: Bounds(west: 0, south: 0, east: 1, north: 1),
    );
    final merged = stored.merge(
      placeFromPoi(
        _feature({'cardinal:version': '7', 'name': 'A', 'cardinal:admin:locality': '1'}),
        area: _area,
      ),
    );
    expect(merged.name, 'A');
    expect(merged.bbox, stored.bbox);
    expect(merged.admins!.single.name, 'Seattle');
  });

  test('poiAdmins finds IDs by placetype, smallest first', () {
    expect(
      poiAdmins({'cardinal:admin:country': '3', 'name': 'x', 'cardinal:admin:locality': '1'}),
      {'locality': 1, 'country': 3},
    );
  });
}
