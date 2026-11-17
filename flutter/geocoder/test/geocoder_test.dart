// Extracts two real tiles (north Seattle, from the Washington release) with the host build of
// `geocoder_flutter`.
//
// Build the library first, from `crates/`:
//
//   cargo build --release -p geocoder_flutter
//
// then run `timeout 45 flutter test --timeout 10s` here.

import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geocoder/geocoder.dart';
import 'package:geocoder/src/rust/frb_generated.dart';

const fixtures = '../../crates/geocoder_flutter/fixtures';

String library() {
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
}
