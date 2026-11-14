import 'package:cardinal/transit/countdown.dart';
import 'package:cardinal/transit/nearby_transit.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_nearby.dart';
import '../fakes/fake_transit.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

void main() {
  final departure = at(300);

  test('counts whole minutes down to a departure, rounding down', () {
    expect(Countdown.at(departure, now: at(0)).minutesUntil, 5);
    expect(Countdown.at(departure, now: at(1)).minutesUntil, 4);
    expect(Countdown.at(departure, now: at(241)).minutesUntil, 0);
    expect(Countdown.at(departure, now: at(300)).minutesUntil, 0);
    expect(Countdown.at(departure, now: at(300)).departed, isFalse);
  });

  test('counts below zero once it has left', () {
    final gone = Countdown.at(
      departure,
      now: at(300).add(const Duration(milliseconds: 1)),
    );
    expect(gone.secondsUntil, -1);
    expect(gone.minutesUntil, -1);
    expect(gone.departed, isTrue);
  });

  test("isn't catchable once the walk is longer than the wait", () {
    expect(
      Countdown.at(departure, now: at(60), walkSeconds: 240).catchable,
      isTrue,
    );
    final late = Countdown.at(departure, now: at(61), walkSeconds: 240);
    expect(late.secondsToSpare, -1);
    expect(late.catchable, isFalse);
    expect(late.departed, isFalse);
  });

  test("reads a nearby departure's own time", () {
    final direction = NearbyTransit.fromJson(snapshotNearbyJson())
        .routes
        .first
        .directions
        .single;
    final countdown = Countdown(
      direction.departures.single,
      now: DateTime.fromMillisecondsSinceEpoch(1781535015 * 1000),
      walkSeconds: direction.walkSeconds,
    );
    expect(countdown.secondsUntil, 675);
    expect(countdown.minutesUntil, 11);
    expect(countdown.secondsToSpare, 633);
  });
}
