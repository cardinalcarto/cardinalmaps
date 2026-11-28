/// The phone's end of CarPlay: tells the car's screen about the trip, and does what
/// the car's buttons ask.
library;

import 'dart:async';

import 'package:collection/collection.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../l10n/lookup.dart';
import '../map/map.dart' show MapTiles;
import '../map/viewport.dart';
import '../navigation/navigation_controller.dart';
import '../store/store.dart';
import '../units.dart';
import 'car_keys.dart';
import 'car_trip.dart';

/// Where the car's map opens before there's a trip to follow: no closer than this, so
/// it shows the town around where the phone's map was looking.
const carOpeningMaxZoom = 14.0;

/// What carries messages to and from the app's CarPlay code: the platform's, or a fake
/// in tests.
abstract class CarChannel {
  /// Sends [method] with [arguments], for its answer.
  Future<T?> invoke<T>(String method, [Object? arguments]);

  /// Called with each message from the car's code.
  set onCall(void Function(String method, Object? arguments)? onCall);
}

/// [CarChannel] on `cardinal/carplay`, which `CarPlayBridge.swift` serves.
class PlatformCarChannel implements CarChannel {
  static const _channel = MethodChannel('cardinal/carplay');

  @override
  Future<T?> invoke<T>(String method, [Object? arguments]) =>
      _channel.invokeMethod<T>(method, arguments);

  @override
  set onCall(void Function(String method, Object? arguments)? onCall) =>
      _channel.setMethodCallHandler(
        onCall == null
            ? null
            : (call) async => onCall(call.method, call.arguments),
      );
}

/// Keeps the car's screen in step with [controller]'s trip, through the app's CarPlay
/// code ([CarChannel]).
///
/// The car's screen is drawn by CarPlay's templates and, under them, by a map in a
/// Flutter engine of its own, which shares nothing with this one: what either shows is
/// what's sent from here. While a car is connected, it's sent:
///
/// - `map`, once: where the map's tiles come from, as [tiles] says, and where it opens
///   ([view], no closer than [carOpeningMaxZoom], north up);
/// - `route`, when the trip's route changes: its line, under a number ([carRouteOf]);
/// - `trip`, when what [carTripOf] says of the trip changes, which is at each fix, and
///   null when the trip ends.
///
/// Nothing is sent with no car connected, so the phone alone does no more work than
/// before. The car's code says when one connects (`connected`), which sends all three
/// again, and when it's gone (`disconnected`).
///
/// The car's End ends the trip ([NavigationController.end]) and its mute button sets
/// [NavigationController.setMuted], as the phone's do, so both screens show the same.
class CarPlayLink {
  CarPlayLink({
    required this.controller,
    required this.settings,
    required this.tiles,
    required this.view,
    CarChannel? channel,
    AppLocalizations Function()? words,
  }) : _channel = channel ?? PlatformCarChannel(),
       _words = words ?? deviceLocalizations;

  final NavigationController controller;
  final SettingsStore settings;

  /// Where the map's tiles come from.
  final Future<MapTiles> Function() tiles;

  /// Where the phone's map is looking, or was when the app last closed.
  final Future<MapViewport> Function() view;

  final CarChannel _channel;
  final AppLocalizations Function() _words;

  UnitSystem _units = UnitSystem.locale;
  StreamSubscription<UnitSystem>? _watchUnits;
  bool _connected = false;

  /// The route whose line was last sent, and the number it was sent under.
  DirectionsRoute? _route;
  int _routeId = 0;

  /// What was last sent of the trip; null when that there's none.
  Map<String, Object?>? _sent;

  static const _same = DeepCollectionEquality();

  /// Starts following the controller, and listening to the car.
  void start() {
    _channel.onCall = _called;
    controller.addListener(_changed);
    _watchUnits = settings.watch(unitSystem).listen((units) {
      if (units == _units) return;
      _units = units;
      _changed();
    });
    // A car may have connected before this engine started.
    unawaited(() async {
      if (await _quietly(() => _channel.invoke<bool>('hello')) ?? false) {
        _connect();
      }
    }());
  }

  void dispose() {
    _channel.onCall = null;
    controller.removeListener(_changed);
    unawaited(_watchUnits?.cancel());
  }

  void _called(String method, Object? arguments) {
    switch (method) {
      case 'connected':
        _connect();
      case 'disconnected':
        _connected = false;
      case 'end':
        unawaited(controller.end());
      case 'setMuted':
        if (arguments case final bool muted) controller.setMuted(muted);
    }
  }

  /// Sends everything again: the car's screen starts knowing nothing.
  void _connect() {
    _connected = true;
    _route = null;
    _sent = null;
    unawaited(_sendMap());
    _changed(force: true);
  }

  Future<void> _sendMap() async {
    try {
      final (tiles, view) = await (this.tiles(), this.view()).wait;
      if (!_connected) return;
      await _channel.invoke<void>('map', {
        CarKeys.tileJson: tiles.tileJson,
        CarKeys.basemap: tiles.basemap,
        CarKeys.view: MapViewport(
          lng: view.lng,
          lat: view.lat,
          zoom: view.zoom,
        ).zoomedOutTo(carOpeningMaxZoom).arg,
      });
    } on Object catch (e) {
      // The trip is still shown in the car's templates, over no map.
      debugPrint('carplay: map: $e');
    }
  }

  void _changed({bool force = false}) {
    if (!_connected) return;
    final route = controller.route;
    if (route == null) {
      // The car's map lets go of the line as the trip ends: the next trip sends its
      // own, even along the same route.
      _route = null;
    } else if (!identical(route, _route)) {
      _route = route;
      _routeId++;
      unawaited(
        _quietly(
          () => _channel.invoke<void>(
            'route',
            carRouteOf(_routeId, route.geometry),
          ),
        ),
      );
    }
    final trip = carTripOf(
      controller,
      words: _words(),
      units: _units.onDevice,
      route: _routeId,
    );
    if (!force && _same.equals(trip, _sent)) return;
    _sent = trip;
    unawaited(_quietly(() => _channel.invoke<void>('trip', trip)));
  }

  static Future<T?> _quietly<T>(Future<T?> Function() call) async {
    try {
      return await call();
    } on Object catch (e) {
      // Navigation goes on without the car's screen.
      debugPrint('carplay: $e');
      return null;
    }
  }
}
