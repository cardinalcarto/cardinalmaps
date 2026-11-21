/// A transit plan as the router answers it (`valinor-transit-plan`'s response JSON),
/// read into what the directions sheet shows.
///
/// Only what the sheet uses is read; everything a feed may omit is nullable. Its routes,
/// stops, departures, vehicles, and alerts are also how nearby transit reads its own
/// responses (`transit/nearby_transit.dart`).
library;

import 'dart:convert';

import 'package:routing/routing.dart' show DirectionsPoint;

/// What is known about whether a stop or leg can be used in a wheelchair.
enum Accessibility {
  accessible,
  inaccessible,
  unknown;

  static Accessibility parse(Object? value) => switch (value) {
    'accessible' => accessible,
    'inaccessible' => inaccessible,
    _ => unknown,
  };
}

/// How a rider gets to, from, and between stops.
enum TransitAccess {
  walking('walking', 'Walk'),
  cycling('cycling', 'Bike'),
  wheelchair('wheelchair', 'Wheelchair');

  const TransitAccess(this.mode, this.label);

  /// The router's name for it.
  final String mode;
  final String label;
}

/// How current the real-time data behind a plan was.
enum RealtimeLevel {
  schedule,
  degraded,
  realtime;

  static RealtimeLevel parse(Object? value) => switch (value) {
    'realtime' => realtime,
    'degraded' => degraded,
    _ => schedule,
  };
}

/// What real-time data an answer used: a response's `realtime`.
class RealtimeStatus {
  RealtimeStatus.fromJson(Map<String, Object?> json)
    : level = RealtimeLevel.parse(json['level']),
      feedsApplied = _feeds(json, applied: true),
      feedsReported = _feeds(json, applied: false);

  // One entry per message (a feed may send trip updates, alerts, and vehicles), so
  // feeds are counted by name.
  static int _feeds(Map<String, Object?> json, {required bool applied}) => {
    for (final m in _list(json['feeds']).map(_map))
      if (!applied || m['state'] == 'applied') m['feed'],
  }.length;

  final RealtimeLevel level;

  /// How many feeds' real-time data was used (any of their messages), of
  /// [feedsReported] feeds that have some.
  final int feedsApplied;
  final int feedsReported;
}

/// Why a plan has no journeys.
enum NoJourneysReason {
  noAccess,
  noEgress,
  noService,
  slowerThanDirect,
  unknown;

  static NoJourneysReason? parse(Object? value) => switch (value) {
    null => null,
    'no_access' => noAccess,
    'no_egress' => noEgress,
    'no_service' => noService,
    'slower_than_direct' => slowerThanDirect,
    _ => unknown,
  };
}

/// The answer to one planning question.
class TransitPlan {
  TransitPlan({
    required this.journeys,
    required this.realtime,
    required this.feedsApplied,
    required this.feedsReported,
    this.noJourneysReason,
    this.directSeconds,
  });

  factory TransitPlan.fromJson(Map<String, Object?> json) {
    final realtime = RealtimeStatus.fromJson(_map(json['realtime']));
    final direct = _map(json['direct']);
    return TransitPlan(
      journeys: [for (final j in _list(json['journeys'])) Journey._(_map(j))],
      realtime: realtime.level,
      feedsApplied: realtime.feedsApplied,
      feedsReported: realtime.feedsReported,
      noJourneysReason: NoJourneysReason.parse(json['no_journeys_reason']),
      directSeconds: direct['status'] == 'found'
          ? (direct['seconds'] as num?)?.toInt()
          : null,
    );
  }

  static TransitPlan decode(List<int> body) =>
      TransitPlan.fromJson(_map(jsonDecode(utf8.decode(body))));

  /// Every journey of the departure window, by departure.
  final List<Journey> journeys;
  final RealtimeLevel realtime;

  /// How many feeds' real-time data was used (any of their messages), of
  /// [feedsReported] feeds that have some.
  final int feedsApplied;
  final int feedsReported;
  final NoJourneysReason? noJourneysReason;

  /// How long going there directly, without transit, takes, when that was found.
  final int? directSeconds;
}

