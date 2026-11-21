import 'dart:convert';

import 'package:cardinal/directions/transit_plan.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_transit.dart';

void main() {
  test('reads a plan as the router writes it', () {
    final plan = TransitPlan.decode(utf8.encode(jsonEncode(fakePlanJson())));
    expect(plan.realtime, RealtimeLevel.degraded);
    expect((plan.feedsApplied, plan.feedsReported), (1, 2));
    final journey = plan.journeys.single;
    expect(journey.transfers, 1);
    expect(journey.legs.map((leg) => leg.runtimeType), [
      StreetLeg,
      TransitLeg,
      StreetLeg,
      TransitLeg,
      StreetLeg,
    ]);
    final walk = journey.legs.first as StreetLeg;
    expect(walk.kind, StreetLegKind.access);
    expect(walk.from, isNull);
    expect(walk.to!.name, 'Fremont Ave N & N 34th St');
    expect(walk.stopMoved.single.atEnd, isTrue);
    expect(walk.stopMoved.single.description, contains('50 feet north'));
    expect(walk.shape, hasLength(2));
    expect(walk.shape.last.lat, closeTo(2.1, 1e-6));

    final bus = journey.rides.first;
    expect(bus.route!.kind, VehicleKind.bus);
    expect(bus.boarding.departureDelay, 240);
    expect(bus.segments.map((s) => s.route!.shortName), ['40', '7']);
    expect(bus.alerts.single.header, 'Route 40 detoured');
    // A ride with no shape is drawn from stop to stop.
    expect(bus.shape, hasLength(3));
    expect(journey.rides.last.route!.kind, VehicleKind.tram);
    expect(journey.accessibility, Accessibility.unknown);
  });

  test('reads a ride\'s departures, with their alerts resolved', () {
    final bus = fakePlan().journeys.single.rides.first;
    final departures = bus.departures;
    expect(departures.map((d) => d.tripId), [
      't40a',
      't40b',
      't40',
      't40c',
      't40d',
      't40e',
    ]);
    expect(departures.map((d) => d.missed), [
      true,
      true,
      false,
      false,
      false,
      false,
    ]);
    expect(bus.suggested!.tripId, 't40');
    expect(bus.suggested!.status, DepartureStatus.live);
    expect(bus.suggested!.delaySeconds, 240);
    // Indices into the leg's own alerts and into its departures' pool.
    expect(bus.suggested!.alerts.single.header, 'Route 40 detoured');
    expect(departures[3].alerts.single.header, 'Trip 40c ends at Westlake');
    expect(departures[3].accessibility, Accessibility.inaccessible);
    expect(departures[4].status, DepartureStatus.canceled);
    expect(departures[4].status.runs, isFalse);
    expect(departures.last.lastOfDay, isTrue);
    expect(fakePlan().journeys.single.rides.last.suggested!.lastOfDay, isTrue);
  });

  test('real-time feeds are counted by feed, not by message', () {
    final json = fakePlanJson();
    final realtime = json['realtime']! as Map<String, Object?>;
    final feeds = realtime['feeds']! as List;
    final metro = feeds.first as Map<String, Object?>;
    // Metro sends alerts and vehicles too; Sound Transit's one message is stale.
    realtime['feeds'] = [
      ...feeds,
      {...metro, 'kind': 'alerts'},
      {...metro, 'kind': 'vehicle_positions', 'state': 'stale'},
    ];
    final plan = TransitPlan.fromJson(json);
    expect((plan.feedsApplied, plan.feedsReported), (1, 2));
  });

  test('a plan with no journeys says why', () {
    final plan = emptyPlan('slower_than_direct');
    expect(plan.journeys, isEmpty);
    expect(plan.noJourneysReason, NoJourneysReason.slowerThanDirect);
  });

  test('polyline6 decodes negative and large steps', () {
    final points = [
      [-122.349, 47.651],
      [-122.3001, 47.6131],
      [-122.296, 47.4489],
    ];
    final decoded = decodePolyline6(polyline6(points));
    expect(decoded, hasLength(3));
    for (final (i, p) in decoded.indexed) {
      expect(p.lng, closeTo(points[i][0], 1e-6));
      expect(p.lat, closeTo(points[i][1], 1e-6));
    }
  });

  test('vehicle kinds come from basic and extended route types', () {
    expect(VehicleKind.of(3), VehicleKind.bus);
    expect(VehicleKind.of(0), VehicleKind.tram);
    expect(VehicleKind.of(4), VehicleKind.ferry);
    expect(VehicleKind.of(2), VehicleKind.rail);
    expect(VehicleKind.of(109), VehicleKind.rail);
    expect(VehicleKind.of(700), VehicleKind.bus);
    expect(VehicleKind.of(1000), VehicleKind.ferry);
  });

  group('which journey a journey is', () {
    List<Journey> journeysOf(List<String> tags, {List<int>? shifts}) =>
        fakePlan(journeys: tags.length, tags: tags, shifts: shifts).journeys;

    test('is its rides\' trips and stops, whatever its times', () {
      final [x, y] = journeysOf(['-x', '-y']);
      final [later] = journeysOf(['-x'], shifts: [240]);
      expect(JourneyIdentity.of(later), JourneyIdentity.of(x));
      expect(
        JourneyIdentity.of(later).hashCode,
        JourneyIdentity.of(x).hashCode,
      );
      expect(JourneyIdentity.of(y), isNot(JourneyIdentity.of(x)));
      expect(JourneyIdentity.of(x).rides, [
        (
          tripId: 't40-x',
          startDate: null,
          board: 'Fremont Ave N & N 34th St',
          alight: 'Westlake Station',
          route: null,
          scheduled: null,
        ),
        (
          tripId: 'link-x',
          startDate: null,
          board: 'Westlake Station Bay 1',
          alight: 'Capitol Hill Station',
          route: null,
          scheduled: null,
        ),
      ]);
    });

    test('differs with the service day, a stop, or the order of the rides', () {
      Map<String, Object?> json() => fakePlanJson(tags: ['-x']);
      Journey read(Map<String, Object?> plan) =>
          TransitPlan.fromJson(plan).journeys.single;
      List<Map<String, Object?>> legs(Map<String, Object?> plan) =>
          (((plan['journeys']! as List).single as Map)['legs'] as List)
              .cast<Map<String, Object?>>();
      final plain = JourneyIdentity.of(read(json()));

      final nextDay = json();
      (legs(nextDay)[1]['trip']! as Map)['start_date'] = '20261110';
      expect(JourneyIdentity.of(read(nextDay)), isNot(plain));

      final otherStop = json();
      ((legs(otherStop)[1]['stops']! as List).last as Map)['stop_id'] = 'Pine';
      expect(JourneyIdentity.of(read(otherStop)), isNot(plain));

      final swapped = json();
      final all = legs(swapped);
      final bus = all[1];
      all[1] = all[3];
      all[3] = bus;
      (((swapped['journeys']! as List).single as Map))['legs'] = all;
      expect(JourneyIdentity.of(read(swapped)), isNot(plain));
    });

    test('a ride with no trip named is told by its route and its schedule', () {
      Journey unnamed({required int delay}) {
        final plan = fakePlanJson(tags: ['-x']);
        final legs =
            ((plan['journeys']! as List).single as Map)['legs'] as List;
        final bus = legs[1] as Map;
        (bus['trip']! as Map).remove('trip_id');
        // Running [delay] later than it was, on the same schedule.
        final boarding = (bus['stops']! as List).first as Map;
        boarding['departure'] = (boarding['departure']! as int) + delay;
        return TransitPlan.fromJson(plan).journeys.single;
      }

      final ride = JourneyIdentity.of(unnamed(delay: 0)).rides.first;
      expect(ride.tripId, isNull);
      expect(ride.route, '100040');
      expect(ride.scheduled, isNotNull);
      expect(
        JourneyIdentity.of(unnamed(delay: 120)),
        JourneyIdentity.of(unnamed(delay: 0)),
      );
    });

    test('journeys with no rides are all the same one', () {
      Journey onFoot(int leaves) => TransitPlan.fromJson({
        ...fakePlanJson(),
        'journeys': [
          {
            'departure': leaves,
            'arrival': leaves + 600,
            'duration_seconds': 600,
            'trips': 0,
            'legs': <Object>[],
          },
        ],
      }).journeys.single;
      expect(
        JourneyIdentity.of(onFoot(fakeStart)),
        JourneyIdentity.of(onFoot(fakeStart + 300)),
      );
    });

    test('a plan\'s keys tell apart journeys that are the same', () {
      final keys = journeyKeys(journeysOf(['-x', '-x', '-y']));
      expect(keys.toSet(), hasLength(3));
      expect(keys.map((key) => key.nth), [0, 1, 0]);
      expect(keys[0].identity, keys[1].identity);
      // Each is found again in a newer plan, in its turn.
      expect(journeyKeys(journeysOf(['-y', '-x', '-x'])).indexOf(keys[1]), 2);
    });
  });
}
