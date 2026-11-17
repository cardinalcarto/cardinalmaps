// Extracts two real tiles (north Seattle, from the Washington release) with the host build of
// `geocoder_flutter`.
//
// Build the library first, from `crates/`:
//
//   cargo build --release -p geocoder_flutter
//
// then run `timeout 45 flutter test --timeout 10s` here (with GEOCODER_LIBRARY set to the
// library's path if it was built elsewhere, as with CARGO_TARGET_DIR).

import 'dart:convert';
import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geocoder/geocoder.dart';
import 'package:geocoder/src/rust/frb_generated.dart';

const fixtures = '../../crates/geocoder_flutter/fixtures';

String library() {
  if (Platform.environment['GEOCODER_LIBRARY'] case final path?) return path;
  final name = Platform.isMacOS
      ? 'libgeocoder_flutter.dylib'
      : Platform.isWindows
      ? 'geocoder_flutter.dll'
      : 'libgeocoder_flutter.so';
  return '../../crates/target/release/$name';
}

GeocoderTile tile(int y) => GeocoderTile(
  z: 14,
  x: 2629,
  y: y,
  data: File('$fixtures/14-2629-$y.mlt.gz').readAsBytesSync(),
);

void main() {
  setUpAll(() async {
    await RustLib.init(externalLibrary: ExternalLibrary.open(library()));
  });

  test('extracts a page of tiles, then joins streets', () async {
    final extraction = RegionExtraction();
    final read = await extraction.addTiles(tiles: [tile(5697), tile(5698)]);
    expect(read.tiles, 2);
    final poi = read.layers.firstWhere((l) => l.layer == 'poi');
    expect(poi.points, greaterThan(10));
    expect(poi.bufferPoints, greaterThan(0));
    expect(read.joined, 0);
    final done = await extraction.finish();
    expect(done.joined, greaterThan(0));
    expect(done.features, greaterThan(read.features));
    await expectLater(
      extraction.finish(),
      throwsA(
        isA<ExtractError>().having(
          (e) => e.kind,
          'kind',
          ExtractErrorKind.finished,
        ),
      ),
    );
  });

  test('a bad tile says which', () async {
    final extraction = RegionExtraction();
    await expectLater(
      extraction.addTiles(
        tiles: [GeocoderTile(z: 14, x: 1, y: 2, data: Uint8List(8))],
      ),
      throwsA(
        isA<ExtractError>()
            .having((e) => e.kind, 'kind', ExtractErrorKind.badTile)
            .having((e) => e.message, 'message', contains('14/1/2')),
      ),
    );
  });

  test(
    'builds an index of the tiles, searches it, and reads a result\'s feature',
    () async {
      final dir = await Directory.systemTemp.createTemp('geocoder_index_test');
      addTearDown(() => dir.delete(recursive: true));
      final wof = File('${dir.path}/wof.json')
        ..writeAsStringSync(
          '{"places": [{"id": 101730401, "placetype": "locality", '
          '"names": [["und", "default", false, "Seattle"]], "ancestors": [], '
          '"country": "US", "label": [-122.33, 47.61], "geometry": null}]}',
        );
      final builder = await IndexBuilder.create(
        dir: '${dir.path}/index',
        wofFiles: [wof.path],
        heapMb: 30,
        threads: 1,
      );
      await builder.addTiles(tiles: [tile(5697), tile(5698)]);
      final stats = await builder.finish();
      expect(stats.tiles, 2);
      expect(stats.admins, 1);
      expect(stats.pointers, greaterThan(10));

      final index = await OfflineIndex.open(dir: '${dir.path}/index');
      final seattle = await index.search(
        query: OfflineQuery(
          text: 'seattle',
          prefix: false,
          typos: false,
          limit: 5,
        ),
      );
      expect(seattle, contains('"wof:101730401"'));
      final results =
          jsonDecode(
                await index.search(
                  query: OfflineQuery(
                    text: 'park',
                    prefix: true,
                    typos: false,
                    limit: 20,
                  ),
                ),
              )['results']
              as List<Object?>;
      final pointer =
          results.cast<Map<String, Object?>>().firstWhere(
                (r) => r['pointer'] != null,
              )['pointer']
              as Map<String, Object?>;
      final at = pointer['tile'] as List<Object?>;
      final feature = await poiFeature(
        tile: tile(at[2] as int),
        poiId: BigInt.from(pointer['poi_id'] as int),
      );
      expect(feature!.properties['name'], isNotNull);
      index.dispose();
    },
  );

  test('a region splits into boxes the WOF service answers', () {
    final boxes = wofBoxes(
      west: -124.85,
      south: 45.54,
      east: -116.91,
      north: 49.01,
      side: 3,
    );
    expect(boxes, hasLength(6));
    expect(boxes.last.north, 49.01);
  });
}
