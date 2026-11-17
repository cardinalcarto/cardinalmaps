import 'dart:convert';

import 'package:cardinal/geocoder/address.dart';
import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// A search result as the geocoder sends it (trimmed).
Map<String, Object?> spaceNeedle() => {
  'id': 'poi:97175',
  'kind': 'poi',
  'name': 'Space Needle',
  'country_code': 'US',
  'center': [-122.3493, 47.6205],
  'address': {
    'housenumber': '400',
    'street': 'Broad Street',
    'unit': null,
    'postcode': '98109',
  },
  'admins': [
    {
      'id': 'wof:101730401',
      'placetype': 'locality',
      'name': 'Seattle',
      'abbreviation': null,
    },
    {
      'id': 'wof:102086191',
      'placetype': 'county',
      'name': 'King',
      'abbreviation': 'KN',
    },
    {
      'id': 'wof:85688623',
      'placetype': 'region',
      'name': 'Washington',
      'abbreviation': 'WA',
    },
    {
      'id': 'wof:85633793',
      'placetype': 'country',
      'name': 'United States',
      'abbreviation': 'U.S.',
    },
  ],
  'importance': 0.5,
  'score': 12.0,
  'poi': {'id': 97175, 'version': 44},
  'osm': {
    'type': 'way',
    'id': 12903132,
    'version': 44,
    'tags': {'tourism': 'attraction', 'height': '184'},
  },
  'wof': null,
  'feature': {
    'type': 'Feature',
    'bbox': [-122.3495555, 47.6203364, -122.3490519, 47.620676],
    'geometry': {
      'type': 'Polygon',
      'coordinates': [
        [
          [-122.3495, 47.6203],
          [-122.3490, 47.6203],
          [-122.3490, 47.6206],
          [-122.3495, 47.6203],
        ],
      ],
    },
    'properties': null,
  },
};

const _seattle = [
  AdminRef(id: 'wof:101730401', placetype: 'locality', name: 'Seattle'),
  AdminRef(
    id: 'wof:85688623',
    placetype: 'region',
    name: 'Washington',
    abbreviation: 'WA',
  ),
  AdminRef(id: 'wof:85633793', placetype: 'country', name: 'United States'),
];

void main() {
  test('a result becomes a place', () {
    final place = placeFromResult(spaceNeedle());
    expect(place.id, 'poi:97175');
    expect(place.version, 44);
    expect(place.countryCode, 'US');
    expect(place.center, const LngLat(-122.3493, 47.6205));
    expect(place.bbox?.north, 47.620676);
    expect(place.shape?['type'], 'Polygon');
    expect(place.address?.street, 'Broad Street');
    expect(place.admins?.first.name, 'Seattle');
    expect(place.tags, {'tourism': 'attraction', 'height': '184'});
  });

  test('a point geometry is no shape', () {
    final result = spaceNeedle();
    (result['feature'] as Map)['geometry'] = {
      'type': 'Point',
      'coordinates': [-122.3493, 47.6205],
    };
    expect(placeFromResult(result).shape, isNull);
  });

  group('address lines', () {
    test('a POI with an address: its street address by US convention', () {
      final place = placeFromResult(spaceNeedle());
      expect(placeTitle(place), 'Space Needle');
      expect(placeSubtitle(place), '400 Broad Street, Seattle, WA 98109');
    });

    test('a Swiss address: street, then postcode before the town', () {
      const place = Place(
        id: 'osm:node/1',
        kind: 'address',
        countryCode: 'CH',
        center: LngLat(8.54, 47.37),
        address: Address(
          housenumber: '4',
          street: 'Bahnhofstrasse',
          postcode: '8001',
        ),
        admins: [
          AdminRef(id: 'wof:1', placetype: 'locality', name: 'Zürich'),
          AdminRef(id: 'wof:2', placetype: 'region', name: 'Zürich'),
          AdminRef(id: 'wof:3', placetype: 'country', name: 'Switzerland'),
        ],
      );
      expect(placeTitle(place), 'Bahnhofstrasse 4');
      expect(placeSubtitle(place), '8001 Zürich');
    });

    test('a POI without an address: its city and region', () {
      const place = Place(
        id: 'poi:1',
        kind: 'poi',
        name: 'Kerry Park',
        countryCode: 'US',
        center: LngLat(-122.36, 47.63),
        admins: _seattle,
      );
      expect(placeSubtitle(place), 'Seattle, WA');
    });

    test('an administrative area: the areas containing it', () {
      const place = Place(
        id: 'wof:101730401',
        kind: 'admin',
        name: 'Seattle',
        countryCode: 'US',
        center: LngLat(-122.33, 47.67),
        admins: [
          AdminRef(id: 'wof:102086191', placetype: 'county', name: 'King'),
          AdminRef(id: 'wof:85688623', placetype: 'region', name: 'Washington'),
          AdminRef(
            id: 'wof:85633793',
            placetype: 'country',
            name: 'United States',
          ),
        ],
      );
      expect(placeSubtitle(place), 'King, Washington, United States');
    });

    test('nothing known: no subtitle', () {
      const place = Place(
        id: 'poi:2',
        kind: 'poi',
        name: 'Somewhere',
        center: LngLat(0, 0),
      );
      expect(placeSubtitle(place), isNull);
    });
  });

  group('Geocoder', () {
    test('searches with the given options', () async {
      late Uri asked;
      final geocoder = OnlineGeocoder(
        Uri.parse('https://example.org/geocoder/'),
        client: MockClient((request) async {
          asked = request.url;
          return http.Response(
            jsonEncode({
              'query': {},
              'results': [spaceNeedle()],
            }),
            200,
          );
        }),
      );
      final results = await geocoder.search(
        'space ne',
        focus: const LngLat(-122.33, 47.6),
        lang: 'en',
        prefix: true,
        typos: true,
      );
      expect(results.single.id, 'poi:97175');
      expect(asked.path, '/geocoder/v1/search');
      expect(asked.queryParameters, {
        'q': 'space ne',
        'lang': 'en',
        'prefix': 'true',
        'typos': 'true',
        'focus': '-122.33,47.6',
      });
    });

    test('looks up a place by ID, or finds none', () async {
      final geocoder = OnlineGeocoder(
        Uri.parse('http://localhost:3000'),
        client: MockClient((request) async {
          expect(request.url.path, '/v1/place');
          return switch (request.url.queryParameters['id']) {
            'poi:97175' => http.Response(
              jsonEncode({'result': spaceNeedle()}),
              200,
            ),
            _ => http.Response('{"title": "Not Found"}', 404),
          };
        }),
      );
      expect((await geocoder.place('poi:97175'))?.name, 'Space Needle');
      expect(await geocoder.place('poi:1'), isNull);
    });

    test('reports problems', () {
      final geocoder = OnlineGeocoder(
        Uri.parse('http://localhost:3000'),
        client: MockClient(
          (_) async => http.Response('{"detail": "q is empty"}', 400),
        ),
      );
      expect(
        geocoder.search('x'),
        throwsA(
          isA<GeocoderException>().having(
            (e) => e.message,
            'message',
            contains('q is empty'),
          ),
        ),
      );
    });
  });
}
