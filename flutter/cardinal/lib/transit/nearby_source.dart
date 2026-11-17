/// Where nearby departures come from: the [NearbyTransitSource] seam, and the device's
/// own over the offline area's timetables.
library;

import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../directions/route_source.dart';
import '../directions/transit_plan.dart';
import '../offline/offline_area.dart';
import '../offline/realtime.dart';
import '../store/store.dart';
import 'nearby_transit.dart';

/// The longest walk to a stop nearby departures are listed from.
const nearbyWalkLimit = Duration(minutes: 30);

/// How far ahead nearby departures are listed.
const nearbyHorizon = Duration(hours: 2);

/// How many departures that run a nearby row lists on each page. Canceled and skipped
/// ones among them are listed too, and don't count.
const nearbyRowDepartures = 3;

/// How many departures that run a route's detail lists, counted as for a row.
const routeDetailDepartures = 6;

/// How nearby transit gets to stops: by [wheelchair] (the `wheelchairByDefault`
/// setting), or else on foot. Nearby departures are never by bike.
TransitAccess nearbyAccess({required bool wheelchair}) =>
    wheelchair ? TransitAccess.wheelchair : TransitAccess.walking;

/// A route detail to ask for, by the identifiers a nearby answer gave.
@immutable
class RouteDetailQuery {
  const RouteDetailQuery({
    required this.feed,
    required this.routeId,
    required this.headsigns,
    required this.stopId,
    required this.walkSeconds,
    this.tripId,
    this.startDate,
  });

  /// [page] of [route] from its stop, showing [departure]'s trip if it's given, and
  /// otherwise the first that runs.
  factory RouteDetailQuery.of(
    NearbyRoute route,
    HeadsignPage page, {
    StopDeparture? departure,
  }) => RouteDetailQuery(
    feed: route.feed,
    routeId: route.route.id,
    headsigns: page.headsigns,
    stopId: page.stop.id,
    walkSeconds: page.walkSeconds,
    tripId: departure?.tripId,
    startDate: departure?.startDate,
  );

  /// The same page, showing [departure]'s trip, or the first that runs if it's null.
  RouteDetailQuery showing(StopDeparture? departure) => RouteDetailQuery(
    feed: feed,
    routeId: routeId,
    headsigns: headsigns,
    stopId: stopId,
    walkSeconds: walkSeconds,
    tripId: departure?.tripId,
    startDate: departure?.startDate,
  );

  final String feed;
  final String routeId;

  /// The page, by its headsigns; empty for the departures that have none.
  final List<String> headsigns;
  final String stopId;

  /// The walk to the stop: departures the rider can't be there for aren't listed.
  final int walkSeconds;

  /// The trip whose stops to show, with its service day.
  final String? tripId;
  final String? startDate;
}

/// The body of a `/transit/nearby` request from [location], getting to stops by
/// [access].
Map<String, Object?> nearbyRequest(LngLat location, TransitAccess access) {
  assert(
    access != TransitAccess.cycling,
    'nearby departures are never by bike',
  );
  return {
    'location': {'lat': location.lat, 'lon': location.lon},
    'mode': access.mode,
    'max_walk_seconds': nearbyWalkLimit.inSeconds,
    'horizon_seconds': nearbyHorizon.inSeconds,
    'max_departures': nearbyRowDepartures,
  };
}

/// The body of a `/transit/route` request for [query], getting to the stop by [access].
Map<String, Object?> routeRequest(
  RouteDetailQuery query,
  TransitAccess access,
) {
  assert(
    access != TransitAccess.cycling,
    'nearby departures are never by bike',
  );
  return {
    'feed': query.feed,
    'route_id': query.routeId,
    'headsigns': query.headsigns,
    'stop_id': query.stopId,
    'mode': access.mode,
    'walk_seconds': query.walkSeconds,
    'horizon_seconds': nearbyHorizon.inSeconds,
    'max_departures': routeDetailDepartures,
    if (query.tripId != null) 'trip_id': query.tripId,
    if (query.tripId != null && query.startDate != null)
      'start_date': query.startDate,
  };
}

/// Where nearby departures come from, behind what the app needs of it: the device's
/// timetables today, and perhaps a server later. Tests fake it.
abstract class NearbyTransitSource {
  /// Gets ready to answer, if that takes a while, so the first answer doesn't wait as
  /// long. It returns at once, doing the work in the background, and fetches nothing
  /// from the network: the app calls it as it starts.
  void prepare() {}

  /// The routes a rider at [location] can catch in the next [nearbyHorizon], walking at
  /// most [nearbyWalkLimit] to a stop by [access], with the real-time data at hand. It
  /// never waits for real-time data being fetched: the first answer is on the schedule,
  /// and [realtimeArrived] says when to ask again for live times. Call it again to
  /// refresh.
  ///
  /// Throws [TransitPlanError] if it fails, [OutsideOfflineArea] or [NoOfflineArea] if
  /// it can't be asked, and [http.RequestAbortedException] if [abort] completes first.
  Future<NearbyTransit> nearby({
    required LngLat location,
    required TransitAccess access,
    Future<void>? abort,
  });

