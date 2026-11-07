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
