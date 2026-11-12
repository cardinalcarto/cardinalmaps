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
}
