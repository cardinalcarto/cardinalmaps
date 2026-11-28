import 'package:cardinal/carplay/car_map.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/navigation/navigation_camera.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart' show EdgeInsets;
import 'package:routing/routing.dart';

import '../fakes/fake_map.dart';

const line = [
  DirectionsPoint(lat: 0, lng: 0),
  DirectionsPoint(lat: 0, lng: 1),
  DirectionsPoint(lat: 0, lng: 2),
  DirectionsPoint(lat: 0, lng: 3),
];

/// What the phone says of a drive: you at [lng] on the line, past point [index].
Map<Object?, Object?> at(
  double lng,
  int index, {
  double? speed,
  String state = 'onRoute',
}) => {
  'state': state,
  'mode': 'driving',
  'at': {'lat': 0.0, 'lng': lng, 'bearing': 90.0, 'speed': speed},
  'snapped': {'lat': 0.0, 'lng': lng},
  'index': index,
};

List<List<double>> coordinates(FakeMap map, String role) {
  final features = ((map.geoJson[navigationSource]! as Map)['features'] as List)
      .cast<Map<String, Object?>>();
  final feature = features.where(
    (f) => (f['properties']! as Map)['role'] == role,
  );
  if (feature.isEmpty) return const [];
  return [
    for (final c in (feature.single['geometry']! as Map)['coordinates'] as List)
      [for (final v in c as List) (v as num).toDouble()],
  ];
}

void main() {
  late FakeMap map;
  late CarMap drawn;
  var now = Duration.zero;

  setUp(() {
    map = FakeMap();
    now = Duration.zero;
    drawn = CarMap(map, elapsed: () => now)..start();
  });

  test("the chevron has the car's look, and is hidden with no trip", () {
    expect(map.puckStyle, carPuckStyle);
    expect(map.puckVisible, isFalse);
  });

  test('draws the route ahead, and the part traveled over it', () {
    drawn
      ..route(line)
      ..trip(at(1.5, 1));
    expect(coordinates(map, 'ahead'), [
      [0, 0],
      [1, 0],
      [2, 0],
      [3, 0],
    ]);
    expect(coordinates(map, 'traveled'), [
      [0, 0],
      [1, 0],
      [1.5, 0],
    ]);
    expect(map.puckVisible, isTrue);
    expect(map.pucks.single.lng, 1.5);
    expect(map.pucks.single.bearing, 90);
  });

  test('the chevron glides for as long as the messages are apart', () {
    drawn
      ..route(line)
      ..trip(at(1, 1));
    now = const Duration(seconds: 1);
    drawn.trip(at(1.1, 1));
    expect(map.pucks.map((p) => p.duration), [
      Duration.zero,
      const Duration(seconds: 1),
    ]);
  });

  test('the camera waits for the map, then follows at the driving zoom', () {
    drawn
      ..route(line)
      ..trip(at(1, 1));
    expect(map.follows, isEmpty);
    drawn.ready();
    final camera = followCamera(NavigationMode.driving);
    expect(map.follows.single.zoom, camera.zoom);
    expect(map.follows.single.pitch, camera.pitch);
  });

  test('a following camera is told again only when the zoom changes', () {
    drawn
      ..ready()
      ..route(line)
      ..trip(at(1, 1))
      ..trip(at(1.1, 1))
      ..trip(at(1.2, 1));
    expect(map.follows, hasLength(1));
    // 200 km/h is well past where the driving camera stops zooming out.
    drawn.trip(at(1.3, 1, speed: 200 / 3.6));
    expect(map.follows, hasLength(2));
    expect(map.follows.last.zoom, followCamera(NavigationMode.driving).minZoom);
  });

  test('follows with the padding it is given', () {
    const padding = EdgeInsets.only(top: 120, left: 200);
    drawn
      ..ready()
      ..padding = padding
      ..route(line)
      ..trip(at(1, 1));
    expect(map.follows.single.padding, padding);
  });

  test('overview shows the rest of the trip, until it is recentered', () {
    drawn
      ..ready()
      ..route(line)
      ..trip(at(1.5, 1))
      ..overview();
    expect(map.subjectBounds, (west: 1.5, south: 0.0, east: 3.0, north: 0.0));
    expect(map.subjectBearing, 0);
    expect(map.subjectPitch, 0);
    // Moving on doesn't take the camera back.
    drawn.trip(at(1.6, 1));
    expect(map.follows, hasLength(1));
    drawn.recenter();
    expect(map.follows, hasLength(2));
  });

  test('once arrived, shows the destination without the chevron', () {
    drawn
      ..ready()
      ..route(line)
      ..trip(at(2.9, 2))
      // The session gives no position once the trip is complete.
      ..trip({'state': 'arrived', 'mode': 'driving'});
    expect(map.puckVisible, isFalse);
    expect(map.subjectBounds, (west: 3.0, south: 0.0, east: 3.0, north: 0.0));
    expect(map.subjectsSet, 1);
    drawn.trip({'state': 'arrived', 'mode': 'driving'});
    expect(map.subjectsSet, 1);
  });

  test('a car that connects after the arrival shows it too', () {
    drawn
      ..ready()
      ..route(line)
      ..trip({'state': 'arrived', 'mode': 'driving'});
    expect(map.subjectBounds, (west: 3.0, south: 0.0, east: 3.0, north: 0.0));
  });

  test('when the trip ends, the chevron and the line go', () {
    drawn
      ..ready()
      ..route(line)
      ..trip(at(1, 1))
      ..trip(null);
    expect(map.puckVisible, isFalse);
    expect(map.following.value, isFalse);
    expect(map.geoJson[navigationSource], empty);
  });
}
