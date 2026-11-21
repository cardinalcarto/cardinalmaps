/// Which departure of each ride in a journey the rider means to take, and what that
/// makes of the rest of the journey: a ride's connections are the departures listed
/// with it, and taking a later one of a ride can mean a later one of the next.
///
/// It starts with each ride's suggested departure, the plan's own. Choosing another
/// one of a ride sets when the rider can be at the next ride's boarding stop (its
/// arrival plus the next ride's [TransitLeg.transferSeconds]), and so which of the
/// next ride's departures they can make. Down the journey, each ride after keeps what
/// was chosen for it while that can still be made. Otherwise, while every ride before
/// it takes its suggested departure, it takes its own suggested one; once a ride before
/// takes another, it takes the departure that can be made and reaches its alighting
/// stop first (the suggested one, then the earliest to leave, on a tie). So a later
/// ride doesn't wait on a train that the plan chose for a bus the rider now gets
/// earlier. A ride none of whose departures can be made has no connection in its list,
/// and what comes after it isn't known.
///
/// The first ride's departures are made as the clock has it, when the chain is given
/// the time ([ConnectionChain.now]): one can be made if it leaves once the rider,
/// setting out now, can be at its stop after the streets to it, and the plan doesn't
/// have it missed. As time passes they're missed one by one, and the rest of the
/// journey follows as it does a choice.
///
/// | A ride's choice | When | Then |
/// |---|---|---|
/// | the suggested one | the plan is read | each ride's suggested departure |
/// | any | the rider chooses one they can make | that one, chosen |
/// | chosen | the ride before changes and it can still be made | it stays |
/// | not chosen | every ride before takes its suggested one | the suggested one, if it can be made |
/// | not chosen, or chosen and can't be made | otherwise | the one that can be made and arrives first |
/// | any | none can be made | no connection, and the rides after unknown |
/// | the first ride's, any | the clock passes the last moment it can be made | it can't be made: as for a choice that can't be |
/// | the first ride's, any | the clock passes the last moment the last of them can be made | no connection: every departure gone, and the rides after unknown |
/// | chosen | a new plan still lists it | it stays, if it can still be made |
/// | any | a new plan has a journey of the departures taken ([ConnectionChain.takes]), and the chain is moved onto it | each stays, chosen if it was, and is now that journey's suggested one |
/// | any | a new plan hasn't the journey | each ride's departures as last looked up between its two stops, if they have been ([ConnectionChain.relist]); otherwise as a ride of that plan between the same two stops lists them, or as they were if none is; a choice stays if it's still listed and can still be made |
/// | any | a ride's departures are looked up between its two stops, for a journey no plan has | that ride's departures are those; a choice stays if it's still listed and can still be made, and a ride with no connection has one again if one of them can be made |
library;

import 'package:flutter/foundation.dart' show listEquals;

import 'transit_plan.dart';

/// One run of a trip: a `trip_id` on its service day.
typedef DepartureRun = ({String tripId, String? startDate});

/// [departure]'s run, of [ride]'s departures: the planner leaves out a departure's
/// service day when it's the ride's own.
DepartureRun _runOf(RideDeparture departure, TransitLeg ride) => (
  tripId: departure.tripId,
  startDate: departure.startDate ?? ride.startDate,
);

/// Whether a ride's chosen departure can be made, as the choices before it have it.
enum RideConnection {
  /// It can: the rider makes the ride.
  made,

  /// None of the ride's departures can be made.
  none,

  /// A ride before it has no connection, so nothing is known of this one.
  unknown,
}

/// What's chosen for one ride of a journey.
class RideChoice {
  RideChoice._({
    required this.ride,
    required this.departures,
    required this.first,
    required this.ready,
    required this.catchable,
    required this.index,
    required this.chosen,
    required this.connection,
  });

  final TransitLeg ride;

  /// Departures between the ride's two stops, by departure, as last known: the ride's
  /// own [TransitLeg.departures], those a newer plan lists between the same stops, as
  /// [ConnectionChain.refresh] has them, or those looked up between them, as
  /// [ConnectionChain.relist] has them.
  final List<RideDeparture> departures;

  /// Whether it's the journey's first ride, whose departures are made by the clock.
  final bool first;

  /// When the rider can be at its boarding stop, from the departure taken of the ride
  /// before; null for the first ride, and where that isn't known (no
  /// [TransitLeg.transferSeconds], or nothing before is made).
  final DateTime? ready;

