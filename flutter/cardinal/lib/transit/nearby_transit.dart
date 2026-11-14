/// Nearby departures and a route's detail, as the transit router answers them
/// (`valinor-transit-plan`'s `/transit/nearby` and `/transit/route` JSON).
///
/// The same JSON comes from a server and from the device, so these models don't depend on
/// where it came from. Routes, stops, departures, vehicles, alerts, and a ride are read as a
/// plan reads them (`directions/transit_plan.dart`).
library;

import 'dart:convert';

import 'package:routing/routing.dart' show DirectionsPoint;

import '../directions/transit_plan.dart';

/// Why a nearby answer lists no routes.
enum NoRoutesReason {
  /// No stop is within the walk.
  noAccess,

  /// Stops are within the walk, but nothing the rider can catch leaves them in time.
  noService,
  unknown;

  static NoRoutesReason? parse(Object? value) => switch (value) {
    null => null,
    'no_access' => noAccess,
    'no_service' => noService,
    _ => unknown,
  };
}

/// Which GTFS route a nearby row is: a `route_id` is only unique within its feed.
typedef RouteKey = ({String feed, String routeId});

/// The routes a rider can catch near a place, soonest first.
class NearbyTransit {
  NearbyTransit.fromJson(Map<String, Object?> json)
    : generatedAt = _time(json['generated_at']),
      realtime = RealtimeStatus.fromJson(_map(json['realtime'])),
      noRoutesReason = NoRoutesReason.parse(json['no_routes_reason']),
      routes = _routes(json);

  static List<NearbyRoute> _routes(Map<String, Object?> json) {
    final alerts = _alerts(json);
    return [
      for (final r in _list(json['routes'])) NearbyRoute._(_map(r), alerts),
    ];
  }

  static NearbyTransit decode(List<int> body) =>
      NearbyTransit.fromJson(_map(jsonDecode(utf8.decode(body))));

  final DateTime generatedAt;
  final RealtimeStatus realtime;

  /// Each GTFS route with a departure the rider can catch, by the soonest.
  final List<NearbyRoute> routes;

  /// Why [routes] is empty, or null when it isn't.
  final NoRoutesReason? noRoutesReason;
}

/// One GTFS route near the rider: a row.
class NearbyRoute {
  NearbyRoute._(Map<String, Object?> json, List<LegAlert> alerts)
    : feed = json['feed'] as String? ?? '',
      route = TransitRoute.fromJson(_map(json['route'])),
      agency = _map(json['agency'])['name'] as String?,
      pages = [
        for (final p in _list(json['pages'])) HeadsignPage._(_map(p), alerts),
      ];

  /// [detail]'s route on its one page, for a detail opened without the row it came
  /// from, as when the app restores it.
  NearbyRoute.ofDetail(RouteDetail detail)
    : feed = detail.feed,
      route = detail.route,
      agency = detail.agency,
      pages = [HeadsignPage.ofDetail(detail)];

  final String feed;
  final TransitRoute route;

  /// The agency's name, when the feed gives one.
  final String? agency;

  /// Each page with a departure the rider can catch, one a row shows at a time, in an
  /// order that stays the same from one answer to the next.
  final List<HeadsignPage> pages;

  RouteKey get key => (feed: feed, routeId: route.id);
}

/// One page of a nearby route: its departures under one headsign, or under several
/// whose trips go the same way from [stop], from the stop where the rider catches them
/// soonest.
class HeadsignPage {
  HeadsignPage._(Map<String, Object?> json, List<LegAlert> alerts)
    : headsigns = _strings(json['headsigns']),
      directionId = (json['direction_id'] as num?)?.toInt(),
      stop = StopRef.fromJson(_map(json['stop'])),
      walkSeconds = (json['walk_seconds'] as num?)?.toInt() ?? 0,
      soonestDeparture = _time(json['soonest_departure']),
      departures = [
        for (final d in _list(json['departures']))
          NearbyDeparture.fromJson(_map(d), alerts: alerts),
      ];

  /// [detail]'s page.
  HeadsignPage.ofDetail(RouteDetail detail)
    : headsigns = detail.headsigns,
      directionId = null,
      stop = detail.stop,
      walkSeconds = detail.walkSeconds,
      soonestDeparture =
          detail.departures.firstOrNull?.departure ?? detail.generatedAt,
      departures = detail.departures;

  /// The page's headsigns, the most frequent first, shown as "A / B"; empty for the
  /// departures that have none. A route's detail asks for the page by these.
  final List<String> headsigns;

