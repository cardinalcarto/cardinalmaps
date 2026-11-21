import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:intl/intl.dart' as intl;

import 'app_localizations_en.dart';

// ignore_for_file: type=lint

/// Callers can lookup localized strings with an instance of AppLocalizations
/// returned by `AppLocalizations.of(context)`.
///
/// Applications need to include `AppLocalizations.delegate()` in their app's
/// `localizationDelegates` list, and the locales they support in the app's
/// `supportedLocales` list. For example:
///
/// ```dart
/// import 'l10n/app_localizations.dart';
///
/// return MaterialApp(
///   localizationsDelegates: AppLocalizations.localizationsDelegates,
///   supportedLocales: AppLocalizations.supportedLocales,
///   home: MyApplicationHome(),
/// );
/// ```
///
/// ## Update pubspec.yaml
///
/// Please make sure to update your pubspec.yaml to include the following
/// packages:
///
/// ```yaml
/// dependencies:
///   # Internationalization support.
///   flutter_localizations:
///     sdk: flutter
///   intl: any # Use the pinned version from flutter_localizations
///
///   # Rest of dependencies
/// ```
///
/// ## iOS Applications
///
/// iOS applications define key application metadata, including supported
/// locales, in an Info.plist file that is built into the application bundle.
/// To configure the locales supported by your app, you’ll need to edit this
/// file.
///
/// First, open your project’s ios/Runner.xcworkspace Xcode workspace file.
/// Then, in the Project Navigator, open the Info.plist file under the Runner
/// project’s Runner folder.
///
/// Next, select the Information Property List item, select Add Item from the
/// Editor menu, then select Localizations from the pop-up menu.
///
/// Select and expand the newly-created Localizations item then, for each
/// locale your application supports, add a new item and select the locale
/// you wish to add from the pop-up menu in the Value field. This list should
/// be consistent with the languages listed in the AppLocalizations.supportedLocales
/// property.
abstract class AppLocalizations {
  AppLocalizations(String locale)
    : localeName = intl.Intl.canonicalizedLocale(locale.toString());

  final String localeName;

  static AppLocalizations of(BuildContext context) {
    return Localizations.of<AppLocalizations>(context, AppLocalizations)!;
  }

  static const LocalizationsDelegate<AppLocalizations> delegate =
      _AppLocalizationsDelegate();

  /// A list of this localizations delegate along with the default localizations
  /// delegates.
  ///
  /// Returns a list of localizations delegates containing this delegate along with
  /// GlobalMaterialLocalizations.delegate, GlobalCupertinoLocalizations.delegate,
  /// and GlobalWidgetsLocalizations.delegate.
  ///
  /// Additional delegates can be added by appending to this list in
  /// MaterialApp. This list does not have to be used at all if a custom list
  /// of delegates is preferred or required.
  static const List<LocalizationsDelegate<dynamic>> localizationsDelegates =
      <LocalizationsDelegate<dynamic>>[
        delegate,
        GlobalMaterialLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
      ];

  /// A list of this localizations delegate's supported locales.
  static const List<Locale> supportedLocales = <Locale>[Locale('en')];

  /// Spoken by turn-by-turn navigation, once, when you leave the route and a new one is about to be found.
  ///
  /// In en, this message translates to:
  /// **'Rerouting'**
  String get rerouting;

  /// How long is left of a trip, in whole hours and minutes, shown while navigating in a narrow space: short forms of hour and minute, the same as the directions sheet's.
  ///
  /// In en, this message translates to:
  /// **'{hours} hr {minutes} min'**
  String tripHoursMinutes(int hours, int minutes);

  /// How long is left of a trip when it's a whole number of hours, as tripHoursMinutes without the minutes.
  ///
  /// In en, this message translates to:
  /// **'{hours} hr'**
  String tripHours(int hours);

  /// How long is left of a trip when it's under an hour, as tripHoursMinutes without the hours.
  ///
  /// In en, this message translates to:
  /// **'{minutes} min'**
  String tripMinutes(int minutes);

