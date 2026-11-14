/// Distances as navigation shows them.
library;

import 'package:intl/intl.dart';

import '../units.dart';

/// [meters] to the next maneuver, rounded as Ferrostar's Android formatter rounds it
/// (`LocalizedDistanceFormatter` in `DistanceFormatter.kt`; see `NOTICE.md`).
///
/// Directions' `formatDistance` gives a route's length, read once; this is read at a
/// glance while it counts down, so it changes in round steps rather than every meter,
/// and stays in feet or meters until the turn is a few hundred meters off:
///
/// - metric: meters up to 1 km, to the nearest 5 up to 10 m, 10 up to 100 m and 100
///   above; then kilometers, to a tenth up to 10 km and whole above;
/// - imperial: feet up to 289 m (about 950 ft, which would round to 1,000), to the
///   nearest 5 below 50 ft, 10 below 100, 50 below 500 and 100 above; then miles, to a
///   tenth up to 10 mi and whole above.
///
/// A tenth that rounds to a whole number is shown without it, as `2 km`. Ferrostar's
/// third system, Britain's yards, isn't one of the app's [UnitSystem]s: Britain gets
/// [UnitSystem.imperial].
///
/// [units] is [UnitSystem.metric] or [UnitSystem.imperial], already resolved for the
/// device's region.
String formatManeuverDistance(double meters, {required UnitSystem units}) {
  assert(units != UnitSystem.locale, 'resolve the units for a locale first');
  if (units == UnitSystem.imperial) {
    if (meters > _imperialCutoff) {
      final miles = meters / _metersPerMile;
      return '${_number(miles, tenths: miles <= 10)} mi';
    }
    final feet = meters * _feetPerMeter;
    final step = feet < 50
        ? 5
        : feet < 100
        ? 10
        : feet < 500
        ? 50
        : 100;
    return '${_number(_nearest(feet, step), tenths: false)} ft';
  }
  if (meters > 1000) {
    return '${_number(meters / 1000, tenths: meters <= 10000)} km';
  }
  final step = meters > 100
      ? 100
      : meters > 10
      ? 10
      : 5;
  return '${_number(_nearest(meters, step), tenths: false)} m';
}

/// Up to where distances are in feet, in meters; miles beyond.
const _imperialCutoff = 289;
const _metersPerMile = 1609.344;
const _feetPerMeter = 3.28084;

double _nearest(double value, int step) => (value / step).round() * step * 1.0;

/// [value] with grouping, to a tenth if [tenths] and whole otherwise.
String _number(double value, {required bool tenths}) =>
    (NumberFormat.decimalPattern()..maximumFractionDigits = tenths ? 1 : 0)
        .format(value);
