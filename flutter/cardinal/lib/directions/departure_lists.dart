/// The departures between the two stops of each ride of a journey no plan has any
/// more, asked of the transit source apart from a plan.
///
/// A plan lists each ride's departures with its journeys. A journey kept open in Trip
/// details after a newer plan dropped it has no such list of its own from then on: a
/// ride of the newer plan between the same two stops may lend it one, and where none
/// does it would keep the list it had, growing stale. So each of its rides is asked
/// for by its stops, around the departure it takes, once for each plan it's missing
/// from; what a plan lends stands in until the answer.
///
/// | A ride's list | When | Then |
/// |---|---|---|
/// | not asked for | a plan without the journey is shown, the ride made | asking, around the departure it takes, two before and seven from it on |
/// | not asked for | a plan without the journey is shown, the first ride with every departure gone | asking, around the time it is, so the departures still to come are listed |
/// | not asked for | a plan without the journey is shown, a later ride with no connection | asking, around when the rider is ready for it |
/// | not asked for | a plan without the journey is shown, the ride after one with no connection | not asked for: nothing says when the rider would be there |
/// | asking | the source answers with departures | looked up; listeners told, and the journey takes it once no finger is down |
/// | asking | the source answers with none | as it was: the list showing stays |
/// | asking | the source fails, or the asker gives up | as it was; asked again under the next plan |
/// | asking | the source can't be asked at all ([TransitDeparturesUnsupported]) | as it was, and no ride is asked for again, of that source |
/// | asking | a newer plan without the journey is shown | dropped, its answer ignored; asking again for that plan |
/// | looked up | a newer plan without the journey is shown | asking again, the list looked up standing in, rather than one the plan lends |
/// | any | a plan has the journey again, or one of the departures taken; or the journey is closed | not asked for, nothing looked up: the plan's own lists are the journey's |
library;

import 'dart:async';

import 'package:flutter/foundation.dart';

import 'connections.dart';
import 'route_source.dart';
import 'transit_plan.dart';

/// How many departures a lookup lists before the time it's around, and from it on: a
/// plan's two and six around a ride's own, the one at the time being the first of
/// those from it on.
const lookupBefore = 2;
const lookupAfter = 7;

/// What to ask the transit source for [choice]'s departures at [now], or null if it
/// isn't to be asked: around the departure taken; for a ride with none it can make,
/// around when the rider could first board, which for the first ride is the time it
/// is. A ride after one with no connection isn't asked for, nor one whose stops' feed
/// isn't known.
DeparturesQuery? departuresQueryOf(RideChoice choice, {required DateTime now}) {
  final ride = choice.ride;
  final feed = ride.boarding.stop.feed;
  if (feed == null) return null;
  final around = switch (choice.connection) {
    RideConnection.unknown => null,
    RideConnection.made => choice.departureTime,
    RideConnection.none => choice.first ? now : choice.ready ?? ride.departure,
  };
  if (around == null) return null;
  return (
    feed: feed,
    boardStopId: ride.boarding.stop.id,
    alightStopId: ride.alighting.stop.id,
    around: around,
    before: lookupBefore,
    after: lookupAfter,
    ready: null,
  );
}

/// See the library's description.
class DepartureLists extends ChangeNotifier {
  DepartureLists(this._source);

  final TransitSource _source;

  /// The sources that can't be asked for departures between stops, for as long as
  /// each lasts.
  static final _unsupported = Expando<bool>('departures unsupported');

  /// Whether the source has said it can't be asked.
  bool get unsupported => _unsupported[_source] ?? false;

  /// The plan the rides were last asked for under.
  TransitPlan? _plan;

  /// Each ride's list as last looked up, by its index among the journey's rides, as
  /// the source gave it: every departure saying its own route, headsign and day.
  final _lists = <int, List<RideDeparture>>{};

  /// What's being asked, by ride, each with what gives up on it.
  final _asking = <int, Completer<void>>{};
  var _disposed = false;

  /// The lists looked up, by ride.
  Map<int, List<RideDeparture>> get lists => Map.unmodifiable(_lists);

  /// Counts the lists looked up, so whoever takes them knows whether it has them all.
  int get revision => _revision;
  int _revision = 0;

  /// Whether ride [ride]'s list is being asked for.
  @visibleForTesting
  bool asking(int ride) => _asking.containsKey(ride);

  /// Asks for the departures of each of [chain]'s rides, of a journey [plan] hasn't,
  /// made getting to stops by [access], as the clock has it at [now]: once for each
  /// plan, giving up on what an older one was asked.
  void ask(
    TransitPlan plan,
    ConnectionChain chain,
    TransitAccess access,
    DateTime now,
  ) {
    if (identical(plan, _plan)) return;
    _plan = plan;
    _giveUp();
    if (unsupported) return;
    for (final (i, choice) in chain.rides.indexed) {
      final query = departuresQueryOf(choice, now: now);
      if (query != null) unawaited(_ask(i, query, access));
    }
  }

  /// Gives up on what's being asked and forgets what was looked up: the journey is a
  /// plan's own again, or closed.
  void forget() {
    _plan = null;
    _giveUp();
    _lists.clear();
  }

  void _giveUp() {
    final asking = _asking.values.toList();
    _asking.clear();
    for (final abort in asking) {
      abort.complete();
    }
  }

  Future<void> _ask(
    int ride,
    DeparturesQuery query,
    TransitAccess access,
  ) async {
    final abort = Completer<void>();
    _asking[ride] = abort;
    List<RideDeparture>? list;
    try {
      list = await _source.departures(
        query: query,
        access: access,
        abort: abort.future,
      );
    } on TransitDeparturesUnsupported {
      // Nothing more is asked of it; what plans lend is all there is.
      _unsupported[_source] = true;
      if (!_disposed) _giveUp();
      return;
    } on Object catch (e) {
      // The list showing stays, and the next plan asks again.
      if (!abort.isCompleted) debugPrint('transit departures: $e');
    }
    // Given up on while it was asked: the plan moved on, or the journey did.
    if (_disposed || !identical(_asking[ride], abort)) return;
    _asking.remove(ride);
    // A list of nothing says less than the one showing.
    if (list == null || list.isEmpty) return;
    _lists[ride] = list;
    _revision++;
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _giveUp();
    super.dispose();
  }
}
