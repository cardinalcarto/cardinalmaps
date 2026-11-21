import 'dart:async';

import 'package:geolocator/geolocator.dart' show LocationPermission;
import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../geocoder/address.dart';
import '../map/location.dart';
import '../offline/offline_area.dart' show NoOfflineTransit;
import '../store/store.dart';
import 'route_source.dart';
import 'transit_plan.dart';

/// One end of a trip: a place, or where you are.
@immutable
class RouteEndpoint {
  RouteEndpoint.place(Place this.place) : point = place.center;
  const RouteEndpoint.myLocation(this.point) : place = null;

  /// The place, or null for where you are.
  final Place? place;
  final LngLat point;

  bool get isMyLocation => place == null;

  String get label => place == null ? 'My Location' : placeTitle(place!);
}

/// The two ends of a trip.
enum RouteField { from, to }

/// Why there are no routes to show.
enum RouteProblem {
  /// There's no way between the two ends.
  noRoute,

  /// Asking for routes failed.
  failed,

  /// An end is outside the offline area.
  outsideArea,

  /// There's no offline area to route in.
  noOfflineArea,
}

/// What one way of traveling has for the trip: its routes, or for transit its plan,
/// which of them is selected, and whether they're still on their way.
@immutable
class ModeOptions {
  const ModeOptions({
    this.loading = false,
    this.problem,
    this.routes = const [],
    this.plan,
    this.selected = 0,
  });

  /// Whether an answer is on its way.
  final bool loading;

  /// Why there's nothing to show, if asking came to nothing.
  final RouteProblem? problem;

  /// The routes, as the router returned them, the fastest first. Empty for transit.
  final List<DirectionsRoute> routes;

  /// Transit's plan, which may have no journeys and says why. Null for the others.
  final TransitPlan? plan;

  /// The index of the selected one among [routes], or the plan's journeys.
  final int selected;

  DirectionsRoute? get route =>
      selected < routes.length ? routes[selected] : null;

  Journey? get journey => switch (plan?.journeys) {
    final journeys? when selected < journeys.length => journeys[selected],
    _ => null,
  };

  /// How many routes or journeys there are to choose from.
  int get count => plan?.journeys.length ?? routes.length;

  /// How long the quickest of them takes: the fastest route's duration, or for
  /// transit the time from [now] until the earliest arrival. Null with none.
  Duration? best(DateTime now) {
    if (plan case final plan?) {
      if (plan.journeys.isEmpty) return null;
      final arrival = plan.journeys
          .map((journey) => journey.arrival)
          .reduce((a, b) => a.isBefore(b) ? a : b);
      final left = arrival.difference(now);
      return left.isNegative ? Duration.zero : left;
    }
    if (routes.isEmpty) return null;
    final seconds = routes
        .map((route) => route.duration)
        .reduce((a, b) => a < b ? a : b);
    return Duration(milliseconds: (seconds * 1000).round());
  }

  ModeOptions _selecting(int index) => ModeOptions(
    loading: loading,
    problem: problem,
    routes: routes,
    plan: plan,
    selected: index,
  );
}

/// A trip being planned: its ends, and what every way of traveling has for it, with
/// the way that's showing ([mode]).
///
/// Every way is asked at once, each on its own, and each answer lands when it comes:
///
/// | In | When | Then |
/// |---|---|---|
/// | either end missing | both are set | every way loading |
/// | anywhere | an end changes, or the ends are swapped | every way loading, with nothing to show |
/// | anywhere | walking by wheelchair is turned on or off | walking loading |
/// | anywhere | how transit gets to stops changes | transit loading |
/// | a way loading | its routes or plan arrive | that way has them, the first selected |
/// | a way loading | there are none, or asking failed | that way has a [RouteProblem] |
/// | anywhere | an end is cleared | no way has anything, a problem, or is loading |
/// | anywhere | another way is chosen | that way showing, as it was left, with no request |
/// | showing routes | a route is selected | that route selected, with no request |
/// | showing journeys | a journey is selected | that journey selected, with no request |
///
/// [routes], [plan], [loading] and the like are those of the way that's showing;
/// [options] has any way's. A way keeps its selection until it's asked again.
///
/// Walking is by wheelchair while [wheelchair] is set.
///
/// Transit answers with a [TransitPlan] rather than routes, and how you get to and from
/// stops ([transitAccess]) is part of its question. A plan with no journeys isn't a
/// [RouteProblem]: the plan says why it has none.
///
/// Only the answer to a way's latest question counts: asking again aborts its last
/// request, and an answer that arrives anyway is dropped.
///
/// The trip starts from where you are if you've already allowed that, without asking,
/// and unless the start has been chosen some other way by the time your position arrives
/// (see [startFromMyLocation]).
class Directions extends ChangeNotifier {
  Directions({
    required RouteSource routes,
    required this._transit,
    required this._location,
    this._mode = TravelMode.driving,
  }) : _source = routes;

