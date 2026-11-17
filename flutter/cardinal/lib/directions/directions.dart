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

/// A trip being planned: its ends, how you're traveling, and the routes for those.
///
/// | In | When | Then |
/// |---|---|---|
/// | either end missing | both are set | loading |
/// | anywhere | an end, the mode, or a recalculation changes the question | loading |
/// | walking | walking by wheelchair is turned on or off | loading |
/// | loading | routes arrive | showing routes, the best selected |
/// | loading | there are none, or asking failed | a [RouteProblem], no routes |
/// | anywhere | an end is cleared | no routes, no problem, nothing loading |
/// | showing routes | a route is selected | that route selected, with no request |
/// | showing journeys | a journey is selected | that journey selected, with no request |
///
/// Walking is by wheelchair while [wheelchair] is set.
///
/// Transit answers with a [TransitPlan] rather than routes, and how you get to and from
/// stops ([transitAccess]) is part of its question. A plan with no journeys isn't a
/// [RouteProblem]: the plan says why it has none.
///
/// Only the latest question's answer counts: asking again aborts the last request, and an
/// answer that arrives anyway is dropped. While loading, routes for the last question
/// stay, so the map and the sheet don't flash empty.
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

  TravelMode get mode => _mode;
  TravelMode _mode;

  /// Whether walking is by wheelchair.
  bool get wheelchair => _wheelchair;
  bool _wheelchair = false;

  /// How transit gets to and from stops.
  TransitAccess get transitAccess => _transitAccess;
  TransitAccess _transitAccess = TransitAccess.walking;

  /// The transit plan for the latest question to be answered, if it was for transit.
  TransitPlan? get plan => _plan;
  TransitPlan? _plan;

  /// The index in [plan]'s journeys of the selected one.
  int get selectedJourney => _selectedJourney;
  int _selectedJourney = 0;

  Journey? get journey => switch (_plan?.journeys) {
    final journeys? when _selectedJourney < journeys.length =>
      journeys[_selectedJourney],
    _ => null,
  };

  /// The routes for the latest question to be answered, the best first.
  List<DirectionsRoute> get routes => _routes;
  List<DirectionsRoute> _routes = const [];

  /// The index in [routes] of the selected one.
  int get selected => _selected;
  int _selected = 0;

  DirectionsRoute? get selectedRoute =>
      _selected < _routes.length ? _routes[_selected] : null;

  /// Whether an answer is on its way.
  bool get loading => _loading;
  bool _loading = false;

  RouteProblem? get problem => _problem;
  RouteProblem? _problem;

  /// Whether a position is being fetched for one of the ends.
  bool get locating => _locating > 0;
  int _locating = 0;

  /// Whether the start has been set, or cleared, by anything but [startFromMyLocation].
  bool _startChosen = false;

  /// How many questions have been asked; only the answer to the latest counts.
  int _asked = 0;
  Completer<void>? _abort;
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
    _ask();
  }

  /// Swaps the two ends of the trip.
  void swap() {
    final from = _from;
    _from = _to;
    _to = from;
    _startChosen = true;
    _ask();
  }

  /// Changes how you're traveling, if it can be routed.
  void setMode(TravelMode mode) {
    if (!mode.available || mode == _mode) return;
    _mode = mode;
    if (mode == TravelMode.transit) _transit.prepare();
    _ask();
  }

  /// Turns walking by wheelchair on or off, asking again if that's how you're traveling.
  void setWheelchair(bool wheelchair) {
    if (wheelchair == _wheelchair) return;
    _wheelchair = wheelchair;
    if (_mode == TravelMode.walking) {
      _ask();
    } else {
      notifyListeners();
    }
  }

  /// Changes how transit gets to and from stops.
  void setTransitAccess(TransitAccess access) {
    if (access == _transitAccess) return;
    _transitAccess = access;
    if (_mode == TravelMode.transit) _ask();
  }

  /// Selects one of [plan]'s journeys.
  void selectJourney(int index) {
    final count = _plan?.journeys.length ?? 0;
    if (index == _selectedJourney || index < 0 || index >= count) return;
    _selectedJourney = index;
    notifyListeners();
  }

  /// Selects one of [routes].
  void select(int index) {
    if (index == _selected || index < 0 || index >= _routes.length) return;
    _selected = index;
    notifyListeners();
  }

  /// Starts the trip from where you are, if you've allowed that and nothing else has been
  /// chosen as the start. Never asks for permission.
  Future<void> startFromMyLocation() async {
    final point = await _allowedPosition();
    if (_disposed || point == null || _startChosen) return;
    _from = RouteEndpoint.myLocation(point);
    _ask();
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

  /// Asks again, with a new position for an end that's where you are, if there is one.
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
    _ask();
  }

  void _ask() {
    _abort?.complete();
    _abort = null;
    final question = ++_asked;
    final from = _from, to = _to;
    _problem = null;
    if (from == null || to == null) {
      _routes = const [];
      _selected = 0;
      _plan = null;
      _selectedJourney = 0;
      _loading = false;
      notifyListeners();
      return;
    }
    _loading = true;
    notifyListeners();
    final abort = _abort = Completer<void>();
    unawaited(_answer(question, from, to, abort.future));
  }

  Future<void> _answer(
    int question,
    RouteEndpoint from,
    RouteEndpoint to,
    Future<void> abort,
  ) async {
    List<DirectionsRoute> routes = const [];
    TransitPlan? plan;
    RouteProblem? problem;
    try {
      if (_mode == TravelMode.transit) {
        plan = await _transit.plan(
          access: _transitAccess,
          from: from.point,
          to: to.point,
          abort: abort,
        );
      } else {
        routes = await _source.routes(
          mode: _mode,
          wheelchair: _wheelchair,
          from: from.point,
          to: to.point,
          abort: abort,
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
    if (_disposed || question != _asked) return;
    _abort = null;
    _routes = routes;
    _selected = 0;
    _plan = plan;
    _selectedJourney = 0;
    _problem = problem;
    _loading = false;
    notifyListeners();
  }

  void _changed() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _abort?.complete();
    _abort = null;
    super.dispose();
  }
}