/// One way to make the trip.
class Journey {
  Journey._(Map<String, Object?> json)
    : departure = _time(json['departure']),
      arrival = _time(json['arrival']),
      duration = (json['duration_seconds'] as num).toInt(),
      trips = (json['trips'] as num).toInt(),
      legs = [
        for (final leg in _list(json['legs'])) JourneyLeg._parse(_map(leg)),
      ];

  final DateTime departure;
  final DateTime arrival;

  /// Seconds door to door.
  final int duration;

  /// How many vehicles it rides; a stay-seated ride counts once.
  final int trips;
  final List<JourneyLeg> legs;

  int get transfers => trips > 0 ? trips - 1 : 0;

  Iterable<TransitLeg> get rides => legs.whereType<TransitLeg>();

  /// Whether any ride's times came from real-time data.
  bool get realtime => rides.any((ride) => ride.realtime);

  /// Meters on foot or by bike, for the legs whose paths are known.
  double get streetMeters => legs.whereType<StreetLeg>().fold(
    0.0,
    (sum, leg) => sum + (leg.walk?.distance ?? 0),
  );

  /// The least accessible of its legs.
  Accessibility get accessibility {
    final all = legs.map((leg) => leg.accessibility);
    if (all.contains(Accessibility.inaccessible)) {
      return Accessibility.inaccessible;
    }
    if (all.every((a) => a == Accessibility.accessible)) {
      return Accessibility.accessible;
    }
    return Accessibility.unknown;
  }

  /// The first ride's departure, which a rider has to make.
  DateTime? get firstBoarding => rides.firstOrNull?.departure;
}

/// One leg of a journey.
sealed class JourneyLeg {
  JourneyLeg(Map<String, Object?> json)
    : accessibility = Accessibility.parse(json['accessibility']),
      departure = _time(json['departure']),
      arrival = _time(json['arrival']),
      duration = (json['duration_seconds'] as num).toInt();

  static JourneyLeg _parse(Map<String, Object?> json) => switch (json['type']) {
    'transit' => TransitLeg.fromJson(json),
    'access' => StreetLeg._(json, StreetLegKind.access),
    'egress' => StreetLeg._(json, StreetLegKind.egress),
    _ => StreetLeg._(json, StreetLegKind.transfer),
  };

  final Accessibility accessibility;
  final DateTime departure;
  final DateTime arrival;
  final int duration;

  /// The line it follows on the map, or empty if it isn't known.
  List<DirectionsPoint> get shape;
}

enum StreetLegKind { access, transfer, egress }

/// A walk or ride on the streets: to the first stop, between stops, or from the last.
class StreetLeg extends JourneyLeg {
  StreetLeg._(super.json, this.kind)
    : mode = json['mode'] as String? ?? 'walking',
      from = kind == StreetLegKind.transfer
          ? StopRef.fromJson(_map(json['from']))
          : kind == StreetLegKind.egress
          ? StopRef.fromJson(_map(json['stop']))
          : null,
      to = kind == StreetLegKind.transfer
          ? StopRef.fromJson(_map(json['to']))
          : kind == StreetLegKind.access
          ? StopRef.fromJson(_map(json['stop']))
          : null,
      walk = json['walk'] == null ? null : WalkPath._(_map(json['walk'])),
      stopMoved = [
        for (final n in _list(json['stop_moved'])) StopMovedNotice._(_map(n)),
      ];

  final StreetLegKind kind;

  /// `walking`, `cycling`, or `wheelchair`.
  final String mode;

  /// The stop it leaves, or null for the trip's start.
  final StopRef? from;

  /// The stop it reaches, or null for the trip's destination.
  final StopRef? to;
  final WalkPath? walk;

  /// Stops this leg walks to or from that have moved, with what the agency says.
  final List<StopMovedNotice> stopMoved;

  @override
  List<DirectionsPoint> get shape => walk?.shape ?? const [];
}

