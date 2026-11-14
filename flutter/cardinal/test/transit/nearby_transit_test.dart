import 'dart:convert';

import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/transit/nearby_source.dart';
import 'package:cardinal/transit/nearby_transit.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_nearby.dart';

void main() {
  test('reads a nearby answer as the router writes it', () {
    final nearby = NearbyTransit.decode(
      utf8.encode(jsonEncode(snapshotNearbyJson())),
    );
    expect(nearby.realtime.level, RealtimeLevel.realtime);
    expect(
      (nearby.realtime.feedsApplied, nearby.realtime.feedsReported),
      (1, 1),
    );
    expect(nearby.noRoutesReason, isNull);
    expect(nearby.generatedAt.millisecondsSinceEpoch, 1781535015 * 1000);

    // Rows in the router's order, soonest first.
    expect(nearby.routes.map((r) => r.route.id), ['R4', 'R5', 'R2', 'R1']);
    final r4 = nearby.routes.first;
    expect(r4.key, (feed: 'metro', routeId: 'R4'));
    expect(r4.route.name, '4');
    expect(r4.agency, 'Metro');
    final up = r4.directions.single;
    expect(up.direction, const DirectionRef.id(0));
    expect(up.headsigns, ['No Steps']);
    expect(
      (up.stop.feed, up.stop.id, up.stop.name),
      ('metro', 'N1', 'North 1st'),
    );
    expect(up.stop.accessibility, Accessibility.accessible);
    expect(up.stop.point!.lat, closeTo(47.6005, 1e-9));
    expect(up.walkSeconds, 42);
    final first = up.departures.single;
    expect((first.tripId, first.startDate), ('R4-0801', '20260615'));
    expect(first.status, DepartureStatus.scheduled);
    expect(first.departure.millisecondsSinceEpoch, 1781535690 * 1000);
    expect(first.accessibility, Accessibility.inaccessible);
    expect(first.alerts, isEmpty);

    // Directions without a direction_id go by their last stop; a canceled departure
    // can come first.
    final r2 = nearby.routes[2];
    expect(r2.directions.map((d) => d.direction), [
      const DirectionRef.terminus('E1'),
      const DirectionRef.terminus('EX'),
      const DirectionRef.terminus('WX'),
    ]);
    expect(r2.directions[1].departures.map((d) => d.status), [
      DepartureStatus.canceled,
      DepartureStatus.scheduled,
    ]);

    // A live departure, with the answer's alert it indexes.
    final r1 = nearby.routes.last;
    final live = r1.directions.first.departures.first;
    expect(live.status, DepartureStatus.live);
    expect(live.realtime, isTrue);
    expect(live.delaySeconds, 120);
    expect(
      live.departure.difference(live.scheduledDeparture!),
      const Duration(minutes: 2),
    );
    expect(live.alerts.single.header, 'Stop relocated');
    expect(live.alerts.single.effect, 'detour');
    expect(r1.directions.first.headsigns, ['Northgate', 'Northgate via 2nd']);
  });

  test('reads why a nearby answer has no routes', () {
    expect(fakeNearby(const []).noRoutesReason, NoRoutesReason.noService);
    expect(NoRoutesReason.parse('no_access'), NoRoutesReason.noAccess);
  });

  test('reads a route detail as the router writes it', () {
    final detail = RouteDetail.decode(
      utf8.encode(jsonEncode(snapshotRouteDetailJson())),
    );
    expect(detail.realtime.level, RealtimeLevel.realtime);
    expect(
      (detail.feed, detail.route.id, detail.route.color),
      ('metro', 'R1', 'FDB71A'),
    );
    expect(detail.agency, 'Metro');
    expect(detail.direction, const DirectionRef.id(0));
    expect((detail.stop.id, detail.walkSeconds), ('N1', 42));
    expect(detail.headsigns, ['Northgate', 'Northgate via 2nd']);
    // The canceled one is beside the two that run.
    expect(detail.departures.map((d) => (d.tripId, d.status)), [
      ('R1N-0810', DepartureStatus.live),
      ('R1N-0820', DepartureStatus.canceled),
      ('R1N-0830', DepartureStatus.scheduled),
    ]);
    final live = detail.departures.first;
    expect((live.vehicleId, live.delaySeconds), ('7033', 120));
    expect(live.alerts.single.header, 'Stop relocated');
    final ride = detail.ride!;
    expect(ride.departure, live.departure);
    expect(ride.stops.map((s) => s.stop.id), ['N1', 'N2', 'NF']);
    expect(ride.tripState, 'delayed');
    expect(ride.vehicle!.id, '7033');
    expect(ride.shape, isNotEmpty);
    final vehicle = detail.vehicles.single;
    expect((vehicle.id, vehicle.tripId), ('7033', 'R1N-0810'));
    expect(vehicle.point!.lat, closeTo(47.585, 1e-4));
  });

  test('a route detail with nothing in the horizon has no ride', () {
    final json = snapshotRouteDetailJson()
      ..remove('ride')
      ..remove('vehicles')
      ..['departures'] = <Object>[];
    final detail = RouteDetail.fromJson(json);
    expect(detail.ride, isNull);
    expect(detail.departures, isEmpty);
    expect(detail.vehicles, isEmpty);
  });

  test('asks for nearby departures from a place, with the fixed limits', () {
    expect(
      nearbyRequest(const LngLat(-122.35, 47.65), TransitAccess.wheelchair),
      {
        'location': {'lat': 47.65, 'lon': -122.35},
        'mode': 'wheelchair',
        'max_walk_seconds': 1800,
        'horizon_seconds': 7200,
      },
    );
    expect(nearbyAccess(wheelchair: true), TransitAccess.wheelchair);
    expect(nearbyAccess(wheelchair: false), TransitAccess.walking);
  });

  test('asks for a route detail by what the nearby answer gave', () {
    final nearby = NearbyTransit.fromJson(snapshotNearbyJson());
    final r2 = nearby.routes[2];
    final east = r2.directions[1];
    expect(routeRequest(RouteDetailQuery.of(r2, east), TransitAccess.walking), {
      'feed': 'metro',
      'route_id': 'R2',
      'terminus_stop_id': 'EX',
      'stop_id': 'N1',
      'mode': 'walking',
      'walk_seconds': 42,
      'horizon_seconds': 7200,
    });
    final r1 = nearby.routes.last;
    final north = r1.directions.first;
    expect(
      routeRequest(
        RouteDetailQuery.of(r1, north, departure: north.departures[1]),
        TransitAccess.wheelchair,
      ),
      {
        'feed': 'metro',
        'route_id': 'R1',
        'direction_id': 0,
        'stop_id': 'N1',
        'mode': 'wheelchair',
        'walk_seconds': 42,
        'horizon_seconds': 7200,
        'trip_id': 'R1N-0820',
        'start_date': '20260615',
      },
    );
  });
}
