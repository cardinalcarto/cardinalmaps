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
  String get navigationShowSteps => 'Show upcoming maneuvers';

  @override
  String get navigationHideSteps => 'Hide upcoming maneuvers';

  @override
  String navigationExitNumber(String number) {
    return 'Exit $number';
  }

  @override
  String navigationLanes(int active, int total) {
    String _temp0 = intl.Intl.pluralLogic(
      active,
      locale: localeName,
      other: '$active lanes of $total for this maneuver',
      one: '1 lane of $total for this maneuver',
    );
    return '$_temp0';
  }

  @override
  String get navigationSpeedLimitSign => 'SPEED LIMIT';

  @override
  String navigationSpeedLimit(String speed) {
    return 'Speed limit $speed';
  }

  @override
  String get navigationNoSpeedLimit => 'No speed limit';

  @override
  String navigationSpeedKph(String speed) {
    return '$speed km/h';
  }

  @override
  String navigationSpeedMph(String speed) {
    return '$speed mph';
  }

  @override
  String get navigationKph => 'km/h';

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

  @override
  String get navigationWaitingForGps => 'Waiting for GPS…';

  @override
  String get navigationOffRoute => 'Off route';

  @override
  String get navigationRerouting => 'Finding a new route…';

  @override
  String get navigationRerouteNoRoute => 'No route from here';

  @override
  String get navigationRerouteOutsideArea => 'Outside the offline area';

  @override
  String get navigationRerouteNoOfflineArea => 'No offline area to route in';

  @override
  String get navigationRerouteFailed => 'Couldn\'t find a new route';

  @override
  String get navigationRerouteRetrying => 'Will keep trying';

  @override
  String get navigationLocationOff => 'Location is off';

  @override
  String get navigationLocationOffDetail => 'Turn it on to keep navigating';

  @override
  String get navigationLocationDenied => 'Location is turned off for Cardinal';

  @override
  String get navigationLocationDeniedDetail => 'Allow it to keep navigating';

  @override
  String get navigationLocationFailed => 'Your location stopped updating';

  @override
  String get navigationOpenSettings => 'Open settings';

  @override
  String get navigationTryAgain => 'Try again';

  @override
  String get navigationGpsLost => 'GPS signal lost';

  @override
  String get navigationArrived => 'You\'ve arrived';
}
