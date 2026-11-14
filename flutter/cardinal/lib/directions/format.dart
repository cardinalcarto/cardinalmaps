/// Distances and durations, as the directions sheet and the map's callouts show them.
library;

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
