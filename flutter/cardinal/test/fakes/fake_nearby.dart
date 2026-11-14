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

  /// Real-time data on its way, for the next question to go without: the test completes
  /// it to say whether it brought anything.
  Completer<bool>? realtimeLanding;

  /// What the last question went without.
  Completer<bool>? _landing;

  @override
  void prepare() => prepared++;

  @override
  Future<bool> realtimeArrived() => _landing?.future ?? Future.value(false);

  @override
  Future<NearbyTransit> nearby({
    required LngLat location,
    required TransitAccess access,
    Future<void>? abort,
  }) {
    final question = NearbyQuestion(location, access, abort);
    asked.add(question);
    _landing = realtimeLanding;
    realtimeLanding = null;
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
    _landing = realtimeLanding;
    realtimeLanding = null;
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
Map<String, Object?> snapshotNearbyJson() => _snapshot('nearby_response');

/// The router's route detail from valinor's own test, as [snapshotNearbyJson] is: route 1
/// north from North 1st, the next trip live and 2 minutes late with its vehicle, the one
/// after canceled, all under a detour alert.
Map<String, Object?> snapshotRouteDetailJson() =>
    _snapshot('route_detail_response');

/// The router's nearby answer with a shared page from valinor's own test: route 44's
/// "UW Medical Center" and "University District", whose 11:50 goes on as route 43.
Map<String, Object?> snapshotSharedPageJson() =>
    _snapshot('nearby_shared_page');

Map<String, Object?> _snapshot(String name) {
  final snap = File(
    '../../valinor/valinor-transit-plan/src/snapshots/'
    'valinor_transit_plan__nearby_tests__$name.snap',
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

/// A nearby route's JSON, on one page from [stop], leaving [minutes] after
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
  'pages': [
    {
      'headsigns': ['Downtown Seattle'],
      'direction_id': 0,
      'stop': {
        'accessibility': 'unknown',
        'feed': feed,
        'stop_id': stop,
        'name': stop,
        'latitude': 47.65,
        'longitude': -122.35,
      },
      'walk_seconds': walkSeconds,
      'soonest_departure': fakeStart + minutes.first * 60,
      'departures': [
        for (final m in minutes) nearbyDepartureJson('$routeId-$m', m),
      ],
    },
  ],
};

/// One page of [nearbyRoutePagesJson]: [headsigns] from [stop], leaving [minutes] after
/// [fakeStart].
typedef FakePage = ({List<String> headsigns, List<int> minutes, String stop});

/// A nearby route's JSON with [pages], in the order given.
Map<String, Object?> nearbyRoutePagesJson(
  String routeId,
  List<FakePage> pages, {
  String feed = 'us-wa-Metro-Transit',
  int walkSeconds = 240,
}) => {
  'feed': feed,
  'route': {'route_id': routeId, 'short_name': routeId, 'route_type': 3},
  'agency': {'name': 'Metro Transit'},
  'pages': [
    for (final page in pages)
      {
        'headsigns': page.headsigns,
        'stop': {
          'accessibility': 'unknown',
          'feed': feed,
          'stop_id': page.stop,
          'name': page.stop,
          'latitude': 47.65,
          'longitude': -122.35,
        },
        'walk_seconds': walkSeconds,
        'soonest_departure': fakeStart + page.minutes.first * 60,
        'departures': [
          for (final m in page.minutes)
            nearbyDepartureJson(
              '$routeId-${page.headsigns.firstOrNull}-$m',
              m,
              headsign: page.headsigns.firstOrNull,
            ),
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