/// A ride on one vehicle, perhaps across several trips without getting off.
class TransitLeg extends JourneyLeg {
  TransitLeg.fromJson(super.json)
    : waitSeconds = (json['wait_seconds'] as num?)?.toInt() ?? 0,
      transferSeconds = (json['transfer_seconds'] as num?)?.toInt(),
      delaySeconds = (json['delay_seconds'] as num?)?.toInt(),
      realtime = json['realtime'] == true,
      route = json['route'] == null
          ? null
          : TransitRoute.fromJson(_map(json['route'])),
      agency = _map(json['agency'])['name'] as String?,
      headsign = _map(json['trip'])['headsign'] as String?,
      tripId = _map(json['trip'])['trip_id'] as String?,
      startDate = _map(json['trip'])['start_date'] as String?,
      tripState = _map(json['trip'])['realtime_state'] as String?,
      bikesAllowed = _map(json['trip'])['bikes_allowed'] as String?,
      stops = [for (final s in _list(json['stops'])) StopVisit._(_map(s))],
      segments = [
        for (final s in _list(json['segments'])) RideSegment._(_map(s)),
      ],
      _shape = json['shape'] as String?,
      vehicle = json['vehicle'] == null
          ? null
          : TransitVehicle.fromJson(_map(json['vehicle'])),
      alerts = [
        for (final a in _list(json['alerts'])) LegAlert.fromJson(_map(a)),
      ],
      departures = _departures(json);

  /// The leg of a ride lookup's answer (`POST /transit/ride`): its `ride`.
  static TransitLeg decodeRide(List<int> body) =>
      TransitLeg.fromJson(_map(_map(jsonDecode(utf8.decode(body)))['ride']));

  static List<RideDeparture> _departures(Map<String, Object?> json) {
    final own = [
      for (final a in _list(json['alerts'])) LegAlert.fromJson(_map(a)),
    ];
    final others = [
      for (final a in _list(json['departure_alerts']))
        LegAlert.fromJson(_map(a)),
    ];
    return [
      for (final d in _list(json['departures']))
        RideDeparture._(_map(d), own, others),
    ];
  }

  final int waitSeconds;

  /// For a ride after the journey's first: from the ride before reaching the stop it's
  /// left at until the rider can be at this one's boarding stop, walking or changing
  /// there. With the arrival of whichever departure of the ride before the rider takes,
  /// when they're ready to board this one. Null for the first ride.
  final int? transferSeconds;

  /// How late (or, below zero, early) it leaves the boarding stop, when known.
  final int? delaySeconds;

  /// Whether any of its times came from real-time data.
  final bool realtime;
  final TransitRoute? route;
  final String? agency;
  final String? headsign;

  /// The trip it rides, when the plan names it, and its service day as `YYYYMMDD`:
  /// with [tripId], which run of the trip it is.
  final String? tripId;
  final String? startDate;

  /// What real-time data did to the trip: `delayed`, `stop_skipped`, `replaced`, or
  /// `added`.
  final String? tripState;

  /// `yes`, `no`, or null when the feed doesn't say.
  final String? bikesAllowed;

  /// Every stop from boarding to alighting.
  final List<StopVisit> stops;

  /// The trips of a stay-seated ride, or empty for a ride on one trip.
  final List<RideSegment> segments;
  final String? _shape;
  final TransitVehicle? vehicle;
  final List<LegAlert> alerts;

  /// Departures between the same two stops, by departure, around and including this
  /// ride's own: what's left if the rider misses it. Most are of the ride's route; one of
  /// another route that also runs from the boarding stop to the alighting stop says so
  /// ([RideDeparture.route]). Empty if the plan didn't list any.
  final List<RideDeparture> departures;

  /// This ride's own departure among [departures], if they're listed.
  RideDeparture? get suggested =>
      departures.where((d) => d.suggested).firstOrNull;

  StopVisit get boarding => stops.first;
  StopVisit get alighting => stops.last;

  /// Stops passed between boarding and alighting.
  int get stopsBetween => stops.length > 2 ? stops.length - 2 : 0;

  @override
  late final List<DirectionsPoint> shape = _shape != null
      ? decodePolyline6(_shape)
      : [for (final s in stops) ?s.stop.point];
}

/// A route, as riders know it.
class TransitRoute {
  TransitRoute.fromJson(Map<String, Object?> json)
    : id = json['route_id'] as String? ?? '',
      shortName = json['short_name'] as String?,
      longName = json['long_name'] as String?,
      type = (json['route_type'] as num?)?.toInt() ?? 3,
      color = json['color'] as String?,
      textColor = json['text_color'] as String?;

