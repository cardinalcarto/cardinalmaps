/// Where fixes come from while navigating: the device's location service, with the screen
/// on or off, or a simulated trip along a route.
library;

import 'dart:async';
import 'dart:ui' show Locale, PlatformDispatcher;

import 'package:flutter/foundation.dart';
import 'package:geolocator/geolocator.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../l10n/lookup.dart';

/// Why fixes stopped coming.
enum LocationProblem {
  /// The device's location service is off.
  off,

  /// The app isn't allowed your location, or no longer is.
  denied,

  /// Anything else: the service failed, or stopped without saying why.
  failed,
}

/// An error on a [NavigationLocationSource]'s stream: why there are no fixes.
class NavigationLocationError implements Exception {
  const NavigationLocationError(this.problem, [this.cause]);

  final LocationProblem problem;

  /// What the platform said, for logs.
  final Object? cause;

  @override
  String toString() => 'NavigationLocationError($problem: $cause)';
}

/// Fixes for navigation, behind what the controller needs, so tests and development can
/// drive it without a location service.
abstract class NavigationLocationSource {
  /// Fixes while traveling by [mode], from when it's listened to until the subscription
  /// is cancelled, which also stops whatever keeps them coming with the screen off.
  ///
  /// When fixes stop, the stream has a [NavigationLocationError] saying why; it may also
  /// end after one, and is listened to again to start over.
  Stream<NavLocation> fixes(NavigationMode mode);
}

/// [NavigationLocationSource] from `geolocator`, as precise and as often as the device can
/// give them, kept coming with the screen off.
class GeolocatorNavigationSource implements NavigationLocationSource {
  const GeolocatorNavigationSource();

  /// How often to ask for a fix on Android: once a second, as GPS gives them, which is what
  /// Ferrostar's step and arrival distances assume.
  static const _interval = Duration(seconds: 1);

  @override
  Stream<NavLocation> fixes(NavigationMode mode) =>
      Geolocator.getPositionStream(locationSettings: _settings(mode))
          .map(navLocation)
          .handleError(_explain);

  static Never _explain(Object error, StackTrace stack) => switch (error) {
    LocationServiceDisabledException() => throw NavigationLocationError(
      LocationProblem.off,
      error,
    ),
    PermissionDeniedException() => throw NavigationLocationError(
      LocationProblem.denied,
      error,
    ),
    _ => throw NavigationLocationError(LocationProblem.failed, error),
  };

  /// The app's strings in the device's language, for the notification, which is
  /// shown with no widget tree to look them up in; English when the app has no
  /// translation for it, as the rest of the app falls back to.
  static AppLocalizations get _words =>
      localizationsFor(PlatformDispatcher.instance.locale.toLanguageTag()) ??
      lookupAppLocalizations(const Locale('en'));

  static LocationSettings _settings(NavigationMode mode) {
    switch (defaultTargetPlatform) {
      case TargetPlatform.android:
        return AndroidSettings(
          accuracy: LocationAccuracy.bestForNavigation,
          distanceFilter: 0,
          intervalDuration: _interval,
          // The platform's location manager rather than Google Play services', as for the
          // map's dot (`GeolocatorSource`), so Google's Location Accuracy is never asked
          // for.
          forceLocationManager: true,
          // A foreground service keeps fixes coming with the screen off, and its wake
          // lock keeps the CPU up to take them. It starts with the stream and stops when
          // the stream is cancelled. What the notification says is still to be designed.
          foregroundNotificationConfig: ForegroundNotificationConfig(
            // The app's name, which isn't translated.
            notificationTitle: 'Cardinal Maps',
            notificationText: _words.navigationNotificationText,
            notificationChannelName: _words.navigationNotificationChannel,
            enableWakeLock: true,
            setOngoing: true,
          ),
        );
      case TargetPlatform.iOS || TargetPlatform.macOS:
        return AppleSettings(
          accuracy: LocationAccuracy.bestForNavigation,
          distanceFilter: 0,
          activityType: _activity(mode),
          // Core Location would otherwise stop updates at a long red light, and only
          // start them again when the app comes to the foreground.
          pauseLocationUpdatesAutomatically: false,
          // Navigation always starts in the foreground, so "while using the app" is
          // enough to keep going in the background; the status bar shows it is.
          allowBackgroundLocationUpdates: true,
          showBackgroundLocationIndicator: true,
        );
      default:
        return const LocationSettings(
          accuracy: LocationAccuracy.bestForNavigation,
          distanceFilter: 0,
        );
    }
  }

