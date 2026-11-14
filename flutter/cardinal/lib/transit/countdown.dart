/// How long until a departure, at a given moment, so a list can count down locally
/// between answers without asking the router again.
library;

import 'package:flutter/foundation.dart';

import '../directions/transit_plan.dart';

/// What a departure is at one moment, for the screen to give each its style.
enum CountdownState {
  /// It runs, and a rider setting off now can be at the stop for it: the countdown.
  upcoming,

  /// It runs and hasn't left, but the walk to the stop can't make it any more: dimmed.
  unreachable,

  /// It has left (at its scheduled time, if it doesn't run): hidden.
  departed,

  /// It doesn't run: struck through, with a label, and no countdown.
  canceled,

  /// It runs, but no longer lets riders on at this stop: as [canceled].
  skipped,
}

/// A departure seen at one moment: how long until it leaves, and what that makes it.
/// What to say and how to draw each (such as "3 min", or "Now" below a minute) is the
/// screen's.
@immutable
class Countdown {
  /// [departure] at [now], from a stop [walkSeconds] away.
  Countdown(
    StopDeparture departure, {
    required DateTime now,
    int walkSeconds = 0,
  }) : this.at(
         departure.departure,
         now: now,
         walkSeconds: walkSeconds,
         status: departure.status,
       );

  /// A departure at [departure] with [status], at [now], from a stop [walkSeconds]
  /// away.
  Countdown.at(
    this.departure, {
    required DateTime now,
    this.walkSeconds = 0,
    this.status = DepartureStatus.scheduled,
  }) : secondsUntil = (departure.difference(now).inMilliseconds / 1000).floor();

  /// When it leaves.
  final DateTime departure;

  /// Whole seconds until it leaves; below zero once it has.
  final int secondsUntil;

  /// The walk to its stop.
  final int walkSeconds;

  /// Whether it runs, and whether real-time data has seen it.
  final DepartureStatus status;

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

  /// What it is now. Once it has left it's [CountdownState.departed], whether or not it
  /// runs; until then one that doesn't run is [CountdownState.canceled] or
  /// [CountdownState.skipped], however far the walk.
  CountdownState get state => departed
      ? CountdownState.departed
      : switch (status) {
          DepartureStatus.canceled => CountdownState.canceled,
          DepartureStatus.skipped => CountdownState.skipped,
          DepartureStatus.scheduled || DepartureStatus.live =>
            catchable ? CountdownState.upcoming : CountdownState.unreachable,
        };
}