  /// GTFS `route_id`, unique within its feed.
  final String id;
  final String? shortName;
  final String? longName;

  /// GTFS `route_type`, including extended values.
  final int type;

  /// `RRGGBB`, when the feed gives one.
  final String? color;
  final String? textColor;

  TransitRoute._copy(TransitRoute other, {this.color, this.textColor})
    : id = other.id,
      shortName = other.shortName,
      longName = other.longName,
      type = other.type;

  /// What a rider calls it: the short name, else the long one.
  String get name => shortName ?? longName ?? '';

  /// This route in [color] and [textColor] if it has no color of its own.
  TransitRoute withColors(String? color, String? textColor) =>
      this.color != null
      ? this
      : TransitRoute._copy(this, color: color, textColor: textColor);

  VehicleKind get kind => VehicleKind.of(type);
}

/// What sort of vehicle a route runs, for its icon and name.
enum VehicleKind {
  bus('Bus'),
  tram('Light rail'),
  subway('Subway'),
  rail('Train'),
  ferry('Ferry'),
  cableway('Cable car'),
  other('Transit');

  const VehicleKind(this.label);
  final String label;

  /// From GTFS's `route_type`, including the extended values.
  static VehicleKind of(int type) => switch (type) {
    0 || 5 || 11 || >= 900 && < 1000 => tram,
    1 || >= 400 && < 500 => subway,
    2 || 12 || >= 100 && < 200 => rail,
    3 || >= 200 && < 300 || >= 700 && < 900 => bus,
    4 || >= 1000 && < 1300 => ferry,
    6 || 7 || >= 1300 && < 1500 => cableway,
    _ => other,
  };
}

/// A stop, as a rider sees it.
class StopRef {
  StopRef.fromJson(Map<String, Object?> json)
    : id = json['stop_id'] as String? ?? '',
      feed = json['feed'] as String?,
      name = json['name'] as String? ?? json['stop_id'] as String? ?? '',
      code = json['code'] as String?,
      platform = json['platform'] as String?,
      accessibility = Accessibility.parse(json['accessibility']),
      point = json['latitude'] is num && json['longitude'] is num
          ? DirectionsPoint(
              lat: (json['latitude'] as num).toDouble(),
              lng: (json['longitude'] as num).toDouble(),
            )
          : null;

  /// GTFS `stop_id`, unique within its [feed].
  final String id;
  final String? feed;
  final String name;
  final String? code;
  final String? platform;
  final Accessibility accessibility;
  final DirectionsPoint? point;
}

/// A stop on a ride, with its times.
class StopVisit {
  StopVisit._(Map<String, Object?> json)
    : stop = StopRef.fromJson(json),
      arrival = _time(json['arrival']),
      departure = _time(json['departure']),
      scheduledArrival = _optionalTime(json['scheduled_arrival']),
      scheduledDeparture = _optionalTime(json['scheduled_departure']),
      predicted =
          json['predicted_arrival'] != null ||
          json['predicted_departure'] != null,
      arrivalDelay = (json['arrival_delay_seconds'] as num?)?.toInt(),
      departureDelay = (json['departure_delay_seconds'] as num?)?.toInt(),
      skipped = json['skipped'] == true,
      utcOffset = (json['utc_offset_seconds'] as num?)?.toInt();

  final StopRef stop;
  final DateTime arrival;
  final DateTime departure;
  final DateTime? scheduledArrival;
  final DateTime? scheduledDeparture;

  /// Whether real-time data predicted its times.
  final bool predicted;
  final int? arrivalDelay;
  final int? departureDelay;
  final bool skipped;

  /// The stop's offset from UTC at the visit, in seconds, so its local time can be shown.
  final int? utcOffset;
}

/// One trip of a stay-seated ride.
class RideSegment {
  RideSegment._(Map<String, Object?> json)
    : route = json['route'] == null
          ? null
          : TransitRoute.fromJson(_map(json['route'])),
      headsign = json['headsign'] as String?,
      firstStop = (json['first_stop_index'] as num).toInt(),
      lastStop = (json['last_stop_index'] as num).toInt();

