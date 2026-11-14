import 'dart:async';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/transit/nearby_rows.dart';
import 'package:cardinal/transit/nearby_transit.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_nearby.dart';

/// Fremont, Seattle.
const here = LngLat(-122.35, 47.65);

/// [meters] north of [here]: a degree of latitude is about 111,195 m.
LngLat north(double meters) => LngLat(here.lon, here.lat + meters / 111195);

const walk = TransitAccess.walking;

List<String> ids(NearbyRows rows) => [for (final r in rows.routes) r.route.id];

const feed = 'us-wa-Metro-Transit';
const RouteKey key44 = (feed: feed, routeId: '44');

HeadsignStop stop44(String stop) => (feed: feed, routeId: '44', stopId: stop);

RememberedHeadsign remembered(String headsign) =>
    RememberedHeadsign(headsign, DateTime(2026, 11, 13));

/// Route 44 in the U District: to Ballard from Stop A, and to UW Medical Center from
/// Stop B, leaving in [ballard] and [uw] minutes.
Map<String, Object?> u44({required int ballard, required int uw}) =>
    nearbyRoutePagesJson('44', [
      (headsigns: ['Ballard'], minutes: [ballard], stop: 'Stop A'),
      (headsigns: ['UW Medical Center'], minutes: [uw], stop: 'Stop B'),
    ]);

