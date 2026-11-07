import 'dart:async';
import 'dart:math' as math;

import 'package:cardinal/map/layers.dart';
import 'package:cardinal/map/location.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart';

import '../fakes/fake_map.dart';

/// A location service that answers as told, and counts what it's asked.
class FakeLocation implements LocationSource {
  FakeLocation({this.enabled = true, this.permission = LocationPermission.whileInUse, this.granted, this.now});

  bool enabled;
  LocationPermission permission;

  /// What asking for permission gives; [permission] stays if null.
  final LocationPermission? granted;
  final Fix? now;
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

const seattle = Fix(lng: -122.3321, lat: 47.6062, accuracy: 25);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('the dot', () {
    test('is nothing without a position', () {
      expect(locationGeoJson(null)['features'], isEmpty);
    });

    test('has an accuracy circle as wide as the accuracy', () {
      final features = locationGeoJson(seattle)['features']! as List;
      expect(features, hasLength(2));
      final ring = ((features[0] as Map)['geometry'] as Map)['coordinates'][0] as List;
      expect(ring.first, ring.last, reason: 'a closed ring');
      // Every vertex is the accuracy away, within a few percent.
      for (final vertex in ring.cast<List<double>>()) {
        final dLat = (vertex[1] - seattle.lat) * math.pi / 180 * 6371008.8;
        final dLng = (vertex[0] - seattle.lng) * math.pi / 180 * 6371008.8 * math.cos(seattle.lat * math.pi / 180);
        expect(math.sqrt(dLat * dLat + dLng * dLng), closeTo(25, 0.5));
      }
      expect(((features[1] as Map)['geometry'] as Map)['coordinates'], [seattle.lng, seattle.lat]);
    });

    test('has no circle when the accuracy is unknown', () {
      final features = locationGeoJson(const Fix(lng: 0, lat: 0, accuracy: 0))['features']! as List;
      expect(features, hasLength(1));
    });

    test('is drawn under the selected place, in both styles', () async {
      for (final s in MapStyle.values) {
        final style = await styleFor(MapLayers(style: s));
        final ids = [for (final layer in style['layers']! as List) (layer as Map)['id']];
        expect(ids.sublist(ids.length - 3), ['location-accuracy', 'location-dot', 'selection-pin'], reason: '$s');
        expect((style['sources']! as Map).containsKey(locationSource), isTrue);
      }
    });
  });

  group('locating', () {
    test('moves the map to you and shows the dot', () async {
      final map = FakeMap()..subject = true;
      final tracker = LocationTracker(map, source: FakeLocation(now: seattle));
      expect(await tracker.locate(), LocateResult.located);
      expect(map.flownTo, (lng: seattle.lng, lat: seattle.lat, zoom: locationZoom));
      expect(map.geoJson[locationSource], locationGeoJson(seattle));
      expect(map.subject, isFalse, reason: 'the map follows you now, not a place');
    });

    test('asks for permission the first time', () async {
      final source = FakeLocation(permission: LocationPermission.denied, granted: LocationPermission.whileInUse, now: seattle);
      final tracker = LocationTracker(FakeMap(), source: source);
      expect(await tracker.locate(), LocateResult.located);
      expect(source.asked, 1);
    });

    test('says why it could not', () async {
      Future<LocateResult> locate(FakeLocation source) => LocationTracker(FakeMap(), source: source).locate();
      expect(await locate(FakeLocation(enabled: false, now: seattle)), LocateResult.serviceOff);
      expect(
        await locate(FakeLocation(permission: LocationPermission.denied, granted: LocationPermission.denied)),
        LocateResult.denied,
      );
      expect(await locate(FakeLocation(permission: LocationPermission.deniedForever)), LocateResult.deniedForever);
      expect(await locate(FakeLocation()), LocateResult.unavailable);
    });

    test('a refusal leaves the map where it was', () async {
      final map = FakeMap();
      await LocationTracker(map, source: FakeLocation(permission: LocationPermission.deniedForever)).locate();
      expect(map.flownTo, isNull);
      expect(map.geoJson, isEmpty);
    });
  });

  test("a platform without a location service shows nothing, and says so", () async {
    final broken = _Broken();
    final map = FakeMap();
    await LocationTracker(map, source: broken).startIfAllowed();
    expect(await LocationTracker(map, source: broken).locate(), LocateResult.unavailable);
    expect(map.geoJson, isEmpty);
  });

  group('following', () {
    test('starts at once if you allowed it before, without asking', () async {
      final map = FakeMap();
      final source = FakeLocation();
      await LocationTracker(map, source: source).startIfAllowed();
      expect(source.asked, 0);
      source.positionsController.add(seattle);
      expect(map.geoJson[locationSource], locationGeoJson(seattle));
      expect(map.flownTo, isNull, reason: 'only the button moves the map');
    });

    test("doesn't start or ask if you haven't allowed it", () async {
      final source = FakeLocation(permission: LocationPermission.denied);
      await LocationTracker(FakeMap(), source: source).startIfAllowed();
      expect(source.asked, 0);
      expect(source.listening, 0);
    });
  });
}

/// A location service that isn't there, as on a platform without one.
class _Broken extends FakeLocation {
  @override
  Future<bool> serviceEnabled() => Future.error(UnsupportedError('no location service'));

  @override
  Future<LocationPermission> checkPermission() => Future.error(UnsupportedError('no location service'));
}
