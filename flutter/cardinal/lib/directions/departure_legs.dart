/// The legs of the departures a rider chose instead of a plan's own, fetched as they're
/// chosen, so the map can draw the journey the rider means to take.
///
/// A plan describes only each ride's suggested departure in full; of the others it says
/// when they leave and arrive. Once the rider takes another, its leg (its stops and its
/// line) is asked of the transit source, and kept for as long as the plan is, so asking
/// again, or listing its stops later, needs no second lookup.
///
/// | A departure's leg | When | Then |
/// |---|---|---|
/// | not known | a chain takes it, not the plan's own | fetching |
/// | fetching | the source answers while a chain still takes it | known; listeners told |
/// | fetching | the latest chain no longer takes it | dropped; its answer ignored |
/// | fetching | the source fails | not known; asked again when a chain takes it again |
/// | known | another chain takes it | known, not asked again |
/// | any | a new plan | not known |
library;

import 'dart:async';

import 'package:flutter/foundation.dart';

import 'connections.dart';
import 'route_source.dart';
import 'transit_plan.dart';

/// Which departure a leg is of, and how the plan got to and from stops: the travel
/// mode decides which departures the planner lists.
typedef DepartureLegKey = ({RideQuery query, TransitAccess access});

/// The departure [choice] takes, as the transit source looks it up, unless it's the
/// plan's own or what's needed isn't known.
RideQuery? rideQueryOf(RideChoice choice) {
  final departure = choice.departure;
  if (departure == null || choice.suggested || !choice.made) return null;
  final ride = choice.ride;
  final feed = ride.boarding.stop.feed;
  // The planner leaves out a departure's service day when it's the ride's own.
  final startDate = departure.startDate ?? ride.startDate;
  if (feed == null || startDate == null) return null;
  return (
    feed: feed,
    boardStopId: ride.boarding.stop.id,
    alightStopId: ride.alighting.stop.id,
    tripId: departure.tripId,
    startDate: startDate,
    departure: departure.departure,
  );
}

/// See the library's description.
class DepartureLegs extends ChangeNotifier {
  DepartureLegs(this._source);

  final TransitSource _source;

  /// The plan the legs are of.
  TransitPlan? _plan;
  final _legs = <DepartureLegKey, TransitLeg>{};

  /// What's being fetched, each with what aborts it.
  final _fetching = <DepartureLegKey, Completer<void>>{};
  var _disposed = false;

  /// The leg of the departure [choice] takes, of a plan made getting to stops by
  /// [access], if it's another than the plan's own and has been fetched.
  TransitLeg? legOf(RideChoice choice, TransitAccess access) {
    final query = rideQueryOf(choice);
    return query == null ? null : _legs[(query: query, access: access)];
  }

  /// Whether the leg of [query] is being fetched.
  @visibleForTesting
  bool fetching(RideQuery query, TransitAccess access) =>
      _fetching.containsKey((query: query, access: access));

  /// Fetches the leg of each departure [chain] takes but the plan's own, of [plan],
  /// made getting to stops by [access], and stops fetching those it no longer takes.
  /// A null [chain] takes none.
  void follow(TransitPlan plan, ConnectionChain? chain, TransitAccess access) {
    if (!identical(plan, _plan)) {
      // Another plan's real-time data may have moved its departures.
      _plan = plan;
      _legs.clear();
      _abortWhere((_) => true);
    }
    final wanted = <DepartureLegKey>{
      for (final choice in chain?.rides ?? const <RideChoice>[])
        if (rideQueryOf(choice) case final query?)
          (query: query, access: access),
    };
    _abortWhere((key) => !wanted.contains(key));
    for (final key in wanted) {
      if (_legs.containsKey(key) || _fetching.containsKey(key)) continue;
      unawaited(_fetch(key));
    }
  }

  void _abortWhere(bool Function(DepartureLegKey key) test) {
    final dropped = _fetching.keys.where(test).toList();
    for (final key in dropped) {
      _fetching.remove(key)!.complete();
    }
  }

  Future<void> _fetch(DepartureLegKey key) async {
    final abort = Completer<void>();
    _fetching[key] = abort;
    TransitLeg? leg;
    try {
      leg = await _source.ride(
        query: key.query,
        access: key.access,
        abort: abort.future,
      );
    } on Object catch (e) {
      // The map draws the plan's line for it, between the same two stops.
      if (!abort.isCompleted) debugPrint('transit ride: $e');
    }
    // Dropped while it was asked: the chain moved on, or the plan did.
    if (_disposed || !identical(_fetching[key], abort)) return;
    _fetching.remove(key);
    if (leg == null) return;
    _legs[key] = leg;
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _abortWhere((_) => true);
    super.dispose();
  }
}
