/// Which departure of each ride in a journey the rider means to take, and what that
/// makes of the rest of the journey: a ride's connections are the departures listed
/// with it, and taking a later one of a ride can mean a later one of the next.
///
/// It starts with each ride's suggested departure, the plan's own. Choosing another
/// one of a ride sets when the rider can be at the next ride's boarding stop (its
/// arrival plus the next ride's [TransitLeg.transferSeconds]), and so which of the
/// next ride's departures they can make. Down the journey, each ride after keeps what
/// was chosen for it while that can still be made; otherwise it takes its suggested
/// departure if that can be made, or else the first that can. A ride none of whose
/// departures can be made has no connection in its list, and what comes after it isn't
/// known.
///
/// | A ride's choice | When | Then |
/// |---|---|---|
/// | the suggested one | the plan is read | each ride's suggested departure |
/// | any | the rider chooses one they can make | that one, chosen |
/// | chosen | the ride before changes and it can still be made | it stays |
/// | chosen | the ride before changes and it can't be made | the suggested one if it can be, else the first that can |
/// | any | none can be made | no connection, and the rides after unknown |
/// | chosen | a new plan still lists it | it stays, if it can still be made |
library;

import 'transit_plan.dart';

/// One run of a trip: a `trip_id` on its service day.
typedef DepartureRun = ({String tripId, String? startDate});

DepartureRun _runOf(RideDeparture departure) =>
    (tripId: departure.tripId, startDate: departure.startDate);

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
    required this.ready,
    required this.catchable,
    required this.index,
    required this.chosen,
    required this.connection,
  });

  final TransitLeg ride;

  /// When the rider can be at its boarding stop, from the departure taken of the ride
  /// before; null for the first ride, and where that isn't known (no
  /// [TransitLeg.transferSeconds], or nothing before is made).
  final DateTime? ready;

  /// For each of the ride's [TransitLeg.departures], whether the rider can make it: it
  /// runs, and it leaves once they're [ready], or, without [ready], the plan doesn't
  /// have it missed.
  final List<bool> catchable;

  /// The departure taken, in [TransitLeg.departures]; null when the ride lists none (it
  /// takes the ride's own) or [connection] isn't [RideConnection.made].
  final int? index;

  /// Whether the rider chose [index] themselves, rather than it following from the
  /// rides before.
  final bool chosen;
  final RideConnection connection;

  bool get made => connection == RideConnection.made;

  /// The departure taken, if the ride lists its departures.
  RideDeparture? get departure =>
      index == null ? null : ride.departures[index!];

  /// Whether the departure taken is the plan's own, whose stops the plan lists.
  bool get suggested => made && (departure?.suggested ?? true);

  /// When it leaves the boarding stop, if it's made.
  DateTime? get departureTime =>
      made ? departure?.departure ?? ride.departure : null;

  /// When it reaches the alighting stop, if it's made.
  DateTime? get arrival => made ? departure?.arrival ?? ride.arrival : null;

  /// The route of the departure taken: another than the ride's, perhaps.
  TransitRoute? get route => departure?.route ?? ride.route;

  /// Where the departure taken is headed: a departure of the ride's route that doesn't
  /// say is headed where the ride is.
  String? get headsign => switch (departure) {
    null || RideDeparture(suggested: true) => ride.headsign,
    final d => d.headsign ?? (d.route == null ? ride.headsign : null),
  };

  /// Whether the times of the departure taken came from real-time data.
  bool get realtime =>
      suggested ? ride.realtime : departure?.realtime ?? ride.realtime;

  /// The ride's stops when they're known for the departure taken: the plan lists the
  /// suggested trip's alone. Null for another, whose boarding and alighting stops are
  /// all that's known.
  List<StopVisit>? get stops => suggested ? ride.stops : null;

  /// Seconds waited at the boarding stop for the departure taken: from when the rider
  /// is [ready] when that's known, as the plan has it otherwise.
  int get waitSeconds => switch ((ready, departureTime)) {
    (final ready?, final leaves?) => leaves.difference(ready).inSeconds,
    _ => ride.waitSeconds,
  };
}

/// What's chosen for each ride of a [journey]: see the library's description.
class ConnectionChain {
  /// [journey] as the plan has it: each ride's suggested departure.
  factory ConnectionChain(Journey journey) =>
      ConnectionChain._evaluate(journey, const {});

  ConnectionChain._(this.journey, this.rides, this._picks);

  /// Works out each ride's choice from [picks], the departures chosen by ride.
  factory ConnectionChain._evaluate(
    Journey journey,
    Map<int, DepartureRun> picks,
  ) {
    final rides = journey.rides.toList();
    final choices = <RideChoice>[];
    final kept = <int, DepartureRun>{};
    DateTime? before;
    var broken = false;
    for (final (i, ride) in rides.indexed) {
      final departures = ride.departures;
      if (broken) {
        choices.add(
          RideChoice._(
            ride: ride,
            ready: null,
            catchable: List.filled(departures.length, false),
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
      bool makes(DateTime leaves) => ready == null || !leaves.isBefore(ready);
      final catchable = [
        for (final d in departures)
          d.status.runs && (ready == null ? !d.missed : makes(d.departure)),
      ];
      int? index;
      var chosen = false;
      final made = departures.isEmpty
          ? makes(ride.departure)
          : catchable.contains(true);
      if (departures.isNotEmpty && made) {
        final pick = picks[i];
        final picked = pick == null
            ? -1
            : departures.indexWhere((d) => _runOf(d) == pick);
        if (picked >= 0 && catchable[picked]) {
          index = picked;
          chosen = true;
          kept[i] = pick!;
        } else {
          final own = departures.indexWhere((d) => d.suggested);
          index = own >= 0 && catchable[own] ? own : catchable.indexOf(true);
        }
      }
      final choice = RideChoice._(
        ride: ride,
        ready: ready,
        catchable: catchable,
        index: index,
        chosen: chosen,
        connection: made ? RideConnection.made : RideConnection.none,
      );
      choices.add(choice);
      before = choice.arrival;
      broken = !made;
    }
    return ConnectionChain._(journey, choices, kept);
  }

  final Journey journey;

  /// What's chosen for each of [journey]'s rides, in order.
  final List<RideChoice> rides;

  /// The departures the rider chose, by ride, that can still be made.
  final Map<int, DepartureRun> _picks;

  /// With the rider taking departure [departure] of ride [ride] (its index in
  /// [Journey.rides]), and the rides after following from it. Unchanged if they can't
  /// make that one.
  ConnectionChain select(int ride, int departure) {
    final choice = rides[ride];
    if (!choice.made || !choice.catchable[departure]) return this;
    return ConnectionChain._evaluate(journey, {
      ..._picks,
      ride: _runOf(choice.ride.departures[departure]),
    });
  }

  /// The same choices in [journey], a newer plan of this journey: each that it still
  /// lists, and that can still be made, stays.
  ConnectionChain rebase(Journey journey) => ConnectionChain._evaluate(
    journey,
    journey.rides.length == rides.length ? _picks : const {},
  );

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
  for (final (i, d) in choice.ride.departures.indexed) {
    final route = d.route;
    if (route == null || !choice.catchable[i]) continue;
    if (d.departure.difference(own.departure).abs() > within) continue;
    if (!seen.add(route.id)) continue;
    routes.add(route);
    if (routes.length == limit) break;
  }
  return routes;
}
