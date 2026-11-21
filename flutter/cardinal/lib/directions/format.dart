/// Distances and durations, as the directions sheet and the map's callouts show them,
/// and what the sheet says of a route among the others.
library;

import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../units.dart';

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

/// What's said under the route at [index] of [routes], the fastest first: how far it
/// goes, then how it differs from the others. In order: its distance; "Fastest" for the
/// first, or how much slower it is; "Shortest", unless it's the only one; and, if
/// [driving], "Ferry", "No freeways" if another of the routes takes one, and "Tolls".
List<String> routeFacts(
  List<DirectionsRoute> routes,
  int index, {
  required bool driving,
  required UnitSystem units,
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
      if (route.hasToll) 'Tolls',
    ],
  ];
}

/// [routeFacts] as the one line a route's row shows.
String routeCaption(
  List<DirectionsRoute> routes,
  int index, {
  required bool driving,
  required UnitSystem units,
}) => routeFacts(routes, index, driving: driving, units: units).join(' · ');