  /// For each of [departures], whether the rider can make it: it runs, and it leaves
  /// once they're [ready]; or for the first ride, the plan doesn't have it missed, and
  /// with the time given, it leaves once the rider can be at the stop, setting out then.
  final List<bool> catchable;

  /// The departure taken, in [departures]; null when the ride lists none (it
  /// takes the ride's own) or [connection] isn't [RideConnection.made].
  final int? index;

  /// Whether the rider chose [index] themselves, rather than it following from the
  /// rides before.
  final bool chosen;
  final RideConnection connection;

  bool get made => connection == RideConnection.made;

  /// The departure taken, if the ride lists its departures.
  RideDeparture? get departure => index == null ? null : departures[index!];

  /// The last of [departures] that runs, if any does.
  RideDeparture? get lastRunning =>
      departures.where((d) => d.status.runs).lastOrNull;

  /// Whether the departure taken is the plan's own, whose stops the plan lists.
  bool get suggested => made && (departure?.suggested ?? true);

  /// When it leaves the boarding stop, if it's made.
  DateTime? get departureTime =>
      made ? departure?.departure ?? ride.departure : null;

  /// When it reaches the alighting stop, if it's made.
  DateTime? get arrival => made ? departure?.arrival ?? ride.arrival : null;

  /// The route of the departure taken: another than the ride's, perhaps.
  TransitRoute? get route => departure?.route ?? ride.route;

  /// Where the departure taken is headed: the planner leaves out a departure's headsign
  /// when it reads the same as the ride's, whatever route the departure is on.
  String? get headsign => switch (departure) {
    null || RideDeparture(suggested: true) => ride.headsign,
    final d => d.headsign ?? ride.headsign,
  };

  /// Whether the times of the departure taken came from real-time data.
  bool get realtime =>
      suggested ? ride.realtime : departure?.realtime ?? ride.realtime;

  /// The leg of the departure taken, with its stops and trips, when it's known: the
  /// ride's own for the suggested one, whose leg the plan has; for another, [fetched]
  /// if it's that departure's, looked up apart from the plan. Null otherwise, when only
  /// its boarding and alighting stops are known.
  TransitLeg? legTaken(TransitLeg? fetched) {
    if (suggested) return ride;
    final departure = this.departure;
    if (departure == null || fetched == null || !made) return null;
    // The planner leaves out a departure's service day when it's the ride's own.
    final startDate = departure.startDate ?? ride.startDate;
    final same =
        fetched.stops.length >= 2 &&
        fetched.tripId == departure.tripId &&
        (fetched.startDate == null || fetched.startDate == startDate);
    return same ? fetched : null;
  }

  /// Seconds waited at the boarding stop for the departure taken: from when the rider
  /// is [ready] when that's known, as the plan has it otherwise.
  int get waitSeconds => switch ((ready, departureTime)) {
    (final ready?, final leaves?) => leaves.difference(ready).inSeconds,
    _ => ride.waitSeconds,
  };
}

/// What's chosen for each ride of a [journey]: see the library's description.
class ConnectionChain {
  /// [journey] as the plan has it: each ride's suggested departure. With [now], the
  /// first ride's departures are made as the clock has it then.
  factory ConnectionChain(Journey journey, {DateTime? now}) =>
      ConnectionChain._evaluate(journey, const {}, null, now);

  ConnectionChain._(
    this.journey,
    this.rides,
    this._picks,
    this._departures,
    this.now,
  );

