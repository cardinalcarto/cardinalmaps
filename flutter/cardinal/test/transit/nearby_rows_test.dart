import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/transit/nearby_rows.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_nearby.dart';

/// Fremont, Seattle.
const here = LngLat(-122.35, 47.65);

/// [meters] north of [here]: a degree of latitude is about 111,195 m.
LngLat north(double meters) => LngLat(here.lon, here.lat + meters / 111195);

const walk = TransitAccess.walking;

List<String> ids(NearbyRows rows) => [for (final r in rows.routes) r.route.id];

void main() {
  group('NearbyRows', () {
    test('1. unsorted: the first answer sorts', () {
      final rows = NearbyRows();
      expect(rows.sortedAt, isNull);
      final answer = fakeNearby([nearbyRouteJson('40'), nearbyRouteJson('62')]);
      rows.apply(answer, origin: here, access: walk);
      expect(ids(rows), ['40', '62']);
      expect(rows.sortedAt, here);
      expect(rows.latest, same(answer));
    });

    test('2. shown, moved 100 m or more: the answer sorts again', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([nearbyRouteJson('40'), nearbyRouteJson('62')]),
          origin: here,
          access: walk,
        );
      final far = north(100.5);
      rows.apply(
        fakeNearby([
          nearbyRouteJson('5'),
          nearbyRouteJson('62'),
          nearbyRouteJson('40'),
        ]),
        origin: far,
        access: walk,
      );
      expect(ids(rows), ['5', '62', '40']);
      expect(rows.sortedAt, far);
    });

    test(
      '3a. shown, moved less: rows keep their order and update in place',
      () {
        final rows = NearbyRows()
          ..apply(
            fakeNearby([
              nearbyRouteJson('40', minutes: [3]),
              nearbyRouteJson('62', minutes: [8]),
            ]),
            origin: here,
            access: walk,
          );
        // 62 now leaves first, from another stop; 40 is late.
        rows.apply(
          fakeNearby([
            nearbyRouteJson('62', minutes: [1], stop: 'N 36th St'),
            nearbyRouteJson('40', minutes: [6]),
          ]),
          origin: north(99),
          access: walk,
        );
        expect(ids(rows), ['40', '62']);
        final [forty, sixtyTwo] = rows.routes;
        expect(forty.pages.single.departures.single.tripId, '40-6');
        expect(sixtyTwo.pages.single.stop.name, 'N 36th St');
        expect(sixtyTwo.pages.single.departures.single.tripId, '62-1');
        expect(rows.sortedAt, here);
      },
    );

    test('3b. shown, moved less: a route that left the answer is removed', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([
            nearbyRouteJson('40'),
            nearbyRouteJson('62'),
            nearbyRouteJson('5'),
          ]),
          origin: here,
          access: walk,
        );
      rows.apply(
        fakeNearby([nearbyRouteJson('5'), nearbyRouteJson('40')]),
        origin: here,
        access: walk,
      );
      expect(ids(rows), ['40', '5']);
    });

    test('3c. shown, moved less: a new route is added at the end', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([nearbyRouteJson('40'), nearbyRouteJson('62')]),
          origin: here,
          access: walk,
        );
      rows.apply(
        fakeNearby([
          nearbyRouteJson('31'),
          nearbyRouteJson('62'),
          nearbyRouteJson('28'),
          nearbyRouteJson('40'),
        ]),
        origin: north(50),
        access: walk,
      );
      expect(ids(rows), ['40', '62', '31', '28']);
    });

    test('3. rows are keyed by feed as well as route_id', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([
            nearbyRouteJson('1', feed: 'a'),
            nearbyRouteJson('1', feed: 'b'),
          ]),
          origin: here,
          access: walk,
        );
      rows.apply(
        fakeNearby([
          nearbyRouteJson('1', feed: 'b', minutes: [2]),
        ]),
        origin: here,
        access: walk,
      );
      expect(rows.routes.single.feed, 'b');
      expect(rows.routes.single.pages.single.departures.single.tripId, '1-2');
    });

    test('3. small moves add up from the last sort, not the last answer', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([nearbyRouteJson('40'), nearbyRouteJson('62')]),
          origin: here,
          access: walk,
        );
      final reversed = fakeNearby([
        nearbyRouteJson('62'),
        nearbyRouteJson('40'),
      ]);
      rows.apply(reversed, origin: north(60), access: walk);
      expect(ids(rows), ['40', '62']);
      rows.apply(reversed, origin: north(120), access: walk);
      expect(ids(rows), ['62', '40']);
      expect(rows.sortedAt, north(120));
    });

    test('2. shown, by another access: the answer sorts again', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([nearbyRouteJson('40'), nearbyRouteJson('62')]),
          origin: here,
          access: walk,
        );
      rows.apply(
        fakeNearby([nearbyRouteJson('62'), nearbyRouteJson('40')]),
        origin: here,
        access: TransitAccess.wheelchair,
      );
      expect(ids(rows), ['62', '40']);
      expect(rows.sortedBy, TransitAccess.wheelchair);
    });

    test('4. reset: the next answer sorts', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([nearbyRouteJson('40'), nearbyRouteJson('62')]),
          origin: here,
          access: walk,
        )
        ..reset();
      expect(rows.routes, isEmpty);
      expect(rows.latest, isNull);
      expect(rows.sortedAt, isNull);
      rows.apply(
        fakeNearby([nearbyRouteJson('62'), nearbyRouteJson('40')]),
        origin: here,
        access: walk,
      );
      expect(ids(rows), ['62', '40']);
    });
  });

  group('NearbyTransitController', () {
    test('refresh asks from the origin and applies the answer', () async {
      final source = FakeNearbySource(
        immediately: fakeNearby([nearbyRouteJson('40')]),
      );
      final controller = NearbyTransitController(source);
      var told = 0;
      controller.addListener(() => told++);
      await controller.refresh(origin: here, access: TransitAccess.wheelchair);
      expect(source.asked.single.location, here);
      expect(source.asked.single.access, TransitAccess.wheelchair);
      expect(ids(controller.rows), ['40']);
      expect(told, 1);
      controller.dispose();
    });

    test('a newer refresh abandons the one before', () async {
      final source = FakeNearbySource();
      final controller = NearbyTransitController(source);
      final first = controller.refresh(
        origin: here,
        access: TransitAccess.walking,
      );
      final second = controller.refresh(
        origin: here,
        access: TransitAccess.walking,
      );
      await first;
      source.asked.first.answer.complete(fakeNearby([nearbyRouteJson('40')]));
      source.asked.last.answer.complete(fakeNearby([nearbyRouteJson('62')]));
      await second;
      expect(ids(controller.rows), ['62']);
      controller.dispose();
    });

    test('a failed refresh leaves the rows', () async {
      final source = FakeNearbySource(
        immediately: fakeNearby([nearbyRouteJson('40')]),
      );
      final controller = NearbyTransitController(source);
      await controller.refresh(origin: here, access: TransitAccess.walking);
      source.immediately = null;
      final failing = controller.refresh(
        origin: here,
        access: TransitAccess.walking,
      );
      source.asked.last.answer.completeError(
        const TransitPlanError('timed_out', 'too slow'),
      );
      await expectLater(failing, throwsA(isA<TransitPlanError>()));
      expect(ids(controller.rows), ['40']);
      controller.dispose();
    });

    test('a refresh by another access sorts again', () async {
      final source = FakeNearbySource(
        immediately: fakeNearby([nearbyRouteJson('40'), nearbyRouteJson('62')]),
      );
      final controller = NearbyTransitController(source);
      await controller.refresh(origin: here, access: TransitAccess.walking);
      source.immediately = fakeNearby([
        nearbyRouteJson('62'),
        nearbyRouteJson('40'),
      ]);
      await controller.refresh(origin: here, access: TransitAccess.walking);
      expect(ids(controller.rows), ['40', '62']);
      await controller.refresh(origin: here, access: TransitAccess.wheelchair);
      expect(ids(controller.rows), ['62', '40']);
      controller.dispose();
    });

    test('reset forgets the rows', () async {
      final controller = NearbyTransitController(
        FakeNearbySource(immediately: fakeNearby([nearbyRouteJson('40')])),
      );
      await controller.refresh(origin: here, access: TransitAccess.walking);
      controller.reset();
      expect(controller.rows.routes, isEmpty);
      controller.dispose();
    });
  });
}
