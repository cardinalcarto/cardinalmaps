/// Navigation's Android notification: the foreground service's, which keeps the trip
/// going with the screen off, showing the next maneuver, or what the trip's status card
/// says, with End.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show IconData;
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../l10n/lookup.dart';
import '../store/store.dart';
import '../units.dart';
import 'maneuver_icon.dart';
import 'navigation_controller.dart';
import 'navigation_format.dart';
import 'navigation_location.dart';
import 'navigation_status.dart';

/// What the navigation notification says.
@immutable
class NavigationNotice {
  const NavigationNotice({
    required this.title,
    this.text,
    this.glyph,
    required this.channel,
    required this.end,
  });

  /// The first line: the distance to the maneuver, or what the status card says.
  final String title;

  /// The line under it: the maneuver's instruction, or the status card's detail.
  final String? text;

  /// The maneuver's icon, in the app's icon font, drawn as the large icon.
  final IconData? glyph;

  /// The notification channel's name, as the system's settings list it.
  final String channel;

  /// The End action's label.
  final String end;

  Map<String, Object?> toMap() => {
    'title': title,
    'text': text,
    'glyph': glyph?.codePoint,
    'channel': channel,
    'end': end,
  };

  @override
  bool operator ==(Object other) =>
      other is NavigationNotice &&
      other.title == title &&
      other.text == text &&
      other.glyph == glyph &&
      other.channel == channel &&
      other.end == end;

  @override
  int get hashCode => Object.hash(title, text, glyph, channel, end);

  @override
  String toString() => 'NavigationNotice($title, $text, $glyph)';
}

/// What the notification says for [controller]'s trip, in [words], with distances in
/// [units] (metric or imperial); null when there's no trip.
///
/// It says what the navigation screen's banner does:
///
/// - the next maneuver: the distance to it, rounded as the banner rounds it, over its
///   primary instruction, with its icon; the instruction alone before there's a
///   distance, and "Navigating" before there's a maneuver;
/// - in the banner's place, a status card's title and detail ([navigationStatusOf]),
///   such as "Finding a new route…", with no icon;
/// - once arrived, "You've arrived" over the destination's name, with the arrival's
///   icon.
NavigationNotice? navigationNoticeOf(
  NavigationController controller, {
  required AppLocalizations words,
  required UnitSystem units,
}) {
  NavigationNotice notice(String title, [String? text, IconData? glyph]) =>
      NavigationNotice(
        title: title,
        text: text,
        glyph: glyph,
        channel: words.navigationNotificationChannel,
        end: words.navigationNotificationEnd,
      );

  final state = controller.state;
  if (state == NavigationState.idle) return null;
  if (state == NavigationState.arrived) {
    return notice(
      words.navigationArrived,
      controller.destinationName,
      maneuverIcon(
            ManeuverType.arrive,
            ArrivalContent.sideOf(controller.steps),
          ) ??
          maneuverIcon(ManeuverType.arrive, null),
    );
  }
  final status = navigationStatusOf(controller);
  if (status != null) {
    return switch (status) {
      NavigationStatus.locationProblem => switch (controller.locationProblem ??
          LocationProblem.failed) {
        LocationProblem.off => notice(
          words.navigationLocationOff,
          words.navigationLocationOffDetail,
        ),
        LocationProblem.denied => notice(
          words.navigationLocationDenied,
          words.navigationLocationDeniedDetail,
        ),
        LocationProblem.failed => notice(words.navigationLocationFailed),
      },
      NavigationStatus.rerouting => notice(words.navigationRerouting),
      NavigationStatus.rerouteFailed => notice(
        switch (controller.rerouteFailure ?? RerouteFailure.failed) {
          RerouteFailure.noRoute => words.navigationRerouteNoRoute,
          RerouteFailure.outsideOfflineArea =>
            words.navigationRerouteOutsideArea,
          RerouteFailure.noOfflineArea => words.navigationRerouteNoOfflineArea,
          RerouteFailure.failed => words.navigationRerouteFailed,
        },
        words.navigationRerouteRetrying,
      ),
      NavigationStatus.offRoute => notice(words.navigationOffRoute),
      NavigationStatus.acquiring => notice(words.navigationWaitingForGps),
    };
  }
  final update = controller.update;
  final primary = update?.visualInstruction?.primary;
  if (primary == null) return notice(words.navigationNotificationText);
  final glyph = maneuverIcon(primary.maneuverType, primary.maneuverModifier);
  final distance = update?.progress?.distanceToNextManeuver;
  if (distance == null) return notice(primary.text, null, glyph);
  return notice(
    formatManeuverDistance(distance, units: units, words: words),
    primary.text,
    glyph,
  );
}