  /// How long is left of a trip when it's less than a minute, as the directions sheet shows it.
  ///
  /// In en, this message translates to:
  /// **'<1 min'**
  String get tripUnderAMinute;

  /// The navigation screen's camera button while the map follows you, as its tooltip and spoken label: shows the rest of the trip on the map, flat and north up.
  ///
  /// In en, this message translates to:
  /// **'Route overview'**
  String get navigationRouteOverview;

  /// The navigation screen's camera button once the map shows the route overview or was moved by hand, as its tooltip and spoken label: brings the map back to following you.
  ///
  /// In en, this message translates to:
  /// **'Recenter'**
  String get navigationRecenter;

  /// The navigation screen's button that silences spoken directions, as its tooltip and spoken label, shown while they're being spoken.
  ///
  /// In en, this message translates to:
  /// **'Mute'**
  String get navigationMute;

  /// The navigation screen's button that lets spoken directions be heard again, as its tooltip and spoken label, shown while they're silenced.
  ///
  /// In en, this message translates to:
  /// **'Unmute'**
  String get navigationUnmute;

  /// The handle along the bottom of the navigation screen's maneuver banner, as its spoken label while closed: opens the banner into the list of the trip's maneuvers after the next one.
  ///
  /// In en, this message translates to:
  /// **'Show upcoming maneuvers'**
  String get navigationShowSteps;

  /// The handle at the bottom of the navigation screen's list of upcoming maneuvers, as its spoken label while the list is open: closes the list back into the maneuver banner.
  ///
  /// In en, this message translates to:
  /// **'Hide upcoming maneuvers'**
  String get navigationHideSteps;

  /// The small badge before a navigation maneuver's instruction naming the highway exit to take, as signs number exits. number is the exit's number or numbers as the road signs them, such as 154B or 1A/1B.
  ///
  /// In en, this message translates to:
  /// **'Exit {number}'**
  String navigationExitNumber(String number);

  /// The spoken label of the row of lane arrows under the navigation screen's maneuver banner: how many of the road's lanes can be used for the coming maneuver. active and total are numbers.
  ///
  /// In en, this message translates to:
  /// **'{active, plural, =1{1 lane of {total} for this maneuver} other{{active} lanes of {total} for this maneuver}}'**
  String navigationLanes(int active, int total);

  /// The words on the US-style speed-limit sign on the navigation screen, over the number, in capitals as the road sign has them. They're set on two lines in a narrow white rectangle, so keep them as short as the language's road signs do.
  ///
  /// In en, this message translates to:
  /// **'SPEED LIMIT'**
  String get navigationSpeedLimitSign;

  /// The spoken label of the speed-limit sign on the navigation screen. speed is the limit with its unit, such as 25 mph or 50 km/h.
  ///
  /// In en, this message translates to:
  /// **'Speed limit {speed}'**
  String navigationSpeedLimit(String speed);

  /// The spoken label of the end-of-restrictions sign on the navigation screen, on roads with no speed limit, such as parts of German autobahns.
  ///
  /// In en, this message translates to:
  /// **'No speed limit'**
  String get navigationNoSpeedLimit;

  /// A speed in kilometers per hour, in short form, as the speed-limit sign on the navigation screen gives it. speed is the number, already formatted for the language.
  ///
  /// In en, this message translates to:
  /// **'{speed} km/h'**
  String navigationSpeedKph(String speed);

  /// A speed in miles per hour, in short form, as the speed-limit sign on the navigation screen gives it. speed is the number, already formatted for the language.
  ///
  /// In en, this message translates to:
  /// **'{speed} mph'**
  String navigationSpeedMph(String speed);

  /// The unit under the number on the navigation screen's US-style speed-limit sign where limits are posted in kilometers per hour, as in Canada, in short form.
  ///
  /// In en, this message translates to:
  /// **'km/h'**
  String get navigationKph;

