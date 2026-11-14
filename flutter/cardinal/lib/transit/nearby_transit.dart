/// Nearby departures and a route's detail, as the transit router answers them
/// (`valinor-transit-plan`'s `/transit/nearby` and `/transit/route` JSON).
///
/// The same JSON comes from a server and from the device, so these models don't depend on
/// where it came from. Routes, stops, departures, vehicles, alerts, and a ride are read as a
/// plan reads them (`directions/transit_plan.dart`).
library;

import 'dart:convert';

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
      directions = [
        for (final d in _list(json['directions']))
          NearbyDirection._(_map(d), alerts),
      ];

  final String feed;
  final TransitRoute route;

  /// The agency's name, when the feed gives one.
  final String? agency;

  /// Each direction with a departure the rider can catch, in an order that stays the
  /// same from one answer to the next.
  final List<NearbyDirection> directions;

  RouteKey get key => (feed: feed, routeId: route.id);
}

/// Which way a route goes: its GTFS `direction_id`, or for trips without one, the
/// `stop_id` of their last stop. Exactly one is set.
class DirectionRef {
  const DirectionRef.id(int this.directionId) : terminusStopId = null;
  const DirectionRef.terminus(String this.terminusStopId) : directionId = null;

  DirectionRef._(Map<String, Object?> json)
    : directionId = (json['direction_id'] as num?)?.toInt(),
      terminusStopId = json['direction_id'] == null
          ? json['terminus_stop_id'] as String?
          : null;

  final int? directionId;
  final String? terminusStopId;

  /// The request fields that name it.
  Map<String, Object?> toJson() => {
    if (directionId != null) 'direction_id': directionId,
    if (terminusStopId != null) 'terminus_stop_id': terminusStopId,
  };

  @override
  bool operator ==(Object other) =>
      other is DirectionRef &&
      other.directionId == directionId &&
      other.terminusStopId == terminusStopId;

  @override
  int get hashCode => Object.hash(directionId, terminusStopId);
}

/// One direction of a nearby route, from the stop where the rider catches it soonest.
class NearbyDirection {
  NearbyDirection._(Map<String, Object?> json, List<LegAlert> alerts)
    : direction = DirectionRef._(json),
      headsigns = [
        for (final h in _list(json['headsigns']))
          if (h is String) h,
      ],
      stop = StopRef.fromJson(_map(json['stop'])),
      walkSeconds = (json['walk_seconds'] as num?)?.toInt() ?? 0,
      departures = [
        for (final d in _list(json['departures']))
          StopDeparture.fromJson(_map(d), alerts: alerts),
      ];

  final DirectionRef direction;

  /// The distinct headsigns of [departures], the most frequent first.
  final List<String> headsigns;

  /// The nearest stop where the rider can catch this direction's earliest trip.
  final StopRef stop;

  /// The walk to [stop].
  final int walkSeconds;

  /// The next departures from [stop] the rider can be there for, by departure. The first
  /// may be canceled or skipped.
  final List<StopDeparture> departures;
}

/// One route and direction from one stop, for a row's detail.
class RouteDetail {
  RouteDetail.fromJson(Map<String, Object?> json) : this._(json, _alerts(json));

  RouteDetail._(Map<String, Object?> json, List<LegAlert> alerts)
    : generatedAt = _time(json['generated_at']),
      realtime = RealtimeStatus.fromJson(_map(json['realtime'])),
      feed = json['feed'] as String? ?? '',
      route = TransitRoute.fromJson(_map(json['route'])),
      agency = _map(json['agency'])['name'] as String?,
      direction = DirectionRef._(json),
      headsigns = [
        for (final h in _list(json['headsigns']))
          if (h is String) h,
      ],
      stop = StopRef.fromJson(_map(json['stop'])),
      walkSeconds = (json['walk_seconds'] as num?)?.toInt() ?? 0,
      departures = [
        for (final d in _list(json['departures']))
          StopDeparture.fromJson(_map(d), alerts: alerts),
      ],
      ride = json['ride'] == null
          ? null
          : TransitLeg.fromJson(_map(json['ride'])),
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
  final DirectionRef direction;

  /// The distinct headsigns of [departures], the most frequent first.
  final List<String> headsigns;
  final StopRef stop;
  final int walkSeconds;

  /// The next departures from [stop] in this direction the rider can be there for.
  final List<StopDeparture> departures;

  /// The chosen trip, or else the first departure that runs, from [stop] to the end of
  /// its trip, with every stop's times, its shape, vehicle, and alerts. Null when nothing
  /// runs in the horizon.
  final TransitLeg? ride;

  /// Vehicles real-time data has on this route's trips in this direction.
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