  /// The GTFS `direction_id` of the page's trips when they share one. It orders the
  /// pages, and says nothing else.
  final int? directionId;

  /// The nearest stop where the rider can catch this page's earliest trip.
  final StopRef stop;

  /// The walk to [stop].
  final int walkSeconds;

  /// When this page's earliest trip the rider can catch leaves [stop].
  final DateTime soonestDeparture;

  /// The next departures from [stop] the rider can be there for, by departure. The first
  /// may be canceled or skipped.
  final List<NearbyDeparture> departures;
}

/// One departure from a nearby stop.
class NearbyDeparture extends StopDeparture {
  NearbyDeparture.fromJson(super.json, {super.alerts})
    : continuesAs = json['continues_as'] == null
          ? null
          : ContinuesAs._(_map(json['continues_as'])),
      super.fromJson();

  /// The route its vehicle goes on as after this trip, with riders staying on.
  final ContinuesAs? continuesAs;
}

/// The trip a vehicle runs next as another route: "continues as 43".
class ContinuesAs {
  ContinuesAs._(Map<String, Object?> json)
    : routeId = json['route_id'] as String? ?? '',
      shortName = json['short_name'] as String?,
      headsign = json['headsign'] as String?;

  /// The next trip's `route_id`, in the departure's feed.
  final String routeId;

  /// That route's short name, when the feed gives one.
  final String? shortName;

  /// The next trip's headsign.
  final String? headsign;
}

/// One page of a route from one stop, for a row's detail.
class RouteDetail {
  RouteDetail.fromJson(Map<String, Object?> json) : this._(json, _alerts(json));

  RouteDetail._(Map<String, Object?> json, List<LegAlert> alerts)
    : generatedAt = _time(json['generated_at']),
      realtime = RealtimeStatus.fromJson(_map(json['realtime'])),
      feed = json['feed'] as String? ?? '',
      route = TransitRoute.fromJson(_map(json['route'])),
      agency = _map(json['agency'])['name'] as String?,
      headsigns = _strings(json['headsigns']),
      stop = StopRef.fromJson(_map(json['stop'])),
      walkSeconds = (json['walk_seconds'] as num?)?.toInt() ?? 0,
      departures = [
        for (final d in _list(json['departures']))
          NearbyDeparture.fromJson(_map(d), alerts: alerts),
      ],
      ride = json['ride'] == null
          ? null
          : TransitLeg.fromJson(_map(json['ride'])),
      shapeToStop = switch (json['shape_to_stop']) {
        final String shape => decodePolyline6(shape),
        _ => const [],
      },
      vehicles = [
        for (final v in _list(json['vehicles']))
          TransitVehicle.fromJson(_map(v)),
      ];

  static RouteDetail decode(List<int> body) =>
      RouteDetail.fromJson(_map(jsonDecode(utf8.decode(body))));

  final DateTime generatedAt;
  final RealtimeStatus realtime;
  final String feed;
  final TransitRoute route;
  final String? agency;

  /// The page's headsigns, as they were asked for.
  final List<String> headsigns;
  final StopRef stop;
  final int walkSeconds;

  /// The next departures from [stop] under the page's headsigns the rider can be there
  /// for.
  final List<NearbyDeparture> departures;

  /// The chosen trip, or else the first departure that runs, from [stop] to the end of
  /// its trip, with every stop's times, its shape, vehicle, and alerts. Null when nothing
  /// runs in the horizon.
  final TransitLeg? ride;

  /// [ride]'s trip before [stop], where the rider can't board it yet: from its first stop
  /// to [stop], where [ride]'s shape begins. Empty when [stop] is its first, or without
  /// a ride.
  final List<DirectionsPoint> shapeToStop;

  /// Vehicles real-time data has on this route's trips that leave [stop] under one of
  /// the page's headsigns.
  final List<TransitVehicle> vehicles;
}

List<LegAlert> _alerts(Map<String, Object?> json) => [
  for (final a in _list(json['alerts'])) LegAlert.fromJson(_map(a)),
];

DateTime _time(Object? seconds) =>
    DateTime.fromMillisecondsSinceEpoch((seconds as num).toInt() * 1000);

Map<String, Object?> _map(Object? value) =>
    value is Map<String, Object?> ? value : const {};

List<Object?> _list(Object? value) => value is List<Object?> ? value : const [];

List<String> _strings(Object? value) => [
  for (final s in _list(value))
    if (s is String) s,
];
