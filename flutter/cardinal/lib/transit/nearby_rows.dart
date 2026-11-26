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

/// Where [page] of [route] remembers its headsign: the route at the page's stop.
HeadsignStop headsignStop(NearbyRoute route, HeadsignPage page) =>
    (feed: route.feed, routeId: route.route.id, stopId: page.stop.id);

/// The headsign remembered when a row settles on [page]: its first, the most frequent;
/// empty for a page without headsigns.
String rememberedHeadsign(HeadsignPage page) =>
    page.headsigns.firstOrNull ?? '';

/// Whether [page] is where [headsign] goes: one of its headsigns, or, when empty, a page
/// without any.
bool _under(HeadsignPage page, String headsign) => headsign.isEmpty
    ? page.headsigns.isEmpty
    : page.headsigns.contains(headsign);

/// The page of [route] a row opens on, by index: the page at a stop whose remembered
/// headsign is among its own (the most recently chosen, when several are), or else the
/// one whose catchable trip leaves soonest (the first of those, in the route's order).
int openingPage(
  NearbyRoute route,
  Map<HeadsignStop, RememberedHeadsign> remembered,
) {
  int? chosen;
  DateTime? chosenAt;
  for (final (i, page) in route.pages.indexed) {
    final r = remembered[headsignStop(route, page)];
    if (r == null || !_under(page, r.headsign)) continue;
    if (chosenAt == null || r.chosenAt.isAfter(chosenAt)) {
      chosen = i;
      chosenAt = r.chosenAt;
    }
  }
  if (chosen != null) return chosen;
  var soonest = 0;
  for (final (i, page) in route.pages.indexed) {
    if (page.soonestDeparture.isBefore(route.pages[soonest].soonestDeparture)) {
      soonest = i;
    }
  }
  return soonest;
}

/// The page of [route] that goes on showing [page] from an earlier answer, by index: the
/// first that shares a headsign with it (or that has none, as it had none), or null.
int? _keptPage(NearbyRoute route, HeadsignPage page) {
  for (final (i, fresh) in route.pages.indexed) {
    final shares = page.headsigns.isEmpty
        ? fresh.headsigns.isEmpty
        : page.headsigns.any(fresh.headsigns.contains);
    if (shares) return i;
  }
  return null;
}

/// The rows of the nearby transit list, one per GTFS route, the page each shows, and how
/// each new answer is applied to them, so rows don't jump around while they're being
/// read.
///
/// A row shows one of its route's pages at a time, and the rider swipes between them.
/// Which page a row shows is decided once per row and then kept:
///
/// - A row that's new to the list opens on its [openingPage]: the page at a stop whose
///   remembered headsign is among the page's own, or else the page with the soonest
///   catchable departure.
/// - A row the list already has keeps showing its page from one answer to the next: the
///   first of the answer's pages that shares a headsign with the one it showed. If none
///   does, it opens again as a new row would.
/// - A swipe that settles shows the page it settled on ([show]).
///
/// States:
///
/// - **Unsorted**: nothing applied since it was made. [routes] is empty and there's no
///   sort origin.
/// - **Shown**: [routes] are in the order of the answer last sorted by (see **sort**),
///   each showing a page, and the sort origin and access are where and how that answer
///   was asked.
///
/// Transitions, on [apply] of an answer asked from an origin by an access, and on [show]:
///
/// 1. Unsorted, apply: **sort**. Each route opens on its page, and [routes] are ordered
///    by the soonest catchable departure of the page each one shows (the router's order
///    breaking ties); the answer's origin and access become the sort origin and access.
///    Now Shown.
/// 2. Shown, apply from [resortMeters] or more from the sort origin, or by another
///    access (wheelchair turned on or off): **sort**, as in 1, with each row that's
///    still there keeping its page.
/// 3. Shown, apply by the same access from less than [resortMeters] from the sort
///    origin: **freeze**. The order stays, and rows are matched by [NearbyRoute.key]
///    (feed and `route_id`):
///    a. a row the answer has is replaced in place by the answer's, with its pages,
///       stops, and departures, and keeps its page;
///    b. a row the answer doesn't have is removed;
///    c. the answer's routes that aren't rows yet open on their pages, and are appended
///       at the end, ordered among themselves as a sort orders them.
///
///    The sort origin stays, so small moves add up to a re-sort.
/// 4. Shown, [show] a page of a row: the row shows it. Nothing moves; the order changes
///    only at the next sort.
///
/// Why the order comes from the page each row shows, and not from the route's soonest
/// page (which is how the router orders routes): a row's countdown is its shown page's.
/// A route whose soonest departure is in 5 minutes, but whose row shows the remembered
/// page leaving in 12, would otherwise sit above a row showing 5 minutes, and the
/// numbers on screen would read out of order: "12 min" above "5 min". Sorting by what's
/// shown keeps the countdowns rising down the list as each sort leaves them.
///
/// Why a swipe doesn't sort (4): the row is under the rider's finger. Showing a page
/// that leaves later would move the row down the list as it settles, and the rider would
/// lose it, or swipe another row that slid into its place. So a swipe changes only what
/// the row shows, and the countdowns may then read out of order until the next sort.
///
/// How that fits with the frozen order (3): between sorts nothing re-orders rows,
/// whether departures leave, real-time data moves them, or the rider swipes; the order
/// is the one the last sort gave, from the pages shown then. A sort happens only once
/// the rider has moved [resortMeters] or changed access, as the list is about somewhere
/// else, and only then does a swiped row's new page count towards its place.
class NearbyRows {
  NearbyRows();

