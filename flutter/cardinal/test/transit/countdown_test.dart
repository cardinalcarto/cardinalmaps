import 'package:cardinal/directions/transit_plan.dart';
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

  group('state', () {
    CountdownState stateAt(int now, {DepartureStatus? status}) => Countdown.at(
      departure,
      now: at(now),
      walkSeconds: 240,
      status: status ?? DepartureStatus.scheduled,
    ).state;

    test('upcoming while the walk can make it, live or scheduled', () {
      expect(stateAt(0), CountdownState.upcoming);
      expect(
        stateAt(60, status: DepartureStatus.live),
        CountdownState.upcoming,
      );
    });

    test('unreachable once the walk can\'t make it, until it leaves', () {
      expect(stateAt(61), CountdownState.unreachable);
      expect(stateAt(300), CountdownState.unreachable);
    });

    test('departed once it has left', () {
      expect(stateAt(301), CountdownState.departed);
    });

    test(
      "canceled or skipped until it would have left, however far the walk",
      () {
        expect(
          stateAt(0, status: DepartureStatus.canceled),
          CountdownState.canceled,
        );
        expect(
          stateAt(200, status: DepartureStatus.skipped),
          CountdownState.skipped,
        );
        expect(
          stateAt(301, status: DepartureStatus.canceled),
          CountdownState.departed,
        );
      },
    );

    test("reads a departure's own status", () {
      final canceled = NearbyTransit.fromJson(snapshotNearbyJson())
          .routes[2]
          .directions[1]
          .departures
          .first;
      expect(
        Countdown(canceled, now: canceled.departure).state,
        CountdownState.canceled,
      );
    });
  });
}