  final RouteSource _source;
  final TransitSource _transit;
  final LocationSource _location;

  RouteEndpoint? get from => _from;
  RouteEndpoint? _from;

  RouteEndpoint? get to => _to;
  RouteEndpoint? _to;

  /// The way of traveling that's showing.
  TravelMode get mode => _mode;
  TravelMode _mode;

  /// Whether walking is by wheelchair.
  bool get wheelchair => _wheelchair;
  bool _wheelchair = false;

  /// How transit gets to and from stops.
  TransitAccess get transitAccess => _transitAccess;
  TransitAccess _transitAccess = TransitAccess.walking;

  final Map<TravelMode, ModeOptions> _options = {};

  /// What traveling by [mode] has for the trip.
  ModeOptions options(TravelMode mode) => _options[mode] ?? const ModeOptions();

  ModeOptions get _showing => options(_mode);

  /// The transit plan, if transit is showing and has one.
  TransitPlan? get plan => _showing.plan;

  /// The index in [plan]'s journeys of the selected one.
  int get selectedJourney => _showing.plan == null ? 0 : _showing.selected;

  Journey? get journey => _showing.journey;

  /// The routes of the way that's showing, the best first.
  List<DirectionsRoute> get routes => _showing.routes;

  /// The index in [routes] of the selected one.
  int get selected => _showing.plan == null ? _showing.selected : 0;

  DirectionsRoute? get selectedRoute => _showing.route;

  /// Whether the answer for the way that's showing is on its way.
  bool get loading => _showing.loading;

  RouteProblem? get problem => _showing.problem;

  /// Whether a position is being fetched for one of the ends.
  bool get locating => _locating > 0;
  int _locating = 0;

  /// Whether the start has been set, or cleared, by anything but [startFromMyLocation].
  bool _startChosen = false;

  /// How many questions each way has been asked; only the answer to its latest counts.
  final Map<TravelMode, int> _asked = {};
  final Map<TravelMode, Completer<void>> _aborts = {};
  bool _disposed = false;

  RouteEndpoint? endpoint(RouteField field) => switch (field) {
    RouteField.from => _from,
    RouteField.to => _to,
  };

  /// Sets or, with null, clears an end of the trip.
  void setEndpoint(RouteField field, RouteEndpoint? endpoint) {
    switch (field) {
      case RouteField.from:
        _from = endpoint;
        _startChosen = true;
      case RouteField.to:
        _to = endpoint;
    }
    _askAll();
  }

  /// Swaps the two ends of the trip.
  void swap() {
    final from = _from;
    _from = _to;
    _to = from;
    _startChosen = true;
    _askAll();
  }

  /// Shows another way of traveling, if it can be routed. Its answer is already here
  /// or on its way.
  void setMode(TravelMode mode) {
    if (!mode.available || mode == _mode) return;
    _mode = mode;
    if (mode == TravelMode.transit) _transit.prepare();
    notifyListeners();
  }

  /// Turns walking by wheelchair on or off, and asks for walking again.
  void setWheelchair(bool wheelchair) {
    if (wheelchair == _wheelchair) return;
    _wheelchair = wheelchair;
    _ask(TravelMode.walking);
    notifyListeners();
  }

  /// Changes how transit gets to and from stops, and asks for transit again.
  void setTransitAccess(TransitAccess access) {
    if (access == _transitAccess) return;
    _transitAccess = access;
    _ask(TravelMode.transit);
    notifyListeners();
  }

  /// Selects one of [plan]'s journeys.
  void selectJourney(int index) {
    if (_showing.plan != null) _select(index);
  }

  /// Selects one of [routes].
  void select(int index) {
    if (_showing.plan == null) _select(index);
  }

  void _select(int index) {
    final showing = _showing;
    if (index == showing.selected || index < 0 || index >= showing.count) {
      return;
    }
    _options[_mode] = showing._selecting(index);
    notifyListeners();
  }

  /// Starts the trip from where you are, if you've allowed that and nothing else has been
  /// chosen as the start. Never asks for permission.
  Future<void> startFromMyLocation() async {
    final point = await _allowedPosition();
    if (_disposed || point == null || _startChosen) return;
    _from = RouteEndpoint.myLocation(point);
    _askAll();
  }