  /// The rows, in the order they're shown.
  List<NearbyRoute> get routes => _routes;
  List<NearbyRoute> _routes = const [];

  /// The page each row shows, by index into its route's pages.
  Map<RouteKey, int> _shown = const {};

  /// The answer last applied: its real-time status, and why it has no routes.
  NearbyTransit? get latest => _latest;
  NearbyTransit? _latest;

  /// Where the answer last sorted by was asked from, or null when Unsorted.
  LngLat? get sortedAt => _sortedAt;
  LngLat? _sortedAt;

  /// How the answer last sorted by got to stops, or null when Unsorted.
  TransitAccess? get sortedBy => _sortedBy;
  TransitAccess? _sortedBy;

  /// Which page the row of [key] shows, by index into its route's pages.
  int shown(RouteKey key) => _shown[key] ?? 0;

  /// The page the row of [route] shows.
  HeadsignPage? shownPage(NearbyRoute route) =>
      route.pages.elementAtOrNull(shown(route.key));

  /// Applies [answer], asked from [origin], getting to stops by [access], opening new
  /// rows by the headsigns [remembered] at their pages' stops.
  void apply(
    NearbyTransit answer, {
    required LngLat origin,
    required TransitAccess access,
    Map<HeadsignStop, RememberedHeadsign> remembered = const {},
  }) {
    _latest = answer;
    final before = {for (final row in _routes) row.key: row};
    final shown = <RouteKey, int>{
      for (final route in answer.routes)
        route.key:
            _keep(before[route.key], route) ?? openingPage(route, remembered),
    };
    _shown = shown;
    final sortedAt = _sortedAt;
    if (sortedAt == null ||
        access != _sortedBy ||
        _meters(sortedAt, origin) >= resortMeters) {
      _routes = List.unmodifiable(_sorted(answer.routes));
      _sortedAt = origin;
      _sortedBy = access;
      return;
    }
    final fresh = {for (final route in answer.routes) route.key: route};
    final kept = [for (final row in _routes) ?fresh.remove(row.key)];
    _routes = List.unmodifiable([...kept, ..._sorted(fresh.values)]);
  }

  /// Shows page [index] of the row of [key], when it has one, leaving the order as it is;
  /// returns the page.
  HeadsignPage? show(RouteKey key, int index) {
    final route = _routes.where((r) => r.key == key).firstOrNull;
    final page = route?.pages.elementAtOrNull(index);
    if (page == null) return null;
    _shown = {..._shown, key: index};
    return page;
  }

  /// The page of [route] that goes on showing what [row] showed, if anything does.
  int? _keep(NearbyRoute? row, NearbyRoute route) => switch (row) {
    final row? => switch (shownPage(row)) {
      final page? => _keptPage(route, page),
      null => null,
    },
    null => null,
  };

  /// [routes] by the soonest catchable departure of the page each shows, in their own
  /// order when two leave together.
  List<NearbyRoute> _sorted(Iterable<NearbyRoute> routes) {
    DateTime soonest(NearbyRoute route) => shownPage(route)!.soonestDeparture;
    final indexed = [...routes.indexed]
      ..sort((a, b) {
        final bySoonest = soonest(a.$2).compareTo(soonest(b.$2));
        return bySoonest != 0 ? bySoonest : a.$1.compareTo(b.$1);
      });
    return [for (final (_, route) in indexed) route];
  }