  /// Whether real-time data has arrived that the last answer ([nearby] or [route]) was
  /// asked without, because it was still being fetched for the first time: completes to
  /// true once it has, when asking again gives live times, and to false when it brought
  /// nothing. It completes to false at once when the last answer didn't wait on such a
  /// fetch; after the first fetch, each answer takes what has arrived since the one
  /// before.
  Future<bool> realtimeArrived() async => false;

  /// One page of a route from a stop, as [query] names it, getting to the stop by
  /// [access]. Like [nearby], it never waits for real-time data being fetched, and
  /// [realtimeArrived] says when to ask again. Throws as [nearby] does; a route, stop, or trip that isn't there is a
  /// [TransitPlanError] of kind `unknown_route`, `unknown_stop`, or `unknown_trip`.
  Future<RouteDetail> route({
    required RouteDetailQuery query,
    required TransitAccess access,
    Future<void>? abort,
  });
}

/// [NearbyTransitSource] on the device, over the offline area's timetables, with the
/// latest real-time data its agencies publish when there's a network.
class OfflineNearbySource implements NearbyTransitSource {
  OfflineNearbySource(this._area, this._feeds);

  final OfflineAreaSource _area;

  /// The area's real-time feeds, shared with the planner's other askers.
  final AreaRealtimeFeeds _feeds;

  RealtimeFeeds _realtime(OfflineArea area) => _feeds.of(area);

  /// The first real-time fetch the last answer was asked during, if any.
  Future<bool>? _firstRealtime;

  /// Opens the area's timetables (on the routing library's threads), so the first
  /// answer doesn't wait for them.
  @override
  void prepare() {
    unawaited(() async {
      try {
        final area = await _area();
        if (area == null || !area.hasTransit) return;
        await area.transit;
      } on Object catch (e) {
        debugPrint('nearby transit: $e');
      }
    }());
  }

  @override
  Future<bool> realtimeArrived() => _firstRealtime ?? Future.value(false);

  @override
  Future<NearbyTransit> nearby({
    required LngLat location,
    required TransitAccess access,
    Future<void>? abort,
  }) async {
    final cancel = TransitCancel();
    unawaited(abort?.then((_) => cancel.cancel()));
    return abortable(
      _ask(
        'nearby',
        nearbyRequest(location, access),
        covering: location,
        realtime: (realtime) {
          final (:feeds, :first) = realtime.arrived();
          _firstRealtime = first;
          return Future.value(feeds);
        },
        query: (transit, request, feeds) =>
            transit.nearby(request: request, feeds: feeds, cancel: cancel),
      ).then(NearbyTransit.decode),
      abort,
    );
  }

  @override
  Future<RouteDetail> route({
    required RouteDetailQuery query,
    required TransitAccess access,
    Future<void>? abort,
  }) async {
    final cancel = TransitCancel();
    unawaited(abort?.then((_) => cancel.cancel()));
    return abortable(
      _ask(
        'route',
        routeRequest(query, access),
        realtime: (realtime) {
          final (:feeds, :first) = realtime.arrived();
          _firstRealtime = first;
          return Future.value(feeds);
        },
        query: (transit, request, feeds) =>
            transit.route(request: request, feeds: feeds, cancel: cancel),
      ).then(RouteDetail.decode),
      abort,
    );
  }

  /// Asks [query] with [body] and the real-time messages [realtime] takes, after
  /// checking the area holds [covering], and returns the answer's body.
  Future<List<int>> _ask(
    String what,
    Map<String, Object?> body, {
    LngLat? covering,
    required Future<List<TransitFeed>> Function(RealtimeFeeds feeds) realtime,
    required Future<TransitResponse> Function(
      OfflineTransit transit,
      List<int> request,
      List<TransitFeed> feeds,
    )
    query,
  }) async {
    final area = await needOfflineArea(_area);
    if (covering != null && !await area.covers(covering.lat, covering.lon)) {
      throw const OutsideOfflineArea();
    }
    // Real-time messages are fetched while the timetables open.
    final fetching = realtime(_realtime(area));
    final opening = area.transit;
    final feeds = await fetching;
    await opening;
    final request = utf8.encode(jsonEncode(body));
    final watch = Stopwatch()..start();
    final response = await area.useTransit(
      (transit) => query(transit, request, feeds),
    );
    debugPrint(
      'cardinal-timing: transit $what ${body['mode']} '
      '${watch.elapsedMilliseconds} ms (${response.status}, '
      '${feeds.length} new real-time messages)',
    );
    return transitAnswer(response);
  }
}