  /// Works out each ride's choice from [picks], the departures chosen by ride, with
  /// [departures] in place of the rides' own where it's given, at [now].
  factory ConnectionChain._evaluate(
    Journey journey,
    Map<int, DepartureRun> picks,
    List<List<RideDeparture>>? departures,
    DateTime? now,
  ) {
    final rides = journey.rides.toList();
    final choices = <RideChoice>[];
    final kept = <int, DepartureRun>{};
    DateTime? before;
    var broken = false;
    // Whether every ride so far takes its suggested departure, so the rider reaches
    // this one's boarding stop when the plan has them there.
    var asPlanned = true;
    // When the rider, setting out now, can be at the first ride's stop.
    final atStop = now?.add(_toFirstStop(journey));
    for (final (i, ride) in rides.indexed) {
      final listed = departures?[i] ?? ride.departures;
      if (broken) {
        choices.add(
          RideChoice._(
            ride: ride,
            departures: listed,
            first: i == 0,
            ready: null,
            catchable: List.filled(listed.length, false),
            index: null,
            chosen: false,
            connection: RideConnection.unknown,
          ),
        );
        continue;
      }
      final transfer = ride.transferSeconds;
      final ready = i == 0 || before == null || transfer == null
          ? null
          : before.add(Duration(seconds: transfer));
      bool makes(DateTime leaves) => switch ((i, ready, atStop)) {
        (_, final ready?, _) => !leaves.isBefore(ready),
        (0, null, final atStop?) => !leaves.isBefore(atStop),
        _ => true,
      };
      final catchable = [
        for (final d in listed)
          d.status.runs &&
              (ready == null
                  ? !d.missed && makes(d.departure)
                  : makes(d.departure)),
      ];
      int? index;
      var chosen = false;
      final made = listed.isEmpty
          ? makes(ride.departure)
          : catchable.contains(true);
      if (listed.isNotEmpty && made) {
        final pick = picks[i];
        final picked = pick == null
            ? -1
            : listed.indexWhere((d) => _runOf(d, ride) == pick);
        if (picked >= 0 && catchable[picked]) {
          index = picked;
          chosen = true;
          kept[i] = pick!;
        } else {
          final own = listed.indexWhere((d) => d.suggested);
          index = asPlanned && own >= 0 && catchable[own]
              ? own
              : _arrivesFirst(listed, catchable);
        }
      }
      final choice = RideChoice._(
        ride: ride,
        departures: listed,
        first: i == 0,
        ready: ready,
        catchable: catchable,
        index: index,
        chosen: chosen,
        connection: made ? RideConnection.made : RideConnection.none,
      );
      choices.add(choice);
      asPlanned = asPlanned && choice.suggested;
      before = choice.arrival;
      broken = !made;
    }
    return ConnectionChain._(journey, choices, kept, departures, now);
  }

  /// How long the streets to [journey]'s first ride take: its legs before that ride.
  static Duration _toFirstStop(Journey journey) => Duration(
    seconds: journey.legs
        .takeWhile((leg) => leg is! TransitLeg)
        .fold(0, (sum, leg) => sum + leg.duration),
  );

  /// Of the departures that can be made, the one reaching the alighting stop first:
  /// on a tie the suggested one, then the earliest to leave (the list's order).
  static int _arrivesFirst(
    List<RideDeparture> departures,
    List<bool> catchable,
  ) {
    var best = -1;
    for (final (i, d) in departures.indexed) {
      if (!catchable[i]) continue;
      if (best < 0) {
        best = i;
        continue;
      }
      final b = departures[best];
      final earlier = d.arrival.isBefore(b.arrival);
      final tie = d.arrival.isAtSameMomentAs(b.arrival);
      if (earlier || (tie && d.suggested && !b.suggested)) best = i;
    }
    return best;
  }

  final Journey journey;

  /// What's chosen for each of [journey]'s rides, in order.
  final List<RideChoice> rides;

  /// The departures the rider chose, by ride, that can still be made.
  final Map<int, DepartureRun> _picks;

  /// Each ride's departures where they're no longer [journey]'s own (see [refresh]
  /// and [relist]).
  final List<List<RideDeparture>>? _departures;

  /// The time the first ride's departures are made as, or null for as the plan has
  /// them.
  final DateTime? now;

  /// With the rider taking departure [departure] of ride [ride] (its index in
  /// [Journey.rides]), and the rides after following from it. Unchanged if they can't
  /// make that one.
  ConnectionChain select(int ride, int departure) {
    final choice = rides[ride];
    if (!choice.made || !choice.catchable[departure]) return this;
    return ConnectionChain._evaluate(
      journey,
      {..._picks, ride: _runOf(choice.departures[departure], choice.ride)},
      _departures,
      now,
    );
  }

  /// The same choices at [now]: those that can still be made then stay.
  ConnectionChain at(DateTime now) =>
      ConnectionChain._evaluate(journey, _picks, _departures, now);

  /// The same choices in [journey], a newer plan of this journey: each that it still
  /// lists, and that can still be made, stays.
  ConnectionChain rebase(Journey journey) => ConnectionChain._evaluate(
    journey,
    journey.rides.length == rides.length ? _picks : const {},
    null,
    now,
  );