  static double _meters(LngLat a, LngLat b) =>
      Geolocator.distanceBetween(a.lat, a.lon, b.lat, b.lon);
}

/// Asks [source] for nearby departures and applies each answer to [rows], telling its
/// listeners, and remembers in [headsigns] the page a row's swipe settles on. The screen
/// decides when to [refresh] and from where.
///
/// An answer uses the data at hand while real-time data is still being fetched
/// ([NearbyTransitSource.realtimeArrived]); once new data arrives, the
/// controller asks again from the same place, and that answer is applied as any other
/// from there: in place, with the order frozen.
class NearbyTransitController extends ChangeNotifier {
  NearbyTransitController(
    this.source, {
    required this.headsigns,
    this.clock = DateTime.now,
  });

  final NearbyTransitSource source;

  /// Where the headsign a row settles on is remembered.
  final HeadsignStore headsigns;

  /// What time it is, for when a headsign was chosen.
  final DateTime Function() clock;

  final rows = NearbyRows();

  /// Completes to abandon the question being asked, if any.
  Completer<void>? _asking;

  /// Counts questions and abandonments: live times are asked for only if nothing has been
  /// asked since the answer that waits for them.
  int _asks = 0;
  bool _disposed = false;

  /// Asks again from [origin], getting to stops by [access], and applies the answer:
  /// one by another access than the rows were sorted by sorts them again. When the
  /// answer used older or scheduled times while real-time data was on its way, it asks again once
  /// that has arrived, unless another question comes first.
  ///
  /// A refresh started before this one is answered abandons it, and its answer isn't
  /// applied. Throws as [NearbyTransitSource.nearby] does, leaving the rows as they were.
  Future<void> refresh({
    required LngLat origin,
    required TransitAccess access,
  }) async {
    final asked = await _ask(origin, access);
    if (asked != null) unawaited(_whenLive(asked, origin, access));
  }

  /// Asks from [origin] by [access] and applies the answer, returning which question it
  /// was, or null if it was abandoned.
  Future<int?> _ask(LngLat origin, TransitAccess access) async {
    _asking?.complete();
    final asking = _asking = Completer<void>();
    final ask = ++_asks;
    try {
      final answer = await source.nearby(
        location: origin,
        access: access,
        abort: asking.future,
      );
      if (asking.isCompleted) return null;
      final remembered = await headsigns.recall([
        for (final route in answer.routes)
          for (final page in route.pages) headsignStop(route, page),
      ]);
      if (asking.isCompleted) return null;
      rows.apply(
        answer,
        origin: origin,
        access: access,
        remembered: remembered,
      );
      notifyListeners();
      return ask;
    } on http.RequestAbortedException {
      if (!asking.isCompleted) rethrow;
      return null;
    } finally {
      if (identical(_asking, asking)) _asking = null;
    }
  }

  /// Asks again from [origin] by [access] once real-time data the answer to question
  /// [asked] went without arrives, if nothing has been asked since.
  Future<void> _whenLive(int asked, LngLat origin, TransitAccess access) async {
    if (_disposed || asked != _asks) return;
    if (!await source.realtimeArrived()) return;
    if (_disposed || asked != _asks) return;
    try {
      final next = await _ask(origin, access);
      if (next != null) unawaited(_whenLive(next, origin, access));
    } on Object catch (e) {
      // The rows stay on the schedule until the next refresh.
      debugPrint('nearby transit: live times: $e');
    }
  }

  /// Shows page [index] of the row of [key], as a swipe settles on it, and remembers its
  /// headsign at its stop. The row stays where it is.
  void show(RouteKey key, int index) {
    final route = rows.routes.where((r) => r.key == key).firstOrNull;
    final page = rows.show(key, index);
    if (route == null || page == null) return;
    notifyListeners();
    unawaited(
      headsigns
          .remember(
            headsignStop(route, page),
            rememberedHeadsign(page),
            chosenAt: clock(),
          )
          .catchError((Object e) => debugPrint('nearby transit: $e')),
    );
  }

  /// Abandons any refresh, and the live times it waits for, as the list stops being
  /// shown. The rows stay, for when it shows again.
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
}
