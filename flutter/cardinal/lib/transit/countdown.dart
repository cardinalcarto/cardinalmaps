/// How long until a departure, at a given moment, so a list can count down locally
/// between answers without asking the router again.
library;

import 'package:flutter/foundation.dart';

import '../directions/transit_plan.dart';

/// A departure seen at one moment: how long until it leaves, and whether the rider can
/// still walk to it. What to say for each (such as "3 min" or "Now") is the screen's.
@immutable
class Countdown {
  /// [departure] at [now], from a stop [walkSeconds] away.
  Countdown(
    StopDeparture departure, {
    required DateTime now,
    int walkSeconds = 0,
  }) : this.at(departure.departure, now: now, walkSeconds: walkSeconds);

  /// A departure at [departure], at [now], from a stop [walkSeconds] away.
  Countdown.at(
    DateTime departure, {
    required DateTime now,
    this.walkSeconds = 0,
  }) : secondsUntil = (departure.difference(now).inMilliseconds / 1000).floor();

  /// Whole seconds until it leaves; below zero once it has.
  final int secondsUntil;

  /// The walk to its stop.
  final int walkSeconds;

  /// Whole minutes until it leaves, rounded down: 0 in its last minute, and below zero
  /// once it has left.
  int get minutesUntil => (secondsUntil / 60).floor();

  /// Whether it has left.
  bool get departed => secondsUntil < 0;

  /// Seconds left after walking to the stop; below zero when it leaves before the rider
  /// can be there.
  int get secondsToSpare => secondsUntil - walkSeconds;

  /// Whether a rider setting off now can be at the stop for it, as the router counts a
  /// first boarding: it leaves at or after now plus the walk.
  bool get catchable => secondsToSpare >= 0;
}
