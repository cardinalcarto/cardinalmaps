import 'dart:async';

import 'package:cardinal/map/location.dart';
import 'package:geolocator/geolocator.dart';

/// A location service that answers as told, and counts what it's asked.
class FakeLocation implements LocationSource {
  FakeLocation({
    this.enabled = true,
    this.permission = LocationPermission.whileInUse,
    this.granted,
    this.now,
  });

  bool enabled;
  LocationPermission permission;

  /// What asking for permission gives; [permission] stays if null.
  final LocationPermission? granted;
  Fix? now;
  int asked = 0;
  int listening = 0;
  final positionsController = StreamController<Fix>.broadcast(sync: true);

  @override
  Future<bool> serviceEnabled() async => enabled;

  @override
  Future<LocationPermission> checkPermission() async => permission;

  @override
  Future<LocationPermission> requestPermission() async {
    asked++;
    return permission = granted ?? permission;
  }

  @override
  Stream<Fix> positions() {
    listening++;
    return positionsController.stream;
  }

  @override
  Future<Fix?> current() async => now;

  @override
  Future<void> openAppSettings() async {}

  @override
  Future<void> openLocationSettings() async {}
}
