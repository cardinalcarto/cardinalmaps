import 'package:cardinal/directions/callouts.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_routes.dart';

bool everywhere(double lng, double lat) => true;

void main() {
  // Two routes sharing their ends: one goes straight up the middle, and the other bulges
  // out to the east halfway along.
  final straight = fakeRoute([
    [0, 0],
    [0, 0.05],
    [0, 0.1],
  ]);
  final bulging = fakeRoute([
    [0, 0],
    [0.02, 0.05],
    [0, 0.1],
  ]);

  test('a callout goes where its route is farthest from the others', () {
    final callouts = CalloutPlacer([straight, bulging]).place(everywhere);
    expect(callouts.map((c) => c.route), [0, 1]);
    // The bulge is widest halfway.
    expect(callouts[1].lng, closeTo(0.02, 0.002));
    expect(callouts[1].lat, closeTo(0.05, 0.003));
    // The straight route's farthest point from the bulge is level with it.
    expect(callouts[0].lng, 0);
    expect(callouts[0].lat, closeTo(0.05, 0.003));
  });

  test('a callout stays in view', () {
    // Only the part of the map south of the bulge's widest point shows.
    final callouts = CalloutPlacer([straight, bulging])
        .place((lng, lat) => lat < 0.03);
    expect(callouts, hasLength(2));
    expect(callouts[1].lat, lessThan(0.03));
    expect(callouts[1].lat, greaterThan(0.025));
  });

  test('a route with none of it in view has no callout', () {
    expect(CalloutPlacer([straight]).place((lng, lat) => lat > 1), isEmpty);
  });

  test('a lone route is labeled in the middle', () {
    final callouts = CalloutPlacer([straight]).place(everywhere);
    expect(callouts.single.lat, closeTo(0.05, 0.001));
  });

  test('routes on the same road are labeled in the middle', () {
    final callouts = CalloutPlacer([straight, straight]).place(everywhere);
    expect(callouts, hasLength(2));
    expect(callouts[0].lat, closeTo(0.05, 0.001));
  });

  test('separation counts between vertices, not only at them', () {
    // The same road with different points along it: Murena's exact matching would call
    // these entirely separate, so its label could land anywhere.
    final coarse = fakeRoute([
      [0, 0],
      [0, 0.1],
    ]);
    final fine = fakeRoute([
      [0, 0],
      [0, 0.033],
      [0, 0.066],
      [0, 0.1],
    ]);
    final callouts = CalloutPlacer([coarse, fine]).place(everywhere);
    expect(callouts[0].lat, closeTo(0.05, 0.001));
    expect(callouts[1].lat, closeTo(0.05, 0.001));
  });

  test(
    'a long gentle slope away from the farthest point does not pull it off',
    () {
      // Farthest from the other route early on, then closing on it by less than the
      // tolerance from one sample to the next, all the way past the route's middle.
      final road = fakeRoute([
        [0, 0],
        [0, 0.1],
      ]);
      final drifting = fakeRoute([
        [0, 0],
        [0.0005, 0.02],
        [0, 0.1],
      ]);
      final callouts = CalloutPlacer([road, drifting]).place(everywhere);
      expect(callouts[1].lat, closeTo(0.02, 0.002));
    },
  );

  test('routes without geometry have no callouts', () {
    expect(CalloutPlacer([fakeRoute([])]).place(everywhere), isEmpty);
  });

  test('being inside an outline of the ground, however the map is turned', () {
    const turned = [
      (lng: 0.0, lat: 1.0),
      (lng: 1.0, lat: 0.0),
      (lng: 0.0, lat: -1.0),
      (lng: -1.0, lat: 0.0),
    ];
    expect(insideOutline(turned, 0, 0), isTrue);
    expect(insideOutline(turned, 0.4, 0.4), isTrue);
    expect(insideOutline(turned, 0.8, 0.8), isFalse);
  });
}