NearbyRoute route44({required int ballard, required int uw}) =>
    fakeNearby([u44(ballard: ballard, uw: uw)]).routes.single;

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

    test('4. shown, show: the row shows the page, and nothing moves', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([
            u44(ballard: 3, uw: 9),
            nearbyRouteJson('62', minutes: [6]),
          ]),
          origin: here,
          access: walk,
        );
      expect(ids(rows), ['44', '62']);
      final page = rows.show(key44, 1);
      expect(page?.headsigns, ['UW Medical Center']);
      expect(rows.shown(key44), 1);
      // 44 now shows 9 minutes above 62's 6: it stays until the next sort.
      expect(ids(rows), ['44', '62']);
      rows.apply(
        fakeNearby([
          u44(ballard: 3, uw: 9),
          nearbyRouteJson('62', minutes: [6]),
        ]),
        origin: north(20),
        access: walk,
      );
      expect(ids(rows), ['44', '62']);
      expect(rows.shown(key44), 1);
    });

    test('2. a sort orders rows by the page each shows', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([
            u44(ballard: 3, uw: 9),
            nearbyRouteJson('62', minutes: [6]),
          ]),
          origin: here,
          access: walk,
        )
        ..show(key44, 1);
      rows.apply(
        fakeNearby([
          u44(ballard: 3, uw: 9),
          nearbyRouteJson('62', minutes: [6]),
        ]),
        origin: north(150),
        access: walk,
      );
      // 44 shows UW Medical Center, in 9 minutes: below 62's 6.
      expect(ids(rows), ['62', '44']);
      expect(rows.shown(key44), 1);
    });

    test('1. a remembered page sorts by its own departure', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([
            u44(ballard: 3, uw: 9),
            nearbyRouteJson('62', minutes: [6]),
          ]),
          origin: here,
          access: walk,
          remembered: {stop44('Stop B'): remembered('UW Medical Center')},
        );
      expect(rows.shown(key44), 1);
      expect(ids(rows), ['62', '44']);
    });

    test('3c. new rows open on their pages and are ordered by them', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([nearbyRouteJson('40')]),
          origin: here,
          access: walk,
        );
      rows.apply(
        fakeNearby([
          u44(ballard: 3, uw: 9),
          nearbyRouteJson('62', minutes: [6]),
          nearbyRouteJson('40'),
        ]),
        origin: here,
        access: walk,
        remembered: {stop44('Stop B'): remembered('UW Medical Center')},
      );
      expect(ids(rows), ['40', '62', '44']);
      expect(rows.shown(key44), 1);
    });

    test('3a. a row keeps its page by a shared headsign', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([u44(ballard: 3, uw: 9)]),
          origin: here,
          access: walk,
        )
        ..show(key44, 1);
      // A new page comes before UW Medical Center, which now shares a page with
      // University District, and Ballard is sooner.
      rows.apply(
        fakeNearby([
          nearbyRoutePagesJson('44', [
            (headsigns: ['Ballard'], minutes: [1], stop: 'Stop A'),
            (headsigns: ['Fremont'], minutes: [2], stop: 'Stop A'),
            (
              headsigns: ['University District', 'UW Medical Center'],
              minutes: [8],
              stop: 'Stop C',
            ),
          ]),
        ]),
        origin: here,
        access: walk,
      );
      expect(rows.shown(key44), 2);
    });

    test('3a. a row whose page is gone opens again', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([u44(ballard: 3, uw: 9)]),
          origin: here,
          access: walk,
        )
        ..show(key44, 1);
      rows.apply(
        fakeNearby([
          nearbyRoutePagesJson('44', [
            (headsigns: ['Ballard'], minutes: [7], stop: 'Stop A'),
            (headsigns: ['Fremont'], minutes: [2], stop: 'Stop A'),
          ]),
        ]),
        origin: here,
        access: walk,
      );
      // Soonest, with nothing remembered.
      expect(rows.shown(key44), 1);
      expect(rows.shownPage(rows.routes.single)!.headsigns, ['Fremont']);
    });

    test('show ignores a page or a row that is not there', () {
      final rows = NearbyRows()
        ..apply(
          fakeNearby([u44(ballard: 3, uw: 9)]),
          origin: here,
          access: walk,
        );
      expect(rows.show(key44, 2), isNull);
      expect(rows.show((feed: feed, routeId: '5'), 0), isNull);
      expect(rows.shown(key44), 0);
    });
  });

  group('openingPage', () {
    NearbyRoute route(List<FakePage> pages) =>
        fakeNearby([nearbyRoutePagesJson('44', pages)]).routes.single;

    test('is the soonest without a remembered headsign', () {
      expect(openingPage(route44(ballard: 9, uw: 3), const {}), 1);
      expect(openingPage(route44(ballard: 3, uw: 9), const {}), 0);
    });

    test('is the first of the soonest when two leave together', () {
      expect(openingPage(route44(ballard: 4, uw: 4), const {}), 0);
    });

    test('is the remembered page, however late', () {
      expect(
        openingPage(route44(ballard: 50, uw: 3), {
          stop44('Stop A'): remembered('Ballard'),
        }),
        0,
      );
    });

    test('matches a merged page by any of its headsigns', () {
      final merged = route([
        (headsigns: ['Ballard'], minutes: [1], stop: 'Stop A'),
        (
          headsigns: ['University District', 'UW Medical Center'],
          minutes: [5],
          stop: 'Stop B',
        ),
      ]);
      expect(
        openingPage(merged, {
          stop44('Stop B'): remembered('UW Medical Center'),
        }),
        1,
      );
    });

    test('needs the headsign remembered at the page stop', () {
      expect(
        openingPage(route44(ballard: 9, uw: 3), {
          stop44('Stop B'): remembered('Ballard'),
        }),
        1,
      );
    });

    test('takes the most recently chosen of several', () {
      final at = DateTime(2026, 11, 13);
      expect(
        openingPage(route44(ballard: 1, uw: 3), {
          stop44('Stop A'): RememberedHeadsign('Ballard', at),
          stop44('Stop B'): RememberedHeadsign(
            'UW Medical Center',
            at.add(const Duration(days: 1)),
          ),
        }),
        1,
      );
    });

    test('a page without headsigns is remembered as empty', () {
      final unnamed = route([
        (headsigns: ['Ballard'], minutes: [1], stop: 'Stop A'),
        (headsigns: [], minutes: [5], stop: 'Stop A'),
      ]);
      expect(rememberedHeadsign(unnamed.pages[1]), '');
      expect(openingPage(unnamed, {stop44('Stop A'): remembered('')}), 1);
    });
  });

  group('NearbyTransitController', () {
    test('refresh asks from the origin and applies the answer', () async {
      final source = FakeNearbySource(
        immediately: fakeNearby([nearbyRouteJson('40')]),
      );
      final controller = NearbyTransitController(
        source,
        headsigns: HeadsignStore.inMemory(),
      );
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
      final controller = NearbyTransitController(
        source,
        headsigns: HeadsignStore.inMemory(),
      );
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
      final controller = NearbyTransitController(
        source,
        headsigns: HeadsignStore.inMemory(),
      );
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
      final controller = NearbyTransitController(
        source,
        headsigns: HeadsignStore.inMemory(),
      );
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

    test(
      'abandon keeps the rows, and drops the answer being waited for',
      () async {
        final source = FakeNearbySource(
          immediately: fakeNearby([nearbyRouteJson('40')]),
        );
        final controller = NearbyTransitController(
          source,
          headsigns: HeadsignStore.inMemory(),
        );
        await controller.refresh(origin: here, access: walk);
        source.immediately = null;
        final waiting = controller.refresh(origin: here, access: walk);
        controller.abandon();
        await waiting;
        source.asked.last.answer.complete(fakeNearby([nearbyRouteJson('62')]));
        await pumpEventQueue();
        expect(ids(controller.rows), ['40']);
        controller.dispose();
      },
    );

    test('opens rows on the remembered headsigns', () async {
      final headsigns = HeadsignStore.inMemory();
      await headsigns.remember(
        stop44('Stop B'),
        'UW Medical Center',
        chosenAt: DateTime(2026, 11, 13),
      );
      final controller = NearbyTransitController(
        FakeNearbySource(immediately: fakeNearby([u44(ballard: 3, uw: 9)])),
        headsigns: headsigns,
      );
      await controller.refresh(origin: here, access: walk);
      expect(controller.rows.shown(key44), 1);
      controller.dispose();
    });

    test('show remembers the headsign at the page stop', () async {
      final headsigns = HeadsignStore.inMemory();
      final at = DateTime(2026, 11, 13, 9);
      final controller = NearbyTransitController(
        FakeNearbySource(immediately: fakeNearby([u44(ballard: 3, uw: 9)])),
        headsigns: headsigns,
        clock: () => at,
      );
      await controller.refresh(origin: here, access: walk);
      var told = 0;
      controller.addListener(() => told++);
      controller.show(key44, 1);
      expect(told, 1);
      expect(controller.rows.shown(key44), 1);
      await pumpEventQueue();
      expect(await headsigns.recall([stop44('Stop B'), stop44('Stop A')]), {
        stop44('Stop B'): RememberedHeadsign('UW Medical Center', at),
      });
      controller.dispose();
    });

    test('answers on the schedule first, then asks again once real-time data '
        'arrives, in place', () async {
      final source = FakeNearbySource();
      final controller = NearbyTransitController(
        source,
        headsigns: HeadsignStore.inMemory(),
      );
      final landing = source.realtimeLanding = Completer<bool>();
      final refreshing = controller.refresh(origin: here, access: walk);
      source.asked.single.answer.complete(
        fakeNearby([
          nearbyRouteJson('40', minutes: [3]),
          nearbyRouteJson('62', minutes: [8]),
        ]),
      );
      await refreshing;
      expect(ids(controller.rows), ['40', '62']);
      expect(source.asked, hasLength(1));

      landing.complete(true);
      await pumpEventQueue();
      expect(source.asked, hasLength(2));
      expect(source.asked.last.location, here);
      // Live: 40 is late, and leaves after 62; the rows stay where they are.
      source.asked.last.answer.complete(
        fakeNearby([
          nearbyRouteJson('62', minutes: [8]),
          nearbyRouteJson('40', minutes: [10]),
        ]),
      );
      await pumpEventQueue();
      expect(ids(controller.rows), ['40', '62']);
      expect(
        controller.rows.routes.first.pages.single.departures.single.tripId,
        '40-10',
      );
      // The live answer had nothing on its way: no more questions.
      await pumpEventQueue();
      expect(source.asked, hasLength(2));
      controller.dispose();
    });

    test('real-time data that brought nothing asks nothing', () async {
      final source = FakeNearbySource(
        immediately: fakeNearby([nearbyRouteJson('40')]),
      );
      final controller = NearbyTransitController(
        source,
        headsigns: HeadsignStore.inMemory(),
      );
      final landing = source.realtimeLanding = Completer<bool>();
      await controller.refresh(origin: here, access: walk);
      landing.complete(false);
      await pumpEventQueue();
      expect(source.asked, hasLength(1));
      controller.dispose();
    });

    test(
      'a question asked meanwhile takes the real-time data instead',
      () async {
        final source = FakeNearbySource(
          immediately: fakeNearby([nearbyRouteJson('40')]),
        );
        final controller = NearbyTransitController(
          source,
          headsigns: HeadsignStore.inMemory(),
        );
        final landing = source.realtimeLanding = Completer<bool>();
        await controller.refresh(origin: here, access: walk);
        await controller.refresh(origin: here, access: walk);
        landing.complete(true);
        await pumpEventQueue();
        expect(source.asked, hasLength(2));
        controller.dispose();
      },
    );
  });
}
