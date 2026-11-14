// ignore: unused_import
import 'package:intl/intl.dart' as intl;

import 'app_localizations.dart';

// ignore_for_file: type=lint

/// The translations for English (`en`).
class AppLocalizationsEn extends AppLocalizations {
  AppLocalizationsEn([String locale = 'en']) : super(locale);

  @override
  String get rerouting => 'Rerouting';

  @override
  String tripHoursMinutes(int hours, int minutes) {
    return '$hours hr $minutes min';
  }

  @override
  String tripHours(int hours) {
    return '$hours hr';
  }

  @override
  String tripMinutes(int minutes) {
    return '$minutes min';
  }

  @override
  String get tripUnderAMinute => '<1 min';

  @override
  String get navigationRouteOverview => 'Route overview';

  @override
  String get navigationRecenter => 'Recenter';

  @override
  String get navigationMute => 'Mute';

  @override
  String get navigationUnmute => 'Unmute';

  @override
  String get navigationEnd => 'End';

  @override
  String navigationDistanceMiles(String distance) {
    return '$distance mi';
  }

  @override
  String navigationDistanceFeet(String distance) {
    return '$distance ft';
  }

  @override
  String navigationDistanceKilometers(String distance) {
    return '$distance km';
  }

  @override
  String navigationDistanceMeters(String distance) {
    return '$distance m';
  }

  @override
  String get navigationNotificationText => 'Navigating';

  @override
  String get navigationNotificationChannel => 'Navigation';
}
