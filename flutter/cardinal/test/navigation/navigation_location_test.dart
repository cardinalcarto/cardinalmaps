import 'package:cardinal/navigation/navigation_location.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart';

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
}