/// What shows the navigation notification: the platform's, or a fake in tests.
abstract class NavigationNotifier {
  /// Shows [notice], starting what keeps navigation going with the screen off if it
  /// isn't running yet.
  Future<void> show(NavigationNotice notice);

  /// Takes the notification away, stopping what keeps navigation going.
  Future<void> hide();

  /// Asks for permission to show notifications, where the platform asks; whether
  /// they may be shown.
  Future<bool> requestPermission();

  /// Called when the notification's End is tapped.
  set onEnd(VoidCallback? onEnd);
}

/// [NavigationNotifier] on Android, through the app's native code on `cardinal/navigation`
/// (`NavigationChannels.kt`), whose foreground service (`NavigationService.kt`) shows
/// the notification.
class AndroidNavigationNotifier implements NavigationNotifier {
  AndroidNavigationNotifier() {
    channel.setMethodCallHandler((call) async {
      if (call.method == 'end') _onEnd?.call();
    });
  }

  static const channel = MethodChannel('cardinal/navigation');

  VoidCallback? _onEnd;

  @override
  set onEnd(VoidCallback? onEnd) => _onEnd = onEnd;

  @override
  Future<void> show(NavigationNotice notice) =>
      _quietly(() => channel.invokeMethod<void>('show', notice.toMap()));

  @override
  Future<void> hide() => _quietly(() => channel.invokeMethod<void>('hide'));

  @override
  Future<bool> requestPermission() async =>
      await _quietly(
        () => channel.invokeMethod<bool>('requestNotificationPermission'),
      ) ??
      false;

  static Future<T?> _quietly<T>(Future<T?> Function() call) async {
    try {
      return await call();
    } on Object catch (e) {
      // Navigation goes on without its notification.
      debugPrint('navigation: notification: $e');
      return null;
    }
  }
}

/// Whether the notification permission has been asked for as a trip started, so it's
/// asked once and never again, whatever the answer.
final navigationNotificationAsked = Setting.flag(
  'navigation.notificationAsked',
  false,
);

/// Keeps the navigation notification in step with [controller]'s trip, through
/// [notifier].
///
/// From when a trip starts until it ends, it shows [navigationNoticeOf] the trip,
/// again only when that changes, as when the maneuver or its rounded distance does,
/// not at each fix. When the trip ends it hides it.
///
/// As the first trip starts, it asks for permission to notify, once
/// ([navigationNotificationAsked]); whatever the answer, the trip goes on, and it's
/// never asked again.
///
/// End on the notification ends the trip ([NavigationController.end]); the navigation
/// screen goes back to directions as the trip ends, now if the app is showing, or as
/// it's next opened.
class TripNotification {
  TripNotification({
    required this.controller,
    required this.notifier,
    required this.settings,
    AppLocalizations Function()? words,
  }) : _words = words ?? deviceLocalizations;

  final NavigationController controller;
  final NavigationNotifier notifier;
  final SettingsStore settings;
  final AppLocalizations Function() _words;

  /// The [unitSystem] setting, until it's read: the device region's.
  UnitSystem _units = UnitSystem.locale;
  StreamSubscription<UnitSystem>? _watchUnits;

  /// What was last shown; null while hidden.
  NavigationNotice? _shown;

  /// Starts following the controller.
  void start() {
    notifier.onEnd = _end;
    controller.addListener(_changed);
    _watchUnits = settings.watch(unitSystem).listen((units) {
      if (units == _units) return;
      _units = units;
      _changed();
    });
    _changed();
  }

  void dispose() {
    notifier.onEnd = null;
    controller.removeListener(_changed);
    unawaited(_watchUnits?.cancel());
  }

  void _changed() {
    final notice = navigationNoticeOf(
      controller,
      words: _words(),
      units: _units.onDevice,
    );
    if (notice == _shown) return;
    final first = _shown == null;
    _shown = notice;
    if (notice == null) {
      unawaited(notifier.hide());
      return;
    }
    unawaited(notifier.show(notice));
    if (first) unawaited(_askOnce());
  }

  Future<void> _askOnce() async {
    if (await settings.get(navigationNotificationAsked)) return;
    await settings.set(navigationNotificationAsked, true);
    await notifier.requestPermission();
  }

  void _end() => unawaited(controller.end());
}
