import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/transit/nearby_source.dart';
import 'package:cardinal/transit/nearby_transit.dart';
import 'package:http/http.dart' as http;

import 'fake_transit.dart';

/// A nearby question [FakeNearbySource] was asked, waiting to be answered.
class NearbyQuestion {
  NearbyQuestion(this.location, this.access, this.abort);

  final LngLat location;
  final TransitAccess access;
  final Future<void>? abort;
  final answer = Completer<NearbyTransit>();
}

/// A route-detail question [FakeNearbySource] was asked, waiting to be answered.
class RouteQuestion {
  RouteQuestion(this.query, this.access, this.abort);

  final RouteDetailQuery query;
  final TransitAccess access;
  final Future<void>? abort;
  final answer = Completer<RouteDetail>();
}

/// Nearby departures answered by the test, or at once with [immediately] or
/// [immediateRoute] if they're set. A question whose abort completes first throws
/// [http.RequestAbortedException], as the real source does.
class FakeNearbySource implements NearbyTransitSource {
  FakeNearbySource({this.immediately, this.immediateRoute});

  NearbyTransit? immediately;
  RouteDetail? immediateRoute;
  final asked = <NearbyQuestion>[];
  final routesAsked = <RouteQuestion>[];
  var prepared = 0;

  @override
  void prepare() => prepared++;

  @override
  Future<NearbyTransit> nearby({
    required LngLat location,
    required TransitAccess access,
    Future<void>? abort,
  }) {
    final question = NearbyQuestion(location, access, abort);
    asked.add(question);
    if (immediately case final now?) question.answer.complete(now);
    return _abortable(question.answer.future, abort);
  }

  @override
  Future<RouteDetail> route({
    required RouteDetailQuery query,
    required TransitAccess access,
    Future<void>? abort,
  }) {
    final question = RouteQuestion(query, access, abort);
    routesAsked.add(question);
    if (immediateRoute case final now?) question.answer.complete(now);
    return _abortable(question.answer.future, abort);
  }

  static Future<T> _abortable<T>(Future<T> work, Future<void>? abort) =>
      abort == null
      ? work
      : Future.any([
          work,
          abort.then<T>((_) => throw http.RequestAbortedException()),
        ]);
}

/// The router's nearby answer from valinor's own test (the insta snapshot of
/// `nearby_tests.rs`), as it serializes it.
Map<String, Object?> snapshotNearbyJson() {
  final snap = File(
    '../../valinor/valinor-transit-plan/src/snapshots/'
    'valinor_transit_plan__nearby_tests__nearby_response.snap',
  ).readAsStringSync();
  // Insta's header is between the first two `---` lines.
  final body = snap.split('\n---\n').skip(1).join('\n---\n');
  return jsonDecode(body) as Map<String, Object?>;
}

/// A nearby departure's JSON, [minutes] after [fakeStart].
Map<String, Object?> nearbyDepartureJson(
  String trip,
  int minutes, {
  String status = 'scheduled',
  String? headsign,
}) => {
  'trip_id': trip,
  'start_date': '20261109',
  'headsign': ?headsign,
  'status': status,
  'departure': fakeStart + minutes * 60,
  'scheduled_departure': fakeStart + minutes * 60,
  'accessibility': 'unknown',
};

/// A nearby route's JSON, in one direction from [stop], leaving [minutes] after
/// [fakeStart].
Map<String, Object?> nearbyRouteJson(
  String routeId, {
  String feed = 'us-wa-Metro-Transit',
  String stop = 'Fremont Ave N & N 34th St',
  List<int> minutes = const [5],
  int walkSeconds = 240,
}) => {
  'feed': feed,
  'route': {'route_id': routeId, 'short_name': routeId, 'route_type': 3},
  'agency': {'name': 'Metro Transit'},
  'directions': [
    {
      'direction_id': 0,
      'headsigns': ['Downtown Seattle'],
      'stop': {
        'accessibility': 'unknown',
        'feed': feed,
        'stop_id': stop,
        'name': stop,
        'latitude': 47.65,
        'longitude': -122.35,
      },
      'walk_seconds': walkSeconds,
      'departures': [
        for (final m in minutes) nearbyDepartureJson('$routeId-$m', m),
      ],
    },
  ],
};

