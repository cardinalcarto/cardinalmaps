import 'package:cardinal/navigation/navigation_location.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart';
import 'package:routing/routing.dart';

Position position({
  double accuracy = 4,
  double heading = 271.5,
  double headingAccuracy = 8,
  double speed = 12,
  double speedAccuracy = 0.5,
  bool measured = true,
}) => Position(
  longitude: 1.52,
  latitude: 42.5,
  timestamp: DateTime.fromMillisecondsSinceEpoch(1763000001234),
  accuracy: accuracy,
  altitude: 0,
  altitudeAccuracy: 0,
  heading: heading,
  headingAccuracy: headingAccuracy,
  speed: speed,
  speedAccuracy: speedAccuracy,
  hasAccuracy: measured,
  hasHeading: measured,
  hasHeadingAccuracy: measured,
  hasSpeed: measured,
  hasSpeedAccuracy: measured,
);

void main() {
  test('a fix keeps its course, speed, their accuracies, and its time', () {
    final fix = navLocation(position());
    expect(fix.lat, 42.5);
    expect(fix.lng, 1.52);
    expect(fix.horizontalAccuracy, 4);
    expect(fix.course, 271.5);
    expect(fix.courseAccuracy, 8);
    expect(fix.speed, 12);
    expect(fix.speedAccuracy, 0.5);
    expect(fix.timestampMs, 1763000001234);
  });

  test("what wasn't measured is left out, not taken as 0", () {
    final fix = navLocation(position(heading: 0, speed: 0, measured: false));
    expect(fix.horizontalAccuracy, double.infinity);
    expect(fix.course, isNull);
    expect(fix.courseAccuracy, isNull);
    expect(fix.speed, isNull);
    expect(fix.speedAccuracy, isNull);
  });

  test("Core Location's -1 for no course or speed is left out", () {
    final fix = navLocation(
      position(heading: -1, headingAccuracy: -1, speed: -1, speedAccuracy: -1),
    );
    expect(fix.course, isNull);
    expect(fix.courseAccuracy, isNull);
    expect(fix.speed, isNull);
    expect(fix.speedAccuracy, isNull);
  });

  group('on Android and iOS', () {
    TestWidgetsFlutterBinding.ensureInitialized();
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    // geolocator's position channels, which the map's location dot listens on: its
    // Android plugin's, and the default one tests run with.
    const geolocatorUpdates = [
      EventChannel('flutter.baseflow.com/geolocator_updates_android'),
      EventChannel('flutter.baseflow.com/geolocator_updates'),
    ];

    tearDown(() {
      debugDefaultTargetPlatformOverride = null;
      messenger.setMockStreamHandler(PlatformNavigationSource.channel, null);
      for (final channel in geolocatorUpdates) {
        messenger.setMockStreamHandler(channel, null);
      }
    });

    test("navigation's fixes are its own, not geolocator's", () {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
      expect(deviceNavigationSource(), isA<PlatformNavigationSource>());
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      expect(deviceNavigationSource(), isA<PlatformNavigationSource>());
      debugDefaultTargetPlatformOverride = TargetPlatform.linux;
      expect(deviceNavigationSource(), isA<GeolocatorNavigationSource>());
    });

    test(
      "listens on its own channel, once a second, with the dot's stream open",
      () async {
        final geolocatorListens = <Object?>[];
        for (final channel in geolocatorUpdates) {
          messenger.setMockStreamHandler(
            channel,
            MockStreamHandler.inline(
              onListen: (arguments, events) => geolocatorListens.add(arguments),
            ),
          );
        }
        final navigationListens = <Object?>[];
        MockStreamHandlerEventSink? sink;
        messenger.setMockStreamHandler(
          PlatformNavigationSource.channel,
          MockStreamHandler.inline(
            onListen: (arguments, events) {
              navigationListens.add(arguments);
              sink = events;
            },
          ),
        );
        debugDefaultTargetPlatformOverride = TargetPlatform.android;
        // The dot, as `GeolocatorSource` asks.
        final dot = Geolocator.getPositionStream(
          locationSettings: AndroidSettings(
            intervalDuration: const Duration(seconds: 5),
            distanceFilter: 2,
          ),
        ).listen((_) {});
        await pumpEventQueue();
        expect(geolocatorListens, hasLength(1));

        final fixes = <NavLocation>[];
        final navigation = const PlatformNavigationSource()
            .fixes(NavigationMode.driving)
            .listen(fixes.add);
        await pumpEventQueue();
        expect(navigationListens, [
          {'intervalMs': 1000, 'mode': 'driving'},
        ]);
        // Nothing more asked of geolocator.
        expect(geolocatorListens, hasLength(1));

        sink!.success({
          'latitude': 42.5,
          'longitude': 1.52,
          'timestamp': 1763000001234,
          'accuracy': 4.0,
        });
        await pumpEventQueue();
        expect(fixes.single.lat, 42.5);
        expect(fixes.single.horizontalAccuracy, 4);
        await navigation.cancel();
        await dot.cancel();
      },
    );

    test('errors say why fixes stopped', () async {
      MockStreamHandlerEventSink? sink;
      messenger.setMockStreamHandler(
        PlatformNavigationSource.channel,
        MockStreamHandler.inline(
          onListen: (arguments, events) => sink = events,
        ),
      );
      final problems = <LocationProblem>[];
      final navigation = const PlatformNavigationSource()
          .fixes(NavigationMode.walking)
          .listen(
            (_) {},
            onError: (Object e) =>
                problems.add((e as NavigationLocationError).problem),
          );
      await pumpEventQueue();
      sink!.error(code: 'off');
      sink!.error(code: 'denied');
      sink!.error(code: 'failed');
      await pumpEventQueue();
      expect(problems, [
        LocationProblem.off,
        LocationProblem.denied,
        LocationProblem.failed,
      ]);
      await navigation.cancel();
    });
  });

  group('a fix from Android', () {
    test('keeps what was measured', () {
      final fix = navLocationFromMap({
        'latitude': 42.5,
        'longitude': 1.52,
        'timestamp': 1763000001234,
        'accuracy': 4.0,
        'heading': 271.5,
        'heading_accuracy': 8.0,
        'speed': 12.0,
        'speed_accuracy': 0.5,
      });
      expect(fix.lat, 42.5);
      expect(fix.lng, 1.52);
      expect(fix.horizontalAccuracy, 4);
      expect(fix.course, 271.5);
      expect(fix.courseAccuracy, 8);
      expect(fix.speed, 12);
      expect(fix.speedAccuracy, 0.5);
      expect(fix.timestampMs, 1763000001234);
    });

    test("leaves out what wasn't measured", () {
      final fix = navLocationFromMap({
        'latitude': 42.5,
        'longitude': 1.52,
        'timestamp': 1763000001234,
      });
      expect(fix.horizontalAccuracy, double.infinity);
      expect(fix.course, isNull);
      expect(fix.courseAccuracy, isNull);
      expect(fix.speed, isNull);
      expect(fix.speedAccuracy, isNull);
    });
  });
}