  final TransitRoute? route;
  final String? headsign;

  /// The range of the leg's stops it covers, inclusive.
  final int firstStop;
  final int lastStop;
}

/// Whether a departure runs, and whether real-time data has seen it.
enum DepartureStatus {
  scheduled,
  live,
  canceled,
  skipped;

  static DepartureStatus parse(Object? value) => switch (value) {
    'live' => live,
    'canceled' => canceled,
    'skipped' => skipped,
    _ => scheduled,
  };

  /// Whether it runs and lets riders on and off at the ride's stops.
  bool get runs => this == scheduled || this == live;
}

/// One departure of a route from a stop.
class StopDeparture {
  /// Reads [json], whose `alerts` index [alerts].
  StopDeparture.fromJson(
    Map<String, Object?> json, {
    List<LegAlert> alerts = const [],
  }) : this._(json, _indexed(json['alerts'], alerts));

  StopDeparture._(Map<String, Object?> json, this.alerts)
    : tripId = json['trip_id'] as String? ?? '',
      startDate = json['start_date'] as String?,
      headsign = json['headsign'] as String?,
      status = DepartureStatus.parse(json['status']),
      departure = _time(json['departure']),
      scheduledDeparture = _optionalTime(json['scheduled_departure']),
      delaySeconds = (json['delay_seconds'] as num?)?.toInt(),
      realtime = json['realtime'] == true,
      vehicleId = json['vehicle_id'] as String?,
      accessibility = Accessibility.parse(json['accessibility']),
      bikesAllowed = json['bikes_allowed'] as String?;

  static List<LegAlert> _indexed(Object? indices, List<LegAlert> alerts) => [
    for (final i in _list(indices))
      if (i is int && i >= 0 && i < alerts.length) alerts[i],
  ];

  final String tripId;

  /// The trip's service day, as `YYYYMMDD`: with [tripId], which run of the trip it is.
  final String? startDate;

  /// Its headsign at the stop; on a ride, null when it's the ride's own.
  final String? headsign;
  final DepartureStatus status;

  /// When it leaves the stop: the prediction when there is one, the schedule for a
  /// canceled or skipped one.
  final DateTime departure;
  final DateTime? scheduledDeparture;

  /// How late (or, below zero, early) it leaves the stop, when known.
  final int? delaySeconds;

  /// Whether its times came from real-time data.
  final bool realtime;
  final String? vehicleId;

  /// Whether its trip and stop can be used in a wheelchair.
  final Accessibility accessibility;

  /// `yes`, `no`, or null when the feed doesn't say.
  final String? bikesAllowed;

  /// The service alerts that apply to it.
  final List<LegAlert> alerts;
}

/// One departure from a ride's boarding stop to its alighting stop: of the ride's route,
/// or of another that runs between them.
class RideDeparture extends StopDeparture {
  RideDeparture._(
    Map<String, Object?> json,
    List<LegAlert> legAlerts,
    List<LegAlert> departureAlerts,
  ) : route = json['route'] == null
          ? null
          : TransitRoute.fromJson(_map(json['route'])),
      arrival = _time(json['arrival']),
      scheduledArrival = _optionalTime(json['scheduled_arrival']),
      suggested = json['suggested'] == true,
      missed = json['missed'] == true,
      lastOfDay = json['last_of_day'] == true,
      super._(json, [
        ...StopDeparture._indexed(json['alerts'], legAlerts),
        ...StopDeparture._indexed(json['departure_alerts'], departureAlerts),
      ]);

  /// Its route when that isn't the ride's: null for one of the ride's own route.
  final TransitRoute? route;

  /// When it reaches the alighting stop, as [departure] leaves the boarding stop.
  final DateTime arrival;
  final DateTime? scheduledArrival;

  /// Whether it's the one the journey takes.
  final bool suggested;

  /// Whether it leaves before the rider can be at the stop, as the plan has the journey.
  final bool missed;

  /// Whether it's the last departure between these stops on its service day, of any
  /// route.
  final bool lastOfDay;
}

