// Offline search over a fake native side: results as the server's, pointers filled in
// from their tile features, and the index and models opened as they change.
import 'dart:convert';
import 'dart:io';

import 'package:cardinal/geocoder/offline_geocoder.dart';
import 'package:cardinal/geocoder/search_index.dart';
import 'package:cardinal/store/store.dart' show LngLat;
import 'package:flutter_test/flutter_test.dart';
import 'package:geocoder/geocoder.dart' show OfflineQuery;

/// Pike Place Market as a pointer, and an address stored whole.
const results = {
  'results': [
    {
      'id': 'poi:230413',
      'kind': 'poi',
      'name': 'Pike Place Market',
      'center': [-122.3421, 47.6097],
      'address': null,
      'admins': [
        {
          'id': 'wof:101730401',
          'placetype': 'locality',
          'name': 'Seattle',
          'abbreviation': null,
        },
      ],
      'country_code': 'US',
      'osm': {
        'type': null,
        'id': null,
        'version': null,
        'tags': {'name': 'Pike Place Market'},
      },
      'pointer': {
        'poi_id': 230413,
        'tile': [14, 2624, 5722],
      },
      'feature': {
        'type': 'Feature',
        'bbox': [-122.3421, 47.6097, -122.3421, 47.6097],
        'geometry': {
          'type': 'Point',
          'coordinates': [-122.3421, 47.6097],
        },
      },
    },
    {
      'id': 'osm:way/256644096',
      'kind': 'address',
      'name': null,
      'center': [-122.34, 47.61],
      'address': {
        'housenumber': '1530',
        'street': 'Post Alley',
        'unit': null,
        'postcode': '98101',
      },
      'admins': [],
      'country_code': 'US',
      'osm': {
        'type': 'way',
        'id': 256644096,
        'version': null,
        'tags': {'addr:housenumber': '1530', 'addr:street': 'Post Alley'},
      },
      'pointer': null,
      'feature': {
        'type': 'Feature',
        'bbox': [-122.34, 47.61, -122.34, 47.61],
        'geometry': {
          'type': 'Point',
          'coordinates': [-122.34, 47.61],
        },
      },
    },
  ],
};

class FakeSearcher implements OfflineSearcher {
  FakeSearcher(this.dir, this.models);

  final String dir;
  final GeocoderModelFiles models;
  final queries = <OfflineQuery>[];
  var disposed = false;

  @override
  Future<String> search(OfflineQuery query) async {
    queries.add(query);
    return jsonEncode(results);
  }

  @override
  Future<String?> place(String id, String? lang) async {
    final found = (results['results'] as List<Map<String, Object?>>)
        .where((r) => r['id'] == id)
        .firstOrNull;
    return found == null ? null : jsonEncode(found);
  }

  @override
  void dispose() => disposed = true;
}

class FakeNative implements OfflineSearchNative {
  final opened = <FakeSearcher>[];
  final asked = <(String, List<PoiInTile>)>[];

  /// The tile features, by POI ID; none means the tile is gone.
  Map<int, Map<String, String>> tiles = {
    230413: {
      'name': 'Pike Place Market',
      'name:es': 'Mercado de Pike Place',
      'addr:street': 'Pike Place',
      'addr:housenumber': '85',
      'amenity': 'marketplace',
      'class': 'shop',
      'cardinal:version': '41',
      'cardinal:admin:locality': '101730401',
    },
  };

  @override
  Future<OfflineSearcher> open(String dir, GeocoderModelFiles models) async {
    final searcher = FakeSearcher(dir, models);
    opened.add(searcher);
    return searcher;
  }

  @override
  Future<List<Map<String, String>?>> features(
    String basemap,
    List<PoiInTile> wanted,
  ) async {
    asked.add((basemap, wanted));
    return [for (final w in wanted) tiles[w.poiId]];
  }
}

void main() {
  late Directory tmp;
  late SearchIndex index;
  late FakeNative native;
  late GeocoderModelFiles models;
  late OfflineGeocoder geocoder;

  setUp(() async {
    tmp = await Directory.systemTemp.createTemp('offline_geocoder_test');
    index = SearchIndex(root: '${tmp.path}/search', wofUrl: '');
    index.current.value = const SearchIndexFiles(
      dir: '/index-1',
      basemap: '/basemap.mbtiles',
    );
    native = FakeNative();
    models = (tagger: null, categories: null);
    geocoder = OfflineGeocoder(
      index: Future.value(index),
      models: () async => models,
      native: native,
    );
  });

  tearDown(() => tmp.delete(recursive: true));

  test(
    'pointers are filled in from their tiles; the rest is as stored',
    () async {
      final places = await geocoder.search(
        'pike',
        lang: 'es',
        prefix: true,
        focus: const LngLat(-122.33, 47.6),
      );
      expect(native.asked.single.$1, '/basemap.mbtiles');
      expect(native.asked.single.$2.single, (
        z: 14,
        x: 2624,
        y: 5722,
        poiId: 230413,
      ));
      final query = native.opened.single.queries.single;
      expect(query.text, 'pike');
      expect(query.prefix, isTrue);
      expect(query.focus, [-122.33, 47.6]);
      expect(query.limit, 10);

      final market = places[0];
      expect(market.id, 'poi:230413');
      expect(market.kind, 'poi');
      // In the language asked for, from the tile.
      expect(market.name, 'Mercado de Pike Place');
      expect(market.version, 41);
      expect(market.address?.housenumber, '85');
      expect(market.address?.street, 'Pike Place');
      expect(market.tags['amenity'], 'marketplace');
      expect(market.tags.containsKey('class'), isFalse);
      // The areas from the index.
      expect(market.admins?.single.name, 'Seattle');
      expect(market.countryCode, 'US');
      expect(market.center.lat, 47.6097);

      final address = places[1];
      expect(address.id, 'osm:way/256644096');
      expect(address.address?.street, 'Post Alley');
      expect(address.tags['addr:housenumber'], '1530');
    },
  );

  test('a pointer whose tile is gone shows what the index has', () async {
    native.tiles = {};
    final market = (await geocoder.search('pike')).first;
    expect(market.name, 'Pike Place Market');
    expect(market.address, isNull);
  });

  test('a place by its ID', () async {
    final market = await geocoder.place('poi:230413');
    expect(market?.address?.housenumber, '85');
    expect(await geocoder.place('poi:1'), isNull);
  });

  test('no index, no results', () async {
    index.current.value = null;
    expect(await geocoder.search('pike'), isEmpty);
    expect(native.opened, isEmpty);
    expect(await OfflineGeocoder.none().search('pike'), isEmpty);
  });

  test('a new index, or models that arrived, open it again', () async {
    await geocoder.search('pike');
    await geocoder.search('pike');
    expect(native.opened, hasLength(1));

    index.current.value = const SearchIndexFiles(
      dir: '/index-2',
      basemap: '/basemap.mbtiles',
    );
    await geocoder.search('pike');
    expect(native.opened.map((s) => s.dir), ['/index-1', '/index-2']);
    expect(native.opened.first.disposed, isTrue);

    models = (tagger: '/tagger.model', categories: '/eng.amcats');
    // Read again only once downloads change.
    await geocoder.search('pike');
    expect(native.opened, hasLength(2));
    await geocoder.offlineChanged(feedsChanged: false);
    await geocoder.search('pike');
    expect(native.opened, hasLength(3));
    expect(native.opened.last.models.tagger, '/tagger.model');
    expect(native.opened.last.models.categories, '/eng.amcats');
  });
}