  /// The button at the end of the trip-progress bar that ends turn-by-turn navigation and goes back to directions, as its tooltip and spoken label.
  ///
  /// In en, this message translates to:
  /// **'End'**
  String get navigationEnd;

  /// A distance in miles while navigating, to the next turn or to the destination, in short form. distance is the number, already formatted for the language.
  ///
  /// In en, this message translates to:
  /// **'{distance} mi'**
  String navigationDistanceMiles(String distance);

  /// A distance in feet while navigating, to the next turn or to the destination, in short form. distance is the number, already formatted for the language.
  ///
  /// In en, this message translates to:
  /// **'{distance} ft'**
  String navigationDistanceFeet(String distance);

  /// A distance in kilometers while navigating, to the next turn or to the destination, in short form. distance is the number, already formatted for the language.
  ///
  /// In en, this message translates to:
  /// **'{distance} km'**
  String navigationDistanceKilometers(String distance);

  /// A distance in meters while navigating, to the next turn or to the destination, in short form. distance is the number, already formatted for the language.
  ///
  /// In en, this message translates to:
  /// **'{distance} m'**
  String navigationDistanceMeters(String distance);

  /// The title of the Android notification shown while turn-by-turn navigation is under way, when there's no maneuver to show in it yet.
  ///
  /// In en, this message translates to:
  /// **'Navigating'**
  String get navigationNotificationText;

  /// The action on the Android notification shown while navigating that ends turn-by-turn navigation.
  ///
  /// In en, this message translates to:
  /// **'End'**
  String get navigationNotificationEnd;

  /// The name of the Android notification channel for the notification shown while navigating, as the system's notification settings list it.
  ///
  /// In en, this message translates to:
  /// **'Navigation'**
  String get navigationNotificationChannel;

  /// The navigation screen's status card, where the maneuver banner goes, after the trip starts and before the first location fix good enough to start from.
  ///
  /// In en, this message translates to:
  /// **'Waiting for GPS…'**
  String get navigationWaitingForGps;

  /// The navigation screen's status card, where the maneuver banner goes, for the few seconds after you leave the route before a new one is asked for; the app finds the new route by itself.
  ///
  /// In en, this message translates to:
  /// **'Off route'**
  String get navigationOffRoute;

  /// The navigation screen's status card, where the maneuver banner goes, while a new route from where you are is being found, after you left the route.
  ///
  /// In en, this message translates to:
  /// **'Finding a new route…'**
  String get navigationRerouting;

  /// The navigation screen's status card, after a new route couldn't be found because the router knows no way from where you are, as when you're off any road it knows.
  ///
  /// In en, this message translates to:
  /// **'No route from here'**
  String get navigationRerouteNoRoute;

  /// The navigation screen's status card, after a new route couldn't be found because where you are is outside the downloaded offline area's streets.
  ///
  /// In en, this message translates to:
  /// **'Outside the offline area'**
  String get navigationRerouteOutsideArea;

  /// The navigation screen's status card, after a new route couldn't be found because there's no downloaded offline area to find one in.
  ///
  /// In en, this message translates to:
  /// **'No offline area to route in'**
  String get navigationRerouteNoOfflineArea;

  /// The navigation screen's status card, after a new route couldn't be found for any other reason.
  ///
  /// In en, this message translates to:
  /// **'Couldn\'t find a new route'**
  String get navigationRerouteFailed;

  /// The line under the navigation screen's status card that says a new route couldn't be found: the app tries again by itself, every so often, until it finds one or you're back on the route.
  ///
  /// In en, this message translates to:
  /// **'Will keep trying'**
  String get navigationRerouteRetrying;

  /// The navigation screen's status card when the device's location service was turned off during a trip, so there are no more location fixes.
  ///
  /// In en, this message translates to:
  /// **'Location is off'**
  String get navigationLocationOff;