/// A vehicle real-time data names: the one running a ride, or one on a route.
class TransitVehicle {
  TransitVehicle.fromJson(Map<String, Object?> json)
    : id = json['id'] as String?,
      label = json['label'] as String? ?? json['id'] as String?,
      tripId = json['trip_id'] as String?,
      point = json['latitude'] is num && json['longitude'] is num
          ? DirectionsPoint(
              lat: (json['latitude'] as num).toDouble(),
              lng: (json['longitude'] as num).toDouble(),
            )
          : null,
      bearing = (json['bearing'] as num?)?.toDouble(),
      occupancy = json['occupancy'] as String?,
      timestamp = _optionalTime(json['timestamp']);

  final String? id;
  final String? label;

  /// The trip it's running, when the feed says.
  final String? tripId;

  /// Where it was at [timestamp], when the feed says.
  final DirectionsPoint? point;

  /// Degrees clockwise from north.
  final double? bearing;

  /// GTFS-RT's occupancy status, lowercase, such as `many_seats_available`.
  final String? occupancy;
  final DateTime? timestamp;
}

/// A service alert on a ride.
class LegAlert {
  LegAlert.fromJson(Map<String, Object?> json)
    : effect = json['effect'] as String?,
      severity = json['severity'] as String?,
      header = _text(json['header']),
      description = _text(json['description']),
      url = _text(json['url']);

  /// GTFS-RT's effect, lowercase, such as `detour` or `stop_moved`.
  final String? effect;
  final String? severity;
  final String? header;
  final String? description;
  final String? url;
}

/// A `STOP_MOVED` alert about a stop a walk goes to or from. The new place is only ever
/// said in words.
class StopMovedNotice {
  StopMovedNotice._(Map<String, Object?> json)
    : atEnd = json['walk_end'] == 'end',
      stopName = json['name'] as String?,
      header = _text(json['header']),
      description = _text(json['description']);

  /// Whether the walk goes to the stop (rather than from it).
  final bool atEnd;
  final String? stopName;
  final String? header;
  final String? description;
}

/// A walk or ride's path on the streets.
class WalkPath {
  WalkPath._(Map<String, Object?> json)
    : distance = (json['distance_meters'] as num).toDouble(),
      _shape = json['shape'] as String,
      maneuvers = [
        for (final m in _list(json['maneuvers']))
          WalkManeuver(
            _map(m)['instruction'] as String? ?? '',
            (_map(m)['distance_meters'] as num?)?.toDouble() ?? 0,
          ),
      ];

  /// Meters.
  final double distance;
  final String _shape;
  final List<WalkManeuver> maneuvers;

  late final List<DirectionsPoint> shape = decodePolyline6(_shape);
}

/// One instruction along a walk.
class WalkManeuver {
  const WalkManeuver(this.instruction, this.distance);

  final String instruction;
  final double distance;
}

/// A polyline encoded with six decimal places, as Valhalla's shapes are.
List<DirectionsPoint> decodePolyline6(String encoded) {
  final points = <DirectionsPoint>[];
  var index = 0, lat = 0, lng = 0;
  int next() {
    var result = 0, shift = 0, byte = 0;
    do {
      byte = encoded.codeUnitAt(index++) - 63;
      result |= (byte & 0x1f) << shift;
      shift += 5;
    } while (byte >= 0x20 && index < encoded.length);
    return (result & 1) != 0 ? ~(result >> 1) : result >> 1;
  }

  while (index < encoded.length) {
    lat += next();
    if (index >= encoded.length) break;
    lng += next();
    points.add(DirectionsPoint(lat: lat / 1e6, lng: lng / 1e6));
  }
  return points;
}

/// The first text of a translated string: the feed's own language comes first.
String? _text(Object? translations) {
  final list = _list(translations);
  if (list.isEmpty) return null;
  final text = _map(list.first)['text'] as String?;
  return text == null || text.trim().isEmpty ? null : text.trim();
}

DateTime _time(Object? seconds) =>
    DateTime.fromMillisecondsSinceEpoch((seconds as num).toInt() * 1000);

DateTime? _optionalTime(Object? seconds) =>
    seconds == null ? null : _time(seconds);

Map<String, Object?> _map(Object? value) =>
    value is Map<String, Object?> ? value : const {};

List<Object?> _list(Object? value) => value is List<Object?> ? value : const [];
