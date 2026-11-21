/// Distances and durations, as the directions sheet and the map's callouts show them,
/// and what the sheet says of a route among the others.
library;

import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../units.dart';
import 'route_tones.dart';

/// [meters] for reading: miles from a tenth of one and feet below that, or kilometers from
/// one and meters below that.
///
/// [units] is [UnitSystem.metric] or [UnitSystem.imperial], already resolved for the
/// device's region, so that the app's one rule for which regions use miles decides.
String formatDistance(double meters, {required UnitSystem units}) {
  assert(units != UnitSystem.locale, 'resolve the units for a locale first');
  String tenths(double value) =>
      value >= 10 ? value.round().toString() : value.toStringAsFixed(1);
  if (units == UnitSystem.imperial) {
    final inMiles = meters / 1609.344;
    return inMiles >= 0.1
        ? '${tenths(inMiles)} mi'
        : '${(meters * 3.28084).round()} ft';
  }
  return meters >= 1000 ? '${tenths(meters / 1000)} km' : '${meters.round()} m';
}

/// [seconds] to the nearest minute, as `11 min` or `2 hr 3 min`; `<1 min` if it's less.
String formatDuration(double seconds) {
  final minutes = (seconds / 60).round();
  if (minutes < 1) return '<1 min';
  final hours = minutes ~/ 60;
  final rest = minutes % 60;
  if (hours == 0) return '$minutes min';
  return rest == 0 ? '$hours hr' : '$hours hr $rest min';
}

/// [items] as one list, the way the language of [words] writes one: "A", "A and B",
/// "A, B, and C".
String formatList(AppLocalizations words, List<String> items) {
  switch (items) {
    case []:
      return '';
    case [final only]:
      return only;
    case [final first, final second]:
      return words.listTwo(first, second);
  }
  final last = items.length - 1;
  var rest = words.listEnd(items[last - 1], items[last]);
  for (var i = last - 2; i > 0; i--) {
    rest = words.listMiddle(items[i], rest);
  }
  return words.listStart(items.first, rest);
}

/// What a route is called in a list of routes: "via" and the roads it's named for, or
/// null if it has none.
String? routeName(AppLocalizations words, DirectionsRoute route) =>
    route.roadNames.isEmpty
    ? null
    : words.directionsVia(formatList(words, route.roadNames));

/// How far a route climbs, [meters], for reading: to the nearest 10 ft, or the nearest
/// 5 m. Null if that's nothing.
String? formatClimb(double meters, {required UnitSystem units}) {
  assert(units != UnitSystem.locale, 'resolve the units for a locale first');
  final (amount, step, unit) = units == UnitSystem.imperial
      ? (meters * 3.28084, 10, 'ft')
      : (meters, 5, 'm');
  final rounded = (amount / step).round() * step;
  return rounded > 0 ? '$rounded $unit' : null;
}

/// How much less a route must climb than every other for it to have the least
/// climbing: at least this many meters, and [leastClimbingShare] of the other's climb.
const double leastClimbingMeters = 15;

/// See [leastClimbingMeters].
const double leastClimbingShare = 0.2;

/// The index of the one of [routes] that climbs clearly less than all the others (by
/// [leastClimbingMeters] and [leastClimbingShare]), or null if there are fewer than
/// two, any of them doesn't say how far it climbs, or none climbs clearly least.
int? leastClimbing(List<DirectionsRoute> routes) {
  if (routes.length < 2 || routes.any((route) => route.elevation == null)) {
    return null;
  }
  final climbs = [for (final route in routes) route.elevation!.climb];
  var least = 0;
  for (var i = 1; i < climbs.length; i++) {
    if (climbs[i] < climbs[least]) least = i;
  }
  for (var i = 0; i < climbs.length; i++) {
    if (i == least) continue;
    final less = climbs[i] - climbs[least];
    if (less < leastClimbingMeters || less < leastClimbingShare * climbs[i]) {
      return null;
    }
  }
  return least;
}

/// What's said under the route at [index] of [routes], the fastest first: how far it
/// goes, then how it differs from the others. In order: its distance; "Fastest" for the
/// first, or how much slower it is; "Shortest", unless it's the only one; then:
///
/// - if [driving], "Ferry", "No freeways" if another of the routes takes one, and
///   "Tolls" unless [tollOnProfile] says its profile has "Toll" written on it;
/// - for a ride or a walk, how far it climbs ("540 ft climb"), or "Least climbing"
///   with it ("Least climbing, 380 ft") if it climbs clearly least of them (see
///   [leastClimbing]); then for a ride "Mostly trails" if over half of it is, and for
///   a walk "Stairs" if it takes any. A walk says nothing of its share of paths: a
///   sidewalk mapped as a way of its own counts as one.
List<String> routeFacts(
  List<DirectionsRoute> routes,
  int index, {
  required bool driving,
  required UnitSystem units,
  bool tollOnProfile = false,
}) {
  final route = routes[index];
  int minutes(DirectionsRoute route) => (route.duration / 60).round();
  // By the minutes each shows, so the two add up.
  final slower = minutes(route) - minutes(routes.first);
  final shortest = routes.indexWhere(
    (other) => !routes.any((r) => r.distance < other.distance),
  );
  return [
    formatDistance(route.distance, units: units),
    if (index == 0)
      'Fastest'
    else if (slower >= 0)
      '${formatDuration(slower * 60)} slower',
    if (routes.length > 1 && index == shortest) 'Shortest',
    if (driving) ...[
      if (route.hasFerry) 'Ferry',
      if (!route.hasMotorway && routes.any((other) => other.hasMotorway))
        'No freeways',
      if (route.hasToll && !tollOnProfile) 'Tolls',
    ] else
      ..._groundFacts(routes, index, units),
  ];
}

/// What [routeFacts] says of a ride's or a walk's ground and ways.
List<String> _groundFacts(
  List<DirectionsRoute> routes,
  int index,
  UnitSystem units,
) {
  final route = routes[index];
  final elevation = route.elevation;
  final climb = elevation == null
      ? null
      : formatClimb(elevation.climb, units: units);
  final kinds = WayKinds.of(route);
  return [
    if (leastClimbing(routes) == index)
      climb == null ? 'Least climbing' : 'Least climbing, $climb'
    else if (climb != null)
      '$climb climb',
    if (kinds == WayKinds.cycling && route.trailShare > 0.5) 'Mostly trails',
    if (kinds == WayKinds.walking && route.hasStairs) 'Stairs',
  ];
}

/// [routeFacts] as the one line a route's row shows.
String routeCaption(
  List<DirectionsRoute> routes,
  int index, {
  required bool driving,
  required UnitSystem units,
  bool tollOnProfile = false,
}) => routeFacts(
  routes,
  index,
  driving: driving,
  units: units,
  tollOnProfile: tollOnProfile,
).join(' · ');