/// A nearby answer listing [routes] in order.
NearbyTransit fakeNearby(List<Map<String, Object?>> routes) =>
    NearbyTransit.fromJson({
      'generated_at': fakeStart,
      'window': {'earliest': fakeStart, 'latest': fakeStart + 7200},
      'realtime': {
        'level': 'schedule',
        'built_at': fakeStart,
        'feeds': <Object>[],
      },
      'routes': routes,
      if (routes.isEmpty) 'no_routes_reason': 'no_service',
    });

/// A route detail's JSON, built field by field from `RouteDetailResponse` in
/// `valinor-transit-plan/src/response/nearby.rs`: route 40 north from Fremont, the next
/// trip live and 2 minutes late under a detour, one canceled after it, a ride to the end
/// of its trip, and its vehicle.
Map<String, Object?> fakeRouteDetailJson() {
  final t = fakeStart;
  Map<String, Object?> stop(String id, double lat, int time) => {
    'accessibility': 'accessible',
    'feed': 'us-wa-Metro-Transit',
    'stop_id': id,
    'name': id,
    'latitude': lat,
    'longitude': -122.35,
    'arrival': time,
    'departure': time,
    'scheduled_arrival': time,
    'scheduled_departure': time,
    'skipped': false,
    'removed_by_alert': false,
    'utc_offset_seconds': -8 * 3600,
  };
  final alert = {
    'feed': 'us-wa-Metro-Transit',
    'id': 'a1',
    'effect': 'detour',
    'active_periods': <Object>[],
    'communication_periods': <Object>[],
    'impact_periods': <Object>[],
    'header': [
      {'text': 'Route 40 detoured', 'language': 'en'},
    ],
    'description': <Object>[],
    'url': <Object>[],
    'informed': [
      {'route_id': '100040'},
    ],
  };
  final vehicle = {
    'feed': 'us-wa-Metro-Transit',
    'id': '7033',
    'label': '7033',
    'trip_id': 't40',
    'route_id': '100040',
    'latitude': 47.64,
    'longitude': -122.35,
    'bearing': 10.0,
    'timestamp': t - 20,
  };
  return {
    'generated_at': t,
    'window': {'earliest': t, 'latest': t + 7200},
    'realtime': {
      'level': 'realtime',
      'built_at': t,
      'feeds': [
        {
          'feed': 'us-wa-Metro-Transit',
          'kind': 'trip_updates',
          'state': 'applied',
          'age_seconds': 20,
          'stale_after_seconds': 300,
          'entities_dropped': 0,
        },
      ],
    },
    'feed': 'us-wa-Metro-Transit',
    'route': {
      'route_id': '100040',
      'short_name': '40',
      'route_type': 3,
      'color': 'FDB71A',
    },
    'agency': {'agency_id': '1', 'name': 'Metro Transit'},
    'direction_id': 0,
    'headsigns': ['Northgate'],
    'stop': {
      'accessibility': 'accessible',
      'feed': 'us-wa-Metro-Transit',
      'stop_id': 'fremont',
      'name': 'Fremont Ave N & N 34th St',
      'latitude': 47.65,
      'longitude': -122.35,
    },
    'walk_seconds': 240,
    'departures': [
      {
        'trip_id': 't40',
        'start_date': '20261109',
        'headsign': 'Northgate',
        'status': 'live',
        'departure': t + 600,
        'scheduled_departure': t + 480,
        'delay_seconds': 120,
        'realtime': true,
        'vehicle_id': '7033',
        'accessibility': 'accessible',
        'alerts': [0],
      },
      {
        'trip_id': 't40b',
        'start_date': '20261109',
        'headsign': 'Northgate',
        'status': 'canceled',
        'departure': t + 1200,
        'scheduled_departure': t + 1200,
        'accessibility': 'accessible',
      },
    ],
    'ride': {
      'accessibility': 'accessible',
      'departure': t + 600,
      'arrival': t + 1800,
      'duration_seconds': 1200,
      'wait_seconds': 360,
      'delay_seconds': 120,
      'realtime': true,
      'route': {'route_id': '100040', 'short_name': '40', 'route_type': 3},
      'agency': {'name': 'Metro Transit'},
      'trip': {'trip_id': 't40', 'headsign': 'Northgate'},
      'stops': [
        stop('fremont', 47.65, t + 600),
        stop('wallingford', 47.66, t + 1200),
        stop('northgate', 47.70, t + 1800),
      ],
      'vehicle': vehicle,
      'alerts': [alert],
    },
    'vehicles': [vehicle],
    'alerts': [alert],
  };
}
