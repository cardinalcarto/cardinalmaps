import 'package:cardinal/map/selection.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:sparkle/sparkle.dart';

import '../fakes/fake_map.dart';

class _ZoomedMap extends FakeMap {
  _ZoomedMap(this.zoom);

  final double zoom;

  @override
  CameraState? get camera => CameraState(lng: 0, lat: 0, zoom: zoom, bearing: 0, pitch: 0);
}

const _place = Place(id: 'poi:1', kind: 'poi', center: LngLat(-122.3, 47.6));

void main() {
  test('the pin says whether it is for dark mode', () {
    Object? dark(bool value) {
      final map = FakeMap();
      showSelection(map, _place, dark: value);
      final data = map.geoJson[selectionSource]! as Map<String, Object?>;
      final pin = (data['features']! as List).cast<Map<String, Object?>>().last;
      return (pin['properties']! as Map)['dark'];
    }

    expect(dark(true), isTrue);
    expect(dark(false), isFalse);
  });

  test('an area big enough to see shows its boundary and no pin; other places keep their pin', () {
    List<String> roles(Map<String, Object?>? shape) {
      final map = FakeMap();
      showSelection(map, Place(id: 'poi:1', kind: 'poi', center: const LngLat(-122.33, 47.6), shape: shape, tags: const {}));
      final data = map.geoJson[selectionSource]! as Map<String, Object?>;
      return [
        for (final f in (data['features']! as List).cast<Map<String, Object?>>()) (f['properties']! as Map)['role'] as String,
      ];
    }

    // A square [size] degrees across, in Seattle.
    List<List<List<double>>> square(double size) => [
      [
        [-122.33, 47.6],
        [-122.33 + size, 47.6],
        [-122.33 + size, 47.6 + size],
        [-122.33, 47.6],
      ],
    ];
    // About 1.5 km across: a park.
    expect(roles({'type': 'Polygon', 'coordinates': square(0.02)}), ['boundary']);
    expect(roles({'type': 'MultiPolygon', 'coordinates': [square(0.02)]}), ['boundary']);
    // About 30 m across: a building, whose boundary is a speck at the zoom it's shown at.
    expect(roles({'type': 'Polygon', 'coordinates': square(0.0003)}), ['boundary', 'pin']);
    expect(roles(const {'type': 'LineString', 'coordinates': [[0.0, 0.0], [1.0, 1.0]]}), ['boundary', 'pin']);
    expect(roles(null), ['pin']);
  });

  test('an area needs to be about 40 dp across at the zoom places are shown at', () {
    Place square(double size) => Place(
      id: 'poi:1',
      kind: 'poi',
      center: const LngLat(0, 0),
      tags: const {},
      shape: {
        'type': 'Polygon',
        'coordinates': [
          [
            [0.0, 0.0],
            [size, 0.0],
            [size, size],
            [0.0, 0.0],
          ],
        ],
      },
    );
    // At the equator and z14, 40 dp is 360 * 40 / (512 * 2^14) degrees.
    const edge = 360 * areaMinSize / (512 * 16384);
    expect(showsAsArea(square(edge * 1.1)), isTrue);
    expect(showsAsArea(square(edge * 0.9)), isFalse);
  });

  test('showing a place zooms in to it, but never out', () {
    double zoomFor(double zoom) {
      final map = _ZoomedMap(zoom);
      focusPlace(map, _place);
      return map.subjectMaxZoom!;
    }

    expect(zoomFor(5), placeZoom);
    expect(zoomFor(placeZoom), placeZoom);
    expect(zoomFor(17), 17);
  });

  test('before the map is set up, it uses the usual zoom', () {
    final map = FakeMap();
    focusPlace(map, _place);
    expect(map.subjectMaxZoom, placeZoom);
  });
}
