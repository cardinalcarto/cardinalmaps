/// Where fixes come from while navigating: the device's location service, with the screen
/// on or off, or a simulated trip along a route.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:geolocator/geolocator.dart';
import 'package:routing/routing.dart';

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

/// The device's [NavigationLocationSource]: [AndroidNavigationSource] on Android, and
/// [GeolocatorNavigationSource] elsewhere.
NavigationLocationSource deviceNavigationSource() =>
    defaultTargetPlatform == TargetPlatform.android
    ? const AndroidNavigationSource()
    : const GeolocatorNavigationSource();

/// How often navigation asks for a fix on Android: once a second, as GPS gives them,
/// which is what Ferrostar's step and arrival distances assume.
const androidFixInterval = Duration(seconds: 1);

/// [NavigationLocationSource] on Android: navigation's own location updates, from the
/// app's native code (`NavigationFixes.kt`) on [channel], every [androidFixInterval],
/// as precise as the device gives them, from the platform's location manager rather
/// than Google Play services'.
///
/// Not `geolocator`'s: it has a single position stream per app, which the map's
/// location dot already holds with its own looser settings when navigation starts, and
/// which it hands to any later caller whatever settings that asks for. Navigation would
/// get the dot's fixes, every 5 s at best, and they stop with the screen off.
///
/// Fixes keep coming with the screen off while the navigation notification's
/// foreground service runs (`NavigationService.kt`, see `TripNotification`),
/// from when a trip starts until it ends. With location turned off the stream stays
/// open, with a [LocationProblem.off] error, and fixes come again once it's on.
class AndroidNavigationSource implements NavigationLocationSource {
  const AndroidNavigationSource();

  static const channel = EventChannel('cardinal/navigation/fixes');

  @override
  Stream<NavLocation> fixes(NavigationMode mode) => channel
      .receiveBroadcastStream({'intervalMs': androidFixInterval.inMilliseconds})
      .map(
        (event) => navLocationFromMap((event as Map).cast<String, Object?>()),
      )
      .handleError(_explain);

  static Never _explain(Object error, StackTrace stack) => switch (error) {
    PlatformException(code: 'off') => throw NavigationLocationError(
      LocationProblem.off,
      error,
    ),
    PlatformException(code: 'denied') => throw NavigationLocationError(
      LocationProblem.denied,
      error,
    ),
    _ => throw NavigationLocationError(LocationProblem.failed, error),
  };
}

/// A fix as `NavigationFixes.kt` sends it: what the platform didn't measure is left
/// out, and 0 accuracies are taken as unknown, as for [navLocation].
@visibleForTesting
NavLocation navLocationFromMap(Map<String, Object?> fix) {
  double? measured(String key) {
    final value = (fix[key] as num?)?.toDouble();
    return value != null && value > 0 ? value : null;
  }

  final speed = (fix['speed'] as num?)?.toDouble();
  final heading = (fix['heading'] as num?)?.toDouble();
  return NavLocation(
    lat: (fix['latitude']! as num).toDouble(),
    lng: (fix['longitude']! as num).toDouble(),
    horizontalAccuracy: measured('accuracy') ?? double.infinity,
    course: heading != null && heading >= 0 ? heading : null,
    courseAccuracy: measured('heading_accuracy'),
    speed: speed != null && speed >= 0 ? speed : null,
    speedAccuracy: measured('speed_accuracy'),
    timestampMs: (fix['timestamp']! as num).toInt(),
  );
}

/// [NavigationLocationSource] from `geolocator`, as precise and as often as the device can
/// give them, kept coming with the screen off: on iOS, and on the desktop. Android has
/// [AndroidNavigationSource].
class GeolocatorNavigationSource implements NavigationLocationSource {
  const GeolocatorNavigationSource();

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

  static LocationSettings _settings(NavigationMode mode) {
    switch (defaultTargetPlatform) {
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
///
/// Development tools can steer it, as the navigation screenshots do to show the
/// screen's status cards: [paused] holds the fixes back, as a tunnel would;
/// [offsetNorth] sets them off the route; and [fail] puts an error on the fixes being
/// listened to, as the location service does when it's turned off.
class SimulatedTripLocation implements NavigationLocationSource {
  SimulatedTripLocation(this.route);

  final NavigableRoute Function() route;

  /// Whether fixes are held back: the simulation goes on, but none of its fixes come
  /// while this is set.
  bool paused = false;

  /// How far north of where the simulation is each fix is put, in meters.
  double offsetNorth = 0;

  /// The fixes being listened to, if any.
  StreamController<NavLocation>? _listened;

  /// Puts [error] on the fixes being listened to, if any.
  void fail(Object error) => _listened?.addError(error);

  @override
  Stream<NavLocation> fixes(NavigationMode mode) {
    StreamSubscription<NavLocation>? simulated;
    late final StreamController<NavLocation> out;
    out = StreamController<NavLocation>(
      onListen: () {
        _listened = out;
        simulated = SimulatedLocationSource(route())
            .fixes(mode)
            .listen(
              (fix) {
                if (!paused) out.add(_offset(fix));
              },
              onError: out.addError,
              onDone: out.close,
            );
      },
      onCancel: () {
        if (_listened == out) _listened = null;
        return simulated?.cancel();
      },
    );
    return out.stream;
  }

  NavLocation _offset(NavLocation fix) {
    if (offsetNorth == 0) return fix;
    // A degree of latitude is 111,320 m, near enough anywhere.
    return NavLocation(
      lat: fix.lat + offsetNorth / 111320,
      lng: fix.lng,
      horizontalAccuracy: fix.horizontalAccuracy,
      course: fix.course,
      courseAccuracy: fix.courseAccuracy,
      speed: fix.speed,
      speedAccuracy: fix.speedAccuracy,
      timestampMs: fix.timestampMs,
    );
  }
}
