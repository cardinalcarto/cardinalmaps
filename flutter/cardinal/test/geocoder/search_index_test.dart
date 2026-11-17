// The device's search index: one index of the ready regions, built again into a new
// directory when they change, with each region's WOF areas fetched once and kept.
import 'dart:convert';
import 'dart:io';

import 'package:cardinal/geocoder/search_index.dart';
import 'package:cardinal/offline/downloads/engine.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geocoder/geocoder.dart' show WofBox;
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

late Directory tmp;
late List<Uri> requests;
late List<IndexBuildJob> builds;
var wofUp = true;

const seattle = IndexRegion(
  id: 1,
  bounds: (west: -122.44, south: 47.49, east: -122.23, north: 47.74),
);
const tacoma = IndexRegion(
  id: 2,
  bounds: (west: -122.6, south: 47.1, east: -122.3, north: 47.3),
);

IndexSources sources(List<IndexRegion> regions) => IndexSources(
  mapRelease: '2026-11-09',
  basemap: '${tmp.path}/map/2026-11-09/basemap.mbtiles',
  regions: regions,
);

SearchIndex newIndex() => SearchIndex(
  root: '${tmp.path}/search',
  wofUrl: 'https://wof.example.org/base/',
  client: MockClient((request) async {
    requests.add(request.url);
    if (!wofUp) return http.Response('down', 503);
    return http.Response('{"places": []}', 200);
  }),
  // Two boxes a region, without the native library.
  boxes: (b) => [
    WofBox(west: b.west, south: b.south, east: b.east, north: b.north),
    WofBox(west: b.west, south: b.south, east: b.east, north: b.north),
  ],
  build: (job) async {
    builds.add(job);
    await Directory(job.dir).create(recursive: true);
    return 'built';
  },
);

void main() {
  setUp(() async {
    tmp = await Directory.systemTemp.createTemp('search_index_test');
    requests = [];
    builds = [];
    wofUp = true;
  });

  tearDown(() => tmp.delete(recursive: true));

  test('a ready region gets its areas once, and an index; nothing changed, nothing built', () async {
    final index = newIndex();
    await index.load();
    expect(index.current.value, isNull);
    index.regionsChanged(sources([seattle]));
    await index.settled;
    expect(requests, hasLength(2));
    expect(requests.first.path, '/base/v1/places');
    expect(
      requests.first.queryParameters['bbox'],
      '-122.44,47.49,-122.23,47.74',
    );
    expect(builds, hasLength(1));
    expect(builds.single.regions, [1]);
    expect(builds.single.wofFiles, [
      '${tmp.path}/search/wof/1/0.json',
      '${tmp.path}/search/wof/1/1.json',
    ]);
    expect(index.current.value!.dir, builds.single.dir);
    expect(index.current.value!.basemap, endsWith('basemap.mbtiles'));

    index.regionsChanged(sources([seattle]));
    await index.settled;
    expect(requests, hasLength(2));
    expect(builds, hasLength(1));

    // After a restart, the same index, and no work.
    final again = newIndex();
    await again.load();
    expect(again.current.value, index.current.value);
    again.regionsChanged(sources([seattle]));
    await again.settled;
    expect(builds, hasLength(1));
  });

  test(
    'adding and deleting regions rebuilds from all of them; the old index goes',
    () async {
      final index = newIndex();
      await index.load();
      index.regionsChanged(sources([seattle]));
      await index.settled;
      final first = index.current.value!.dir;
      index.regionsChanged(sources([seattle, tacoma]));
      await index.settled;
      expect(builds.last.regions, [1, 2]);
      // Only the new region's areas were fetched.
      expect(requests, hasLength(4));
      final second = index.current.value!.dir;
      expect(second, isNot(first));
      expect(Directory(first).existsSync(), isFalse);

      index.regionsChanged(sources([tacoma]));
      await index.settled;
      expect(builds.last.regions, [2]);
      expect(Directory('${tmp.path}/search/wof/1').existsSync(), isFalse);
      expect(Directory(second).existsSync(), isFalse);

      index.regionsChanged(IndexSources.none);
      await index.settled;
      expect(index.current.value, isNull);
      expect(File('${tmp.path}/search/current.json').existsSync(), isFalse);
      expect(
        Directory('${tmp.path}/search')
            .listSync()
            .whereType<Directory>()
            .map((d) => d.uri.pathSegments.where((s) => s.isNotEmpty).last),
        ['wof'],
      );
    },
  );

  test(
    'without the WOF service it indexes without areas, and fetches them later',
    () async {
      wofUp = false;
      final index = newIndex();
      await index.load();
      index.regionsChanged(sources([seattle]));
      await index.settled;
      expect(builds.single.wofFiles, isEmpty);
      expect(Directory('${tmp.path}/search/wof/1').existsSync(), isFalse);

      wofUp = true;
      index.regionsChanged(sources([seattle]));
      await index.settled;
      expect(builds, hasLength(2));
      expect(builds.last.wofFiles, hasLength(2));
    },
  );

  test('the old index stays searchable while a new one builds', () async {
    final index = newIndex();
    await index.load();
    index.regionsChanged(sources([seattle]));
    await index.settled;
    final old = index.current.value;
    final building = <bool>[];
    index.building.addListener(() => building.add(index.building.value));
    final seen = <SearchIndexFiles?>[];
    late final SearchIndex slow;
    slow = SearchIndex(
      root: '${tmp.path}/search',
      wofUrl: '',
      build: (job) async {
        seen.add(slow.current.value);
        await Directory(job.dir).create(recursive: true);
        return 'built';
      },
    );
    await slow.load();
    expect(slow.current.value, old);
    slow.building.addListener(() => building.add(slow.building.value));
    slow.regionsChanged(sources([seattle, tacoma]));
    await slow.settled;
    expect(seen, [old]);
    expect(building, [true, false]);
    expect(slow.current.value, isNot(old));
    final manifest = jsonDecode(
      File('${tmp.path}/search/current.json').readAsStringSync(),
    ) as Map<String, Object?>;
    expect(manifest['dir'], slow.current.value!.dir);
  });

  test('a failed build keeps the index there was', () async {
    final index = newIndex();
    await index.load();
    index.regionsChanged(sources([seattle]));
    await index.settled;
    final old = index.current.value;
    final failing = SearchIndex(
      root: '${tmp.path}/search',
      wofUrl: '',
      build: (job) async {
        await Directory(job.dir).create(recursive: true);
        throw StateError('no space');
      },
    );
    await failing.load();
    failing.regionsChanged(sources([seattle, tacoma]));
    await failing.settled;
    expect(failing.current.value, old);
    expect(Directory(old!.dir).existsSync(), isTrue);
    expect(
      Directory('${tmp.path}/search')
          .listSync()
          .where((e) => e.path.contains('index-')),
      hasLength(1),
    );
  });
}
