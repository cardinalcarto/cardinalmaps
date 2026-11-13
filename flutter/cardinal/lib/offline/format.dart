/// How offline downloads are written out: sizes, and the names and dates of areas.
library;

import 'package:material_ui/material_ui.dart';

/// [bytes] for people, in decimal units as phones count storage: "136 MB", "12.4 GB".
/// Under 100 of a unit it has a decimal, unless that's a zero ("4 MB", not "4.0 MB").
/// The space is a no-break space.
String formatBytes(int bytes) {
  const units = ['KB', 'MB', 'GB', 'TB'];
  var value = bytes / 1000;
  var unit = 0;
  // Up a unit wherever rounding would make it 1000: 999.6 MB is 1 GB.
  while (value >= 999.5 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  final String number;
  if (value >= 99.95) {
    number = value.round().toString();
  } else {
    final fixed = value.toStringAsFixed(1);
    number = fixed.endsWith('.0')
        ? fixed.substring(0, fixed.length - 2)
        : fixed;
  }
  // A no-break space: "136 MB" never splits across lines.
  return '$number ${units[unit]}';
}

/// The name an area gets unless it's given another: when it was drawn, as "Nov 12,
/// 9:41 PM" in the device's language and clock.
String defaultAreaName(BuildContext context, DateTime when) {
  final words = MaterialLocalizations.of(context);
  final time = words.formatTimeOfDay(
    TimeOfDay.fromDateTime(when),
    alwaysUse24HourFormat: MediaQuery.alwaysUse24HourFormatOf(context),
  );
  return '${words.formatShortMonthDay(when)}, $time';
}

/// When an area was downloaded, for its row: "Nov 12", with the year if it isn't
/// [now]'s.
String areaDate(BuildContext context, DateTime when, {DateTime? now}) {
  final words = MaterialLocalizations.of(context);
  return when.year == (now ?? DateTime.now()).year
      ? words.formatShortMonthDay(when)
      : words.formatShortDate(when);
}
