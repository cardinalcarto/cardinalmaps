/// One page of a nearby route in detail, as answers arrive: its departures, the one
/// selected, and the selected trip's ride.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;
import 'package:routing/routing.dart' show DirectionsPoint;

import '../directions/transit_plan.dart';
import 'countdown.dart';
import 'nearby_source.dart';
import 'nearby_transit.dart';

/// One run of a trip: a `trip_id` on its service day.
typedef TripRun = ({String tripId, String? startDate});

/// [departure]'s run.
TripRun runOf(StopDeparture departure) =>
    (tripId: departure.tripId, startDate: departure.startDate);

/// [ride]'s run, if the answer names its trip.
TripRun? rideRun(TransitLeg ride) => switch (ride.tripId) {
  final id? => (tripId: id, startDate: ride.startDate),
  null => null,
};

/// Whether [departure], from a stop [walkSeconds] away, still shows at [now]: it hasn't
/// left.
bool departureShows(
  StopDeparture departure,
  DateTime now, {
  required int walkSeconds,
}) => !Countdown(departure, now: now, walkSeconds: walkSeconds).departed;

/// The departure of [departures] a detail selects at [now], from a stop [walkSeconds]
/// away: the first a rider setting off now can catch; or else the first that runs and
/// hasn't left; or null.
StopDeparture? firstCatchable(
  List<StopDeparture> departures,
  DateTime now, {
  required int walkSeconds,
}) {
  StopDeparture? running;
  for (final d in departures) {
    final state = Countdown(d, now: now, walkSeconds: walkSeconds).state;
    if (state == CountdownState.upcoming) return d;
    if (state == CountdownState.unreachable) running ??= d;
  }
  return running;
}

/// Asks [source] for one page of a route in detail, [query]'s, and keeps what the page
/// shows: the departures, the one selected, and its trip's ride. The screen decides
/// when to [refresh].
///
/// States:
///
/// - **Seeded**: before any answer, the departures are [seed] (the row's), and the
///   first catchable of them is selected. There's no [ride].
/// - **Answered**: the departures are the last answer's, a departure is selected (or
///   none, when nothing that runs is left), and [ride] is the latest ride of the
///   selected trip the router gave, or the one before it while the selected trip's is
///   being asked for.
///
/// Transitions:
///
/// 1. An answer arrives: its departures replace the old. If the selected trip isn't among
///    them or no longer runs or has left, the first catchable is selected instead. If the
///    answer's ride is the selected trip's, it's the [ride]; otherwise the old [ride]
///    stays, and the selected trip is asked for.
/// 2. [select] a departure that runs: it's selected at once, and its trip is asked for;
///    [ride] stays until that answer arrives (1).
/// 3. [keepSelection] at a moment the selected departure has left: the first catchable
///    is selected, and asked for as in 2.
/// 4. Asking for the selected trip fails (the router doesn't know it, or any error): the
///    first catchable is selected at once instead, and asked for as in 2, unless it's the
///    trip that failed.
///
/// An answer uses the data at hand while real-time data is still being fetched
/// ([NearbyTransitSource.realtimeArrived]); once new data arrives, the page is
/// asked for again, as nearby rows are.
class RouteDetailController extends ChangeNotifier {
  RouteDetailController(
    this.source,
    this.query, {
    this.seed = const [],
    this.clock = DateTime.now,
  }) : _selected = switch (firstCatchable(
         seed,
         clock(),
         walkSeconds: query.walkSeconds,
       )) {
         final d? => runOf(d),
         null => null,
       };

  final NearbyTransitSource source;

  /// The page, without a trip.
  final RouteDetailQuery query;

  /// The departures to show before the first answer: the row's.
  final List<StopDeparture> seed;

  /// What time it is: the device's clock, or a test's.
  final DateTime Function() clock;

  /// The last answer, or null before the first.
  RouteDetail? get detail => _detail;
  RouteDetail? _detail;

  /// Whether the last question failed, and nothing has been answered since.
  bool get failed => _failed;
  bool _failed = false;

  /// The departures shown: the last answer's, or [seed] before the first.
  List<StopDeparture> get departures => _detail?.departures ?? seed;

  /// The selected departure's run.
  TripRun? get selected => _selected;
  TripRun? _selected;

  /// The selected trip's ride from the stop, or the previously selected one's while it's
  /// being asked for; null before the first.
  TransitLeg? get ride => _ride;
  TransitLeg? _ride;

  /// [ride]'s trip before the stop, from the answer [ride] came with
  /// ([RouteDetail.shapeToStop]).
  List<DirectionsPoint> get shapeToStop => _shapeToStop;
  List<DirectionsPoint> _shapeToStop = const [];

  /// How the rider gets to the stop, from the first [refresh].
  TransitAccess? _access;

  /// Completes to abandon the question being asked, if any.
  Completer<void>? _asking;

  /// Whether a question is being asked.
  bool get asking => _asking != null;

  /// Counts questions and abandonments: live times are asked for only if nothing has been
  /// asked since the answer that waits for them.
  int _asks = 0;
  bool _disposed = false;