  /// The line under the navigation screen's "Location is off" status card, above its Open settings button.
  ///
  /// In en, this message translates to:
  /// **'Turn it on to keep navigating'**
  String get navigationLocationOffDetail;

  /// The navigation screen's status card when the app's permission to use your location was taken away during a trip.
  ///
  /// In en, this message translates to:
  /// **'Location is turned off for Cardinal'**
  String get navigationLocationDenied;

  /// The line under the navigation screen's "Location is turned off for Cardinal" status card, above its Open settings button.
  ///
  /// In en, this message translates to:
  /// **'Allow it to keep navigating'**
  String get navigationLocationDeniedDetail;

  /// The navigation screen's status card when the device's location service failed or stopped sending fixes during a trip, for a reason it didn't say.
  ///
  /// In en, this message translates to:
  /// **'Your location stopped updating'**
  String get navigationLocationFailed;

  /// The button on the navigation screen's status card for location turned off or not allowed: opens the device's location settings, or the app's settings, to put it right.
  ///
  /// In en, this message translates to:
  /// **'Open settings'**
  String get navigationOpenSettings;

  /// The button on the navigation screen's status card for location that stopped updating: listens for location fixes again.
  ///
  /// In en, this message translates to:
  /// **'Try again'**
  String get navigationTryAgain;

  /// The small indicator under the navigation screen's map buttons when no location fix has come for a few seconds, as in a tunnel; it goes once fixes come again.
  ///
  /// In en, this message translates to:
  /// **'GPS signal lost'**
  String get navigationGpsLost;

  /// The navigation screen's banner at the end of a trip, above the destination's name, until you end the trip.
  ///
  /// In en, this message translates to:
  /// **'You\'ve arrived'**
  String get navigationArrived;

  /// The line under a route's travel time in the directions sheet's list of routes, naming the main roads the route takes. roads is one road's name, or several already joined as a list with listTwo, listStart, listMiddle and listEnd.
  ///
  /// In en, this message translates to:
  /// **'via {roads}'**
  String directionsVia(String roads);

  /// A list of exactly two things, such as two road names, as the language writes one (Unicode CLDR's list pattern for two).
  ///
  /// In en, this message translates to:
  /// **'{first} and {second}'**
  String listTwo(String first, String second);

  /// The start of a list of three or more things: its first thing, then the rest of the list, already joined (Unicode CLDR's list pattern start).
  ///
  /// In en, this message translates to:
  /// **'{first}, {rest}'**
  String listStart(String first, String rest);

  /// The middle of a list of four or more things: one of the things between its first and its last two, then the rest of the list, already joined (Unicode CLDR's list pattern middle).
  ///
  /// In en, this message translates to:
  /// **'{first}, {rest}'**
  String listMiddle(String first, String rest);

  /// The end of a list of three or more things: its last two (Unicode CLDR's list pattern end).
  ///
  /// In en, this message translates to:
  /// **'{first}, and {last}'**
  String listEnd(String first, String last);
}

class _AppLocalizationsDelegate
    extends LocalizationsDelegate<AppLocalizations> {
  const _AppLocalizationsDelegate();

  @override
  Future<AppLocalizations> load(Locale locale) {
    return SynchronousFuture<AppLocalizations>(lookupAppLocalizations(locale));
  }

  @override
  bool isSupported(Locale locale) =>
      <String>['en'].contains(locale.languageCode);

  @override
  bool shouldReload(_AppLocalizationsDelegate old) => false;
}

AppLocalizations lookupAppLocalizations(Locale locale) {
  // Lookup logic when only language code is specified.
  switch (locale.languageCode) {
    case 'en':
      return AppLocalizationsEn();
  }

  throw FlutterError(
    'AppLocalizations.delegate failed to load unsupported locale "$locale". This is likely '
    'an issue with the localizations generation tool. Please file an issue '
    'on GitHub with a reproducible sample app and the gen-l10n configuration '
    'that was used.',
  );
}