  /// The same choices with the departures of a newer plan's [journeys], none of them
  /// this journey: each ride's as the first ride among them between the same two
  /// stops lists them, or as they were if none is, or if the ride is one of [keep]
  /// (by its index in [rides]), whose departures were looked up and stay until
  /// they're looked up again. Each choice they still list, and that can still be made,
  /// stays.
  ConnectionChain refresh(
    Iterable<Journey> journeys, {
    Set<int> keep = const {},
  }) {
    final others = [for (final j in journeys) ...j.rides];
    return ConnectionChain._evaluate(journey, _picks, [
      for (final (i, choice) in rides.indexed)
        switch (keep.contains(i)
            ? null
            : others.where((o) => _sameStops(o, choice.ride)).firstOrNull) {
          final other? => [
            for (final d in other.departures)
              d.relisted(from: other, to: choice.ride),
          ],
          null => choice.departures,
        },
    ], now);
  }

  /// The same choices with the departures of [lists], by ride (its index in [rides]),
  /// in place of those rides': each as the transit source lists them between the
  /// ride's two stops ([RideDeparture.listed]), read as the ride's own list would say
  /// them. The other rides' stay as they were. Each choice still listed, and that can
  /// still be made, stays; a ride with no connection has one again if one of its new
  /// departures can be made.
  ConnectionChain relist(
    Map<int, List<RideDeparture>> lists,
  ) => ConnectionChain._evaluate(journey, _picks, [
    for (final (i, choice) in rides.indexed)
      switch (lists[i]) {
        final list? => [
          // Each says everything of itself, so the ride it's listed with says
          // nothing for it.
          for (final d in list) d.relisted(from: choice.ride, to: choice.ride),
        ],
        null => choice.departures,
      },
  ], now);

  static bool _sameStops(TransitLeg a, TransitLeg b) =>
      a.boarding.stop.id == b.boarding.stop.id &&
      a.boarding.stop.feed == b.boarding.stop.feed &&
      a.alighting.stop.id == b.alighting.stop.id &&
      a.alighting.stop.feed == b.alighting.stop.feed;

  /// When, after [now], the next of the first ride's departures that can be made then
  /// can't be: the moment after the last one the rider, setting out, can be at its stop
  /// for it. Null without [now], or with none to miss.
  DateTime? get missesAt {
    final now = this.now;
    final first = rides.firstOrNull;
    if (now == null ||
        first == null ||
        first.connection != RideConnection.made) {
      return null;
    }
    final toStop = _toFirstStop(journey);
    final leaving = first.departures.isEmpty
        ? [first.ride.departure]
        : [
            for (final (i, d) in first.departures.indexed)
              if (first.catchable[i]) d.departure,
          ];
    if (leaving.isEmpty) return null;
    final soonest = leaving.reduce((a, b) => a.isBefore(b) ? a : b);
    return soonest.subtract(toStop).add(const Duration(microseconds: 1));
  }

  /// Whether [other]'s rides are the departures taken here, in order, between the same
  /// stops: as many rides, each the same run of a trip (its trip and service day, or,
  /// without a trip, its route and when it's scheduled to leave, as [RideIdentity] has
  /// it). Never while a ride has no connection, nor where what's taken isn't known.
  ///
  /// A service day either leaves out is taken to be the other's, as the planner leaves
  /// it out of a departure that's the ride's own.
  bool takes(Journey other) {
    if (brokenAt != null) return false;
    final theirs = other.rides.toList();
    if (theirs.length != rides.length) return false;
    for (final (i, choice) in rides.indexed) {
      final ride = choice.ride;
      final their = theirs[i];
      if (!_sameStops(ride, their)) return false;
      final departure = choice.departure;
      final tripId = departure?.tripId ?? ride.tripId;
      if (tripId != null || their.tripId != null) {
        final startDate = departure == null
            ? ride.startDate
            : _runOf(departure, ride).startDate;
        final theirDate = their.suggested?.startDate ?? their.startDate;
        if (tripId != their.tripId ||
            (startDate != null &&
                theirDate != null &&
                startDate != theirDate)) {
          return false;
        }
        continue;
      }
      // Neither names its trip: the ride lists no departures (each listed names its
      // trip), so it takes its own.
      if (ride.route?.id != their.route?.id ||
          (ride.boarding.scheduledDeparture ?? ride.departure) !=
              (their.boarding.scheduledDeparture ?? their.departure)) {
        return false;
      }
    }
    return true;
  }