  /// Asks again, getting to the stop by [access], for the selected trip. A question
  /// already being asked is abandoned. Throws as [NearbyTransitSource.route] does,
  /// leaving what's shown as it was.
  Future<void> refresh(TransitAccess access) async {
    _access = access;
    final asked = await _ask();
    if (asked != null) unawaited(_whenLive(asked));
  }

  /// Selects [departure], and asks for its trip, unless it's already selected or doesn't
  /// run.
  void select(StopDeparture departure) {
    final run = runOf(departure);
    if (run == _selected || !departure.status.runs) return;
    _selected = run;
    notifyListeners();
    _askForSelected();
  }

  /// Selects the first catchable departure if the selected one has left at [now].
  void keepSelection(DateTime now) {
    if (_shows(_selected, departures, now)) return;
    final next = _firstAt(departures, now);
    if (next == _selected) return;
    _selected = next;
    notifyListeners();
    _askForSelected();
  }

  /// Abandons any question, and the live times it waits for, as the page stops being
  /// shown. What it shows stays, for when it shows again.
  void abandon() {
    _asking?.complete();
    _asking = null;
    _asks++;
  }

  @override
  void dispose() {
    _disposed = true;
    abandon();
    super.dispose();
  }

  TripRun? _firstAt(List<StopDeparture> departures, DateTime now) =>
      switch (firstCatchable(departures, now, walkSeconds: query.walkSeconds)) {
        final d? => runOf(d),
        null => null,
      };

  /// Whether [run] is among [departures], runs, and hasn't left at [now].
  bool _shows(TripRun? run, List<StopDeparture> departures, DateTime now) {
    if (run == null) return false;
    for (final d in departures) {
      if (runOf(d) != run) continue;
      return d.status.runs &&
          departureShows(d, now, walkSeconds: query.walkSeconds);
    }
    return false;
  }

  /// Asks for the selected trip, when there is one and its ride isn't shown.
  void _askForSelected() {
    if (_access == null || _selected == null) return;
    if (_ride case final ride? when rideRun(ride) == _selected) return;
    unawaited(
      _ask().then(
        (asked) {
          if (asked != null) unawaited(_whenLive(asked));
        },
        onError: (Object e) {
          // What's shown stays until the next refresh.
          debugPrint('route detail: $e');
        },
      ),
    );
  }

  /// Asks for the selected trip and applies the answer, returning which question it was,
  /// or null if it was abandoned.
  Future<int?> _ask() async {
    _asking?.complete();
    final asking = _asking = Completer<void>();
    final ask = ++_asks;
    final selected = _selected;
    try {
      final answer = await source.route(
        query: RouteDetailQuery(
          feed: query.feed,
          routeId: query.routeId,
          headsigns: query.headsigns,
          stopId: query.stopId,
          walkSeconds: query.walkSeconds,
          tripId: selected?.tripId,
          startDate: selected?.startDate,
        ),
        access: _access!,
        abort: asking.future,
      );
      if (asking.isCompleted) return null;
      _asking = null;
      _apply(answer);
      notifyListeners();
      _askForSelected();
      return ask;
    } on http.RequestAbortedException {
      if (!asking.isCompleted) rethrow;
      return null;
    } on Object {
      if (!asking.isCompleted) {
        _asking = null;
        _failed = true;
        _fallBack(selected);
        notifyListeners();
      }
      rethrow;
    } finally {
      if (identical(_asking, asking)) _asking = null;
    }
  }

  /// After the trip of [failed] couldn't be loaded, selects the first catchable departure
  /// instead and asks for its trip, unless that's the one that failed or another has been
  /// selected since.
  void _fallBack(TripRun? failed) {
    if (failed == null || _selected != failed) return;
    final first = _firstAt(departures, clock());
    if (first == null || first == failed) return;
    _selected = first;
    _askForSelected();
  }

  void _apply(RouteDetail answer) {
    _detail = answer;
    _failed = false;
    final now = clock();
    if (!_shows(_selected, answer.departures, now)) {
      _selected = _firstAt(answer.departures, now);
    }
    final ride = answer.ride;
    if (ride == null) {
      // Nothing runs in the horizon.
      if (answer.departures.isEmpty) {
        _ride = null;
        _shapeToStop = const [];
      }
      return;
    }
    if (_selected == null || rideRun(ride) == _selected) {
      _ride = ride;
      _shapeToStop = answer.shapeToStop;
    }
  }

  /// Asks again once real-time data the answer to question [asked] went without arrives,
  /// if nothing has been asked since.
  Future<void> _whenLive(int asked) async {
    if (_disposed || asked != _asks) return;
    if (!await source.realtimeArrived()) return;
    if (_disposed || asked != _asks) return;
    try {
      final next = await _ask();
      if (next != null) unawaited(_whenLive(next));
    } on Object catch (e) {
      // What's shown stays on the schedule until the next refresh.
      debugPrint('route detail: live times: $e');
    }
  }
}

/// [NearbyDeparture]'s [ContinuesAs], for any departure that has one.
ContinuesAs? continuesAsOf(StopDeparture departure) =>
    departure is NearbyDeparture ? departure.continuesAs : null;