  Future<LngLat?> _allowedPosition() async {
    try {
      if (!locationAllowed(await _location.checkPermission()) ||
          !await _location.serviceEnabled()) {
        return null;
      }
      return await _position();
    } on Object {
      // A platform without a location service.
      return null;
    }
  }

  /// Sets [field] to where you are, asking for permission first if it hasn't been given.
  Future<LocateResult> useMyLocation(RouteField field) async {
    try {
      if (!await _location.serviceEnabled()) return LocateResult.serviceOff;
      var permission = await _location.checkPermission();
      if (permission == LocationPermission.denied) {
        permission = await _location.requestPermission();
      }
      if (permission == LocationPermission.deniedForever) {
        return LocateResult.deniedForever;
      }
      if (!locationAllowed(permission)) return LocateResult.denied;
      final point = await _position();
      if (point == null) return LocateResult.unavailable;
      if (!_disposed) setEndpoint(field, RouteEndpoint.myLocation(point));
      return LocateResult.located;
    } on Object {
      return LocateResult.unavailable;
    }
  }

  Future<LngLat?> _position() async {
    _locating++;
    _changed();
    try {
      final fix = await _location.current();
      return fix == null ? null : LngLat(fix.lng, fix.lat);
    } finally {
      _locating--;
      _changed();
    }
  }

  /// Asks every way again, with a new position for an end that's where you are, if
  /// there is one.
  Future<void> recalculate() async {
    if (_from?.isMyLocation ?? false) {
      final point = await _position();
      if (_disposed) return;
      if (point != null && (_from?.isMyLocation ?? false)) {
        _from = RouteEndpoint.myLocation(point);
      }
    }
    if (_to?.isMyLocation ?? false) {
      final point = await _position();
      if (_disposed) return;
      if (point != null && (_to?.isMyLocation ?? false)) {
        _to = RouteEndpoint.myLocation(point);
      }
    }
    _askAll();
  }

  /// Asks every way of traveling for the trip as it is now.
  void _askAll() {
    for (final mode in TravelMode.values) {
      _ask(mode);
    }
    notifyListeners();
  }

  /// Asks for the trip by [mode], dropping what it had. Listeners aren't told: the
  /// caller does that, once it has asked all it means to.
  void _ask(TravelMode mode) {
    _aborts.remove(mode)?.complete();
    final question = _asked[mode] = (_asked[mode] ?? 0) + 1;
    final from = _from, to = _to;
    if (from == null || to == null || !mode.available) {
      _options.remove(mode);
      return;
    }
    _options[mode] = const ModeOptions(loading: true);
    final abort = _aborts[mode] = Completer<void>();
    unawaited(_answer(mode, question, from, to, abort));
  }

  Future<void> _answer(
    TravelMode mode,
    int question,
    RouteEndpoint from,
    RouteEndpoint to,
    Completer<void> abort,
  ) async {
    List<DirectionsRoute> routes = const [];
    TransitPlan? plan;
    RouteProblem? problem;
    try {
      if (mode == TravelMode.transit) {
        plan = await _transit.plan(
          access: _transitAccess,
          from: from.point,
          to: to.point,
          abort: abort.future,
        );
      } else {
        routes = await _source.routes(
          mode: mode,
          wheelchair: _wheelchair,
          from: from.point,
          to: to.point,
          abort: abort.future,
        );
        if (routes.isEmpty) problem = RouteProblem.noRoute;
      }
    } on http.RequestAbortedException {
      return;
    } on DirectionsError catch (e) {
      debugPrint('directions: ${e.message}');
      problem = e.kind == DirectionsErrorKind.noRoute
          ? RouteProblem.noRoute
          : RouteProblem.failed;
    } on OutsideOfflineArea {
      problem = RouteProblem.outsideArea;
    } on NoOfflineArea {
      problem = RouteProblem.noOfflineArea;
    } on NoOfflineTransit {
      // An area without timetables plans no journeys.
      problem = RouteProblem.noRoute;
    } on TransitPlanError catch (e) {
      debugPrint('directions: $e');
      problem = e.kind == 'no_street_nearby'
          ? RouteProblem.noRoute
          : RouteProblem.failed;
    } on Object catch (e) {
      debugPrint('directions: $e');
      problem = RouteProblem.failed;
    }
    if (_disposed || question != _asked[mode]) return;
    if (_aborts[mode] == abort) _aborts.remove(mode);
    _options[mode] = ModeOptions(routes: routes, plan: plan, problem: problem);
    notifyListeners();
  }

  void _changed() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    for (final abort in _aborts.values) {
      abort.complete();
    }
    _aborts.clear();
    super.dispose();
  }
}