  /// Whether every departure of the first ride is gone: none can be made.
  bool get gone => rides.firstOrNull?.connection == RideConnection.none;

  /// Whether [other] chooses as this does: the same departures listed, made and taken
  /// for each ride.
  bool choosesAs(ConnectionChain other) {
    if (!identical(other.journey, journey) ||
        other.rides.length != rides.length) {
      return false;
    }
    for (final (i, r) in rides.indexed) {
      final o = other.rides[i];
      if (!identical(o.departures, r.departures) ||
          o.index != r.index ||
          o.chosen != r.chosen ||
          o.connection != r.connection ||
          o.ready != r.ready ||
          !listEquals(o.catchable, r.catchable)) {
        return false;
      }
    }
    return true;
  }

  /// Whether every ride is as the plan has it.
  bool get suggested => rides.every((r) => r.suggested);

  /// The first ride with no connection, by its index in [rides]; null if every ride is
  /// made.
  int? get brokenAt {
    final i = rides.indexWhere((r) => !r.made);
    return i < 0 ? null : i;
  }

  /// [index]'s choice, by the ride's place among [journey]'s legs.
  RideChoice? forLeg(int index) {
    final ride = journey.legs[index];
    return rides.where((r) => identical(r.ride, ride)).firstOrNull;
  }

  /// When to leave the start: the plan's time, moved as much as the first ride's
  /// departure is, so there's as long to get to it.
  DateTime get departure {
    final first = rides.firstOrNull;
    final leaves = first?.departureTime;
    if (first == null || leaves == null) return journey.departure;
    return journey.departure.add(leaves.difference(first.ride.departure));
  }

  /// When the rider reaches the destination: the last ride's arrival, then the way from
  /// its stop. Null when a ride has no connection.
  DateTime? get arrival {
    final last = rides.lastOrNull;
    if (last == null) return journey.arrival;
    final reaches = last.arrival;
    if (reaches == null) return null;
    final after = journey.legs.skip(journey.legs.indexOf(last.ride) + 1);
    return reaches.add(
      Duration(seconds: after.fold(0, (sum, leg) => sum + leg.duration)),
    );
  }

  /// Seconds door to door, when the journey is made.
  int? get duration => arrival?.difference(departure).inSeconds;

  /// When each of [journey]'s legs starts and ends with these choices: a ride's as its
  /// departure taken has them, and each walk as long as the plan has it, after what
  /// comes before. Null when a ride has no connection.
  List<({DateTime start, DateTime end})>? get legTimes {
    if (brokenAt != null) return null;
    final shift = departure.difference(journey.departure);
    final times = <({DateTime start, DateTime end})>[];
    DateTime? after;
    for (final (i, leg) in journey.legs.indexed) {
      final choice = leg is TransitLeg ? forLeg(i) : null;
      if (choice != null) {
        times.add((start: choice.departureTime!, end: choice.arrival!));
        after = choice.arrival;
      } else {
        final start = after ?? leg.departure.add(shift);
        final end = start.add(Duration(seconds: leg.duration));
        times.add((start: start, end: end));
        after = end;
      }
    }
    return times;
  }
}

/// The other routes worth naming beside each of [journey]'s rides, as the plan has the
/// journey: up to [limit] that run between the same two stops, can be made, and leave
/// within [within] of the suggested departure, in the order they leave, each once.
List<List<TransitRoute>> otherRoutes(
  Journey journey, {
  int limit = 2,
  Duration within = const Duration(minutes: 15),
}) => [
  for (final choice in ConnectionChain(journey).rides)
    switch (choice.ride.suggested) {
      null => const [],
      final own => _others(choice, own, limit, within),
    },
];

List<TransitRoute> _others(
  RideChoice choice,
  RideDeparture own,
  int limit,
  Duration within,
) {
  final seen = <String>{?choice.ride.route?.id};
  final routes = <TransitRoute>[];
  for (final (i, d) in choice.departures.indexed) {
    final route = d.route;
    if (route == null || !choice.catchable[i]) continue;
    if (d.departure.difference(own.departure).abs() > within) continue;
    if (!seen.add(route.id)) continue;
    routes.add(route);
    if (routes.length == limit) break;
  }
  return routes;
}