  /// What Core Location is told the fixes are for, which tunes its filtering: a car stays
  /// on roads, while a bicycle and a walker don't.
  static ActivityType _activity(NavigationMode mode) => switch (mode) {
    NavigationMode.driving => ActivityType.automotiveNavigation,
    // Apple's category for navigating by any vehicle but a car.
    NavigationMode.cycling => ActivityType.otherNavigation,
    NavigationMode.walking => ActivityType.fitness,
  };
}

/// [p] as navigation takes it, leaving out what the platform didn't measure, so that a
/// placeholder of 0 isn't read as heading north or standing still.
@visibleForTesting
NavLocation navLocation(Position p) => NavLocation(
  lat: p.latitude,
  lng: p.longitude,
  // A fix of unknown accuracy can't be trusted to advance a step or leave the route.
  horizontalAccuracy: p.hasAccuracy && p.accuracy > 0
      ? p.accuracy
      : double.infinity,
  // Core Location gives -1 for a course or speed it doesn't have.
  course: p.hasHeading && p.heading >= 0 ? p.heading : null,
  courseAccuracy: p.hasHeadingAccuracy && p.headingAccuracy > 0
      ? p.headingAccuracy
      : null,
  speed: p.hasSpeed && p.speed >= 0 ? p.speed : null,
  speedAccuracy: p.hasSpeedAccuracy && p.speedAccuracy > 0
      ? p.speedAccuracy
      : null,
  timestampMs: p.timestamp.millisecondsSinceEpoch,
);

/// [NavigationLocationSource] that travels along [route], for development and tests: fixes
/// a second's travel apart at a typical speed for the mode, as GPS gives them, then the
/// last one over and over, as a phone left at the destination would give.
///
/// They come every [interval]: a second, for development; tests make it shorter, to
/// cover a route quickly with the same fixes.
class SimulatedLocationSource implements NavigationLocationSource {
  SimulatedLocationSource(
    this.route, {
    this.interval = const Duration(seconds: 1),
    this.bias = SimulationBias.none,
    this.biasMeters = 0,
    this.now = DateTime.now,
  });

  final NavigableRoute route;
  final Duration interval;

  /// Which side of the line fixes are set off to, and how far, to see the route snap.
  final SimulationBias bias;
  final double biasMeters;

  /// What fixes are stamped with, since the simulation's own times are when it was made.
  final DateTime Function() now;

  /// The speed each mode is simulated at, in meters per second: 50 km/h in a car, 18 km/h
  /// on a bicycle, and 5 km/h walking, the speeds the step and arrival distances in the
  /// routing library were chosen for.
  static double speed(NavigationMode mode) => switch (mode) {
    NavigationMode.driving => 50 / 3.6,
    NavigationMode.cycling => 18 / 3.6,
    NavigationMode.walking => 5 / 3.6,
  };

  /// The accuracy simulated fixes claim, in meters: a phone with a clear view of the sky.
  static const accuracy = 5.0;

  @override
  Stream<NavLocation> fixes(NavigationMode mode) {
    Timer? timer;
    late final StreamController<NavLocation> controller;
    controller = StreamController<NavLocation>(
      onListen: () {
        final LocationSimulation simulation;
        try {
          simulation = LocationSimulation(
            route: route,
            resampleDistance: speed(mode),
            bias: bias,
            biasMeters: biasMeters,
          );
        } on Object catch (e) {
          controller.addError(
            NavigationLocationError(LocationProblem.failed, e),
          );
          return;
        }
        controller.add(_stamped(simulation.current(), speed(mode)));
        timer = Timer.periodic(interval, (_) {
          if (simulation.isFinished()) {
            controller.add(_stamped(simulation.current(), 0));
          } else {
            controller.add(_stamped(simulation.advance(), speed(mode)));
          }
        });
      },
      onCancel: () => timer?.cancel(),
    );
    return controller.stream;
  }

  NavLocation _stamped(NavLocation fix, double speed) => NavLocation(
    lat: fix.lat,
    lng: fix.lng,
    horizontalAccuracy: accuracy,
    course: fix.course,
    courseAccuracy: fix.course == null ? null : 10,
    speed: speed,
    speedAccuracy: 1,
    timestampMs: now().millisecondsSinceEpoch,
  );
}

/// [NavigationLocationSource] that simulates each trip along the route it starts with,
/// for driving navigation on a desktop or an emulator without moving (see
/// `simulateNavigation` in `config.dart`). [route] is the trip's route when its fixes
/// are listened to: the controller's, as it starts.
///
/// A new route doesn't change where the fixes go: they keep following the route the trip
/// started with, as a driver who ignores the new route would.
class SimulatedTripLocation implements NavigationLocationSource {
  SimulatedTripLocation(this.route);

  final NavigableRoute Function() route;

  @override
  Stream<NavLocation> fixes(NavigationMode mode) =>
      SimulatedLocationSource(route()).fixes(mode);
}
