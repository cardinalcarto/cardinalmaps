/// Distances, durations and times as navigation shows them.
library;

import 'package:intl/intl.dart';
import 'package:material_ui/material_ui.dart';

import '../l10n/app_localizations.dart';
import '../units.dart';

/// [meters] to the next maneuver, or to the destination, rounded as Ferrostar's
/// Android formatter rounds it (`LocalizedDistanceFormatter` in `DistanceFormatter.kt`;
/// see `NOTICE.md`). Ferrostar's trip progress shows the distance left with the same
/// formatter as the banner (`TripProgressView.kt:69`), so this is the trip-progress
/// pill's too.
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

/// [seconds] left of the trip, in whole hours and minutes with a unit left out when
/// it's zero, as Ferrostar's trip progress shows it (Android `DurationFormatter.kt:86-92`,
/// iOS `DefaultFormatters.swift:25-31`), but in the directions sheet's words: `1 hr 5 min`,
/// `2 hr` or `18 min`, and `<1 min` under a minute. Ferrostar's own `5m` reads as meters
/// beside a metric distance, and nothing or `0m` under a minute would read as missing.
///
/// It counts down rather than rounding to the nearest minute: the parts are truncated,
/// as Android's are (`DurationFormatter.kt:40-51`), so the minute shown is the one
/// you're in, and it doesn't say a minute is left when 31 seconds are. Hours don't roll
/// over into days on a long trip, as on both platforms: a day and a bit is `26 hr 5 min`.
///
/// The short forms are words, which other languages abbreviate their own way, so they
/// come from [words], the app's strings.
String formatTripDuration(double seconds, AppLocalizations words) {
  final whole = seconds.isFinite && seconds > 0 ? seconds.floor() : 0;
  final hours = whole ~/ 3600;
  final minutes = whole % 3600 ~/ 60;
  if (hours == 0) {
    return minutes == 0 ? words.tripUnderAMinute : words.tripMinutes(minutes);
  }
  if (minutes == 0) return words.tripHours(hours);
  return words.tripHoursMinutes(hours, minutes);
}

/// [when] as a clock time, as Ferrostar shows the arrival time: the locale's short time,
/// as `5:20 PM` or `17:20` (Android `DateTimeFormatter.kt:13-21`, iOS
/// `DefaultFormatters.swift:18-20`), in the 24-hour clock if the device is set to it
/// (`MediaQuery.alwaysUse24HourFormat`), whatever the locale's own habit.
String formatArrivalTime(BuildContext context, DateTime when) =>
    MaterialLocalizations.of(context).formatTimeOfDay(
      TimeOfDay.fromDateTime(when),
      alwaysUse24HourFormat: MediaQuery.alwaysUse24HourFormatOf(context),
    );
