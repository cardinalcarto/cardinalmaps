/// The nearby transit list as it's shown: which rows, in which order, as answers arrive.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:geolocator/geolocator.dart' show Geolocator;
import 'package:http/http.dart' as http;

import '../directions/transit_plan.dart';
import '../store/store.dart';
import 'nearby_source.dart';
import 'nearby_transit.dart';

/// How far the origin moves before the next answer re-sorts the list, in meters.
const resortMeters = 100.0;

/// The rows of the nearby transit list, one per GTFS route, and how each new answer is
/// applied to them, so rows don't jump around while they're being read.
///
/// States:
///
/// - **Unsorted**: nothing applied since it was made or [reset]. [routes] is empty and
///   there's no sort origin.
/// - **Shown**: [routes] are in the order of the answer last sorted by, and the sort
///   origin and access are where and how that answer was asked.
///
/// Transitions, on [apply] of an answer asked from an origin by an access, and on
/// [reset]:
///
/// 1. Unsorted, apply: **sort**. [routes] take the answer's order (soonest catchable
///    departure first, as the router orders them), and its origin and access become
///    the sort origin and access. Now Shown.
/// 2. Shown, apply from [resortMeters] or more from the sort origin, or by another
///    access (wheelchair turned on or off): **sort**, as in 1.
/// 3. Shown, apply by the same access from less than [resortMeters] from the sort
///    origin: **freeze**. The order stays, and rows are matched by [NearbyRoute.key]
///    (feed and `route_id`):
///    a. a row the answer has is replaced in place by the answer's, with its pages,
///       stops, and departures;
///    b. a row the answer doesn't have is removed;
///    c. the answer's routes that aren't rows yet are appended at the end, in the
///       answer's order.
///
///    The sort origin stays, so small moves add up to a re-sort.
/// 4. Either, [reset]: Unsorted. The list isn't shown any more, so the next answer
///    sorts.
class NearbyRows {
  NearbyRows();

  /// The rows, in the order they're shown.
  List<NearbyRoute> get routes => _routes;
  List<NearbyRoute> _routes = const [];

  /// The answer last applied: its real-time status, and why it has no routes.
  NearbyTransit? get latest => _latest;
  NearbyTransit? _latest;

  /// Where the answer last sorted by was asked from, or null when Unsorted.
  LngLat? get sortedAt => _sortedAt;
  LngLat? _sortedAt;

  /// How the answer last sorted by got to stops, or null when Unsorted.
  TransitAccess? get sortedBy => _sortedBy;
  TransitAccess? _sortedBy;

  /// Applies [answer], asked from [origin], getting to stops by [access].
  void apply(
    NearbyTransit answer, {
    required LngLat origin,
    required TransitAccess access,
  }) {
    _latest = answer;
    final sortedAt = _sortedAt;
    if (sortedAt == null ||
        access != _sortedBy ||
        _meters(sortedAt, origin) >= resortMeters) {
      _routes = List.unmodifiable(answer.routes);
      _sortedAt = origin;
      _sortedBy = access;
      return;
    }
    final fresh = {for (final route in answer.routes) route.key: route};
    final kept = [for (final row in _routes) ?fresh.remove(row.key)];
    _routes = List.unmodifiable([...kept, ...fresh.values]);
  }

  /// Forgets the rows, as the list stops being shown: the next answer sorts.
  void reset() {
    _routes = const [];
    _latest = null;
    _sortedAt = null;
    _sortedBy = null;
  }

  static double _meters(LngLat a, LngLat b) =>
      Geolocator.distanceBetween(a.lat, a.lon, b.lat, b.lon);
}

/// Asks [source] for nearby departures and applies each answer to [rows], telling its
/// listeners. The screen decides when to [refresh] and from where.
class NearbyTransitController extends ChangeNotifier {
  NearbyTransitController(this.source);

  final NearbyTransitSource source;
  final rows = NearbyRows();

  /// Completes to abandon the question being asked, if any.
  Completer<void>? _asking;

  /// Asks again from [origin], getting to stops by [access], and applies the answer:
  /// one by another access than the rows were sorted by sorts them again.
  ///
  /// A refresh started before this one is answered abandons it, and its answer isn't
  /// applied. Throws as [NearbyTransitSource.nearby] does, leaving the rows as they were.
  Future<void> refresh({
    required LngLat origin,
    required TransitAccess access,
  }) async {
    _asking?.complete();
    final asking = _asking = Completer<void>();
    try {
      final answer = await source.nearby(
        location: origin,
        access: access,
        abort: asking.future,
      );
      if (asking.isCompleted) return;
      rows.apply(answer, origin: origin, access: access);
      notifyListeners();
    } on http.RequestAbortedException {
      if (!asking.isCompleted) rethrow;
    } finally {
      if (identical(_asking, asking)) _asking = null;
    }
  }

  /// Forgets the rows, as the list stops being shown, abandoning any refresh.
  void reset() {
    _asking?.complete();
    _asking = null;
    rows.reset();
    notifyListeners();
  }

  @override
  void dispose() {
    _asking?.complete();
    _asking = null;
    super.dispose();
  }
}
