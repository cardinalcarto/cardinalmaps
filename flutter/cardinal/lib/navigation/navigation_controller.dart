import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:routing/routing.dart';

import '../directions/route_source.dart';
import '../l10n/lookup.dart';
import '../store/store.dart';
import 'road_matcher.dart';
import 'navigation_location.dart';
import 'speech.dart';
import 'trip_session.dart';

/// Where a trip stands, as [NavigationController] tracks it.
enum NavigationState {
  /// No trip.
  idle,

  /// Started, with no fix yet good enough to start from.
  acquiring,

  /// Following the route.
  onRoute,

  /// Away from the route, waiting to see if you come back before asking for a new one.
  offRoute,

  /// Asking for a new route, while the old one keeps following you.
  rerouting,

  /// At the destination, until you say you're done.
  arrived,
}

/// Why the last try at a new route failed.
enum RerouteFailure {
  /// The router found no way from where you are: off any road it knows, say.
  noRoute,

  /// Where you are is outside the offline area's streets.
  outsideOfflineArea,

  /// There's no offline area to route in.
  noOfflineArea,

  /// Anything else.
  failed;

  static RerouteFailure of(Object error) => switch (error) {
    DirectionsError(kind: DirectionsErrorKind.noRoute) => noRoute,
    OutsideOfflineArea() => outsideOfflineArea,
    NoOfflineArea() => noOfflineArea,
    _ => failed,
  };
}

/// How navigation travels for [TravelMode]: transit isn't navigated.
NavigationMode? navigationMode(TravelMode mode) => switch (mode) {
  TravelMode.driving => NavigationMode.driving,
  TravelMode.cycling => NavigationMode.cycling,
  TravelMode.walking => NavigationMode.walking,
  TravelMode.transit => null,
};

/// The worst accuracy a fix may have to start a trip from, in meters.
///
/// The session starts on the step nearest the first fix, so it must be good enough to
/// tell which road you're on; the first fixes after the location service starts are often
/// from cell towers and Wi-Fi, hundreds of meters off, and GPS replaces them within
/// seconds. 50 m tells a block's streets apart, and is what a phone with a view of the sky
/// gives in its first few seconds of GPS.
const acquiringAccuracy = 50.0;

/// How long you must stay off the route before a new one is asked for.
///
/// Ferrostar already says you're off only a fix after you are, and only for a fix within
/// its accuracy limit, so this only has to ride out a few seconds of fixes thrown off by
/// buildings, or a car pulled briefly into a lot beside the road: at 50 km/h, 3 s is 40 m,
/// short enough that the new route still starts near where you turned off.
const offRouteHold = Duration(seconds: 3);

/// The least time between asking for one new route and the next.
///
/// Each new route starts its announcements over, so routes asked for back to back, as
/// when weaving through a parking lot, would talk over each other; a new route also starts
/// from your heading, so one that's right rarely needs replacing within 10 s.
const minRerouteInterval = Duration(seconds: 10);

/// How long to wait before trying again after a new route couldn't be found, at first.
///
/// The offline router answers in well under a second, so a failure isn't a slow answer
/// but no answer from here: off any road, or outside the area. Waiting as long as between
/// reroutes lets you move somewhere it may find one.
const rerouteBackoff = Duration(seconds: 10);

/// The longest wait before trying again: each failure doubles the wait up to this, so a
/// trip that's stuck off the road tries once a minute rather than draining the battery.
const maxRerouteBackoff = Duration(seconds: 60);

/// Navigation along a route: takes fixes, follows the route with Ferrostar's session,
/// asks for a new route when you leave it, and speaks each announcement once.
///
/// It depends on no widgets, so the phone's screen, Android Auto and CarPlay can all show
/// the same trip; they listen to it, as a [ChangeNotifier].
///
/// ## States
///
/// | State | Leaves to | When |
/// |---|---|---|
/// | [NavigationState.idle] | acquiring | [start] |
/// | [NavigationState.acquiring] | on route | a fix within [acquiringAccuracy] starts the session |
/// | | idle | [end] |
/// | [NavigationState.onRoute] | off route | the session says you're off the route |
/// | | arrived | the session says the trip is complete |
/// | | idle | [end] |
/// | [NavigationState.offRoute] | on route | the session says you're back on the line (and, if that update also completes the trip, on to arrived) |
/// | | rerouting | still off after [offRouteHold], and at least [minRerouteInterval] since the last new route was asked for |
/// | | idle | [end] |
/// | [NavigationState.rerouting] | on route | a new route came, and the session follows it |
/// | | off route | no new route came: tried again after a backoff, from [rerouteBackoff] doubling to [maxRerouteBackoff], with [rerouteFailure] saying why |
/// | | idle | [end] |
/// | [NavigationState.arrived] | idle | [end], when you say you're done; never by itself |
///
/// While rerouting, fixes still go to the old route, so the banner and progress keep
/// up; the new route replaces it when it comes.
///
/// Leaving the route cuts off whatever is being said. Off the route and while rerouting,
/// the old route's announcements aren't spoken, and aren't spoken late either; speaking
/// picks up with the new route, or with the old one's next announcement once you're back
/// on it.
///
/// There's no state for having lost your location: [lastFixAt] says how stale it is,
/// and when fixes stop because location was turned off, permission was taken away or the
/// service failed, [locationProblem] says why, until a fix comes again ([resumeLocation]
/// listens again once it's fixed).
///
/// [end], from any state, goes to idle and stops the fixes (with the service that keeps
/// them coming with the screen off), speech, and keeping the screen on.
///
/// Fixes are handled one at a time, in order, since each session call must finish before
/// the next is made. A fix that comes while one is being handled waits; a newer one
/// replaces it, so a slow call never builds up a backlog of stale fixes.
///
/// Once the session has started, each fix is matched to the roads with the fixes of the
/// [MatchWindow] before it, and the session is given both: it follows the match where
/// the match is plausible, which lets it tell a parallel road from the route, and the fix
/// where it isn't (see `NavigationSession.updateLocation` in the routing library). A new
/// route starts from the session's [TripSession.rerouteOrigin], the latest match when
/// there was one.
class NavigationController extends ChangeNotifier {
  NavigationController({
    required this._routes,
    required this._location,
    required this._speaker,
    required this._screen,
    required this._matcher,
    this._startSession = RustTripSession.start,
  });

  final RouteSource _routes;
  final NavigationLocationSource _location;
  final Speaker _speaker;
  final ScreenWake _screen;
  final RoadMatcher _matcher;
  final TripSessionStarter _startSession;

  NavigationState get state => _state;
  NavigationState _state = NavigationState.idle;

  /// How the trip travels; null when idle.
  TravelMode? get mode => _mode;
  TravelMode? _mode;
  NavigationMode? _navigationMode;

  /// The route being followed: the one started with, or the latest new one. Null when
  /// idle.
  DirectionsRoute? get route => _route;
  DirectionsRoute? _route;

  /// [route]'s steps, which [NavUpdate.stepIndex] indexes.
  List<NavStep> get steps => _steps;
  List<NavStep> _steps = const [];

  /// The locale [route]'s announcements are in, and spoken in.
  String? get language => _language;
  String? _language;

  /// Where navigation stood after the latest fix; null until the session starts.
  NavUpdate? get update => _update;
  NavUpdate? _update;

  /// When the latest fix was taken, however poor; null until one comes.
  DateTime? get lastFixAt => _lastFixAt;
  DateTime? _lastFixAt;

  /// Why fixes stopped coming, until one comes again.
  LocationProblem? get locationProblem => _locationProblem;
  LocationProblem? _locationProblem;

  /// Why the last try at a new route failed, until one is found or you're back on the
  /// route.
  RerouteFailure? get rerouteFailure => _rerouteFailure;
  RerouteFailure? _rerouteFailure;

  /// What the last failed try at a new route threw, for logs.
  Object? get rerouteError => _rerouteError;
  Object? _rerouteError;

  /// Which trip this is: each [start] and [end] moves it on, so that work for an earlier
  /// trip that finishes late is dropped.
  int _trip = 0;
  TripSession? _session;
  StreamSubscription<NavLocation>? _fixes;
  NavLocation? _lastFix;
  NavLocation? _pending;
  bool _handling = false;

  /// The fixes the next one is matched with, every fix that came included, even those
  /// replaced before they were handled.
  final _window = MatchWindow();

  /// The end of the original route, which every new route goes to.
  LngLat? _destination;

  /// The utterances said, or passed over off the route, on this trip, by ID, so each is
  /// said at most once.
  final _spoken = <String>{};

  /// What [offRouteHold] or the backoff after a failure is timed with.
  Timer? _hold;

  /// Running from when a new route is asked for until [minRerouteInterval] after.
  Timer? _interval;

  /// Whether a hold ended while [_interval] was running, so a new route is asked for when
  /// it ends.
  bool _waitingForInterval = false;
  Duration _backoff = rerouteBackoff;

  /// The last of the session calls made so far, which the next waits for.
  Future<void> _calls = Future.value();

  /// Starts navigating [route], traveling by [mode], which can't be transit. Fixes start
  /// coming, the screen stays on, and the route's announcements are spoken as they're
  /// due.
  ///
  /// Only from idle: [end] the trip under way first.
  void start({required DirectionsRoute route, required TravelMode mode}) {
    if (_state != NavigationState.idle) {
      throw StateError('a trip is under way ($_state)');
    }
    final navigation = navigationMode(mode);
    if (navigation == null) {
      throw ArgumentError.value(mode, 'mode', "can't be navigated");
    }
    _trip++;
    _mode = mode;
    _navigationMode = navigation;
    _route = route;
    _steps = route.navigable.steps();
    _language = route.navigable.language();
    final end = route.geometry.last;
    _destination = LngLat(end.lng, end.lat);
    _state = NavigationState.acquiring;
    unawaited(_speaker.prepare(_language!));
    unawaited(_screen.keepOn());
    _listen();
    notifyListeners();
  }

  /// Ends the trip, from any state: stops the fixes, speech, and keeping the screen on.
  Future<void> end() async {
    if (_state == NavigationState.idle) return;
    final stopping = _stop();
    notifyListeners();
    await stopping;
  }

  /// Listens for fixes again, after [locationProblem] said why they stopped and it's been
  /// put right. [locationProblem] stays until a fix comes.
  Future<void> resumeLocation() async {
    if (_state == NavigationState.idle) return;
    final trip = _trip;
    final fixes = _fixes;
    _fixes = null;
    await fixes?.cancel();
    // Ended, or resumed again, while the old stream stopped.
    if (trip != _trip || _fixes != null) return;
    _listen();
  }

  void _listen() {
    final trip = _trip;
    _fixes = _location
        .fixes(_navigationMode!)
        .listen(
          _onFix,
          onError: (Object error) {
            if (trip != _trip) return;
            debugPrint('navigation: location: $error');
            _locationProblem = error is NavigationLocationError
                ? error.problem
                : LocationProblem.failed;
            notifyListeners();
          },
          onDone: () {
            if (trip != _trip) return;
            _fixes = null;
            _locationProblem ??= LocationProblem.failed;
            notifyListeners();
          },
        );
  }

  void _onFix(NavLocation fix) {
    if (_state == NavigationState.idle) return;
    _lastFix = fix;
    _window.add(fix);
    _lastFixAt = DateTime.fromMillisecondsSinceEpoch(fix.timestampMs.toInt());
    _locationProblem = null;
    _pending = fix;
    if (!_handling) unawaited(_handlePending(_trip));
  }

  Future<void> _handlePending(int trip) async {
    _handling = true;
    try {
      while (trip == _trip) {
        final fix = _pending;
        if (fix == null) break;
        _pending = null;
        try {
          await _handle(fix, trip);
        } on Object catch (e) {
          debugPrint('navigation: fix: $e');
        }
      }
    } finally {
      if (trip == _trip) _handling = false;
    }
  }

  Future<void> _handle(NavLocation fix, int trip) async {
    switch (_state) {
      case NavigationState.idle:
        return;
      case NavigationState.acquiring:
        if (fix.horizontalAccuracy > acquiringAccuracy) {
          notifyListeners();
          return;
        }
        final session = await _call(
          () => _startSession(_route!.navigable, _navigationMode!, fix),
        );
        if (trip != _trip) {
          unawaited(_call(() async => session.dispose()));
          return;
        }
        _session = session;
        _state = NavigationState.onRoute;
        _apply(session.current());
      case NavigationState.arrived:
        // The trip is over; the fix only keeps [lastFixAt] fresh.
        notifyListeners();
      case NavigationState.onRoute ||
          NavigationState.offRoute ||
          NavigationState.rerouting:
        final session = _session!;
        // [fix] is the newest to come, so the window ends with it.
        final matched = await _matcher.match(_navigationMode!, _window.fixes);
        if (trip != _trip) return;
        final update = await _call(() => session.updateLocation(fix, matched));
        if (trip != _trip) return;
        _apply(update);
    }
  }

  /// Makes [call] once the session calls before it are done.
  Future<T> _call<T>(Future<T> Function() call) {
    final result = _calls.then((_) => call());
    _calls = result.then<void>((_) {}, onError: (Object _) {});
    return result;
  }

  void _apply(NavUpdate update) {
    _update = update;
    switch (_state) {
      case NavigationState.onRoute:
        if (update.status == TripStatus.complete) {
          _arrive();
        } else if (update.deviation == NavDeviation.offRoute) {
          _leave();
        }
      case NavigationState.offRoute:
        if (update.deviation != NavDeviation.offRoute) {
          _backOn();
          if (update.status == TripStatus.complete) _arrive();
        }
      case NavigationState.rerouting ||
          NavigationState.acquiring ||
          NavigationState.arrived ||
          NavigationState.idle:
        // Rerouting waits for its answer whatever the old route says.
        break;
    }
    // After the state moves, so the update that takes you off the route is held to the
    // off-route rule, and the one that brings you back on is spoken.
    _speak(update.spokenInstruction);
    notifyListeners();
  }

  /// Says [spoken] if it hasn't been said on this trip, and only while on the route or
  /// arriving.
  ///
  /// Off the route, and while rerouting, the old route's announcements are about turns
  /// you're no longer heading for, so they're dropped rather than said. They're marked as
  /// said all the same, so one that came due while you were off isn't said late if you
  /// come back onto the line: by then it's about a point you may already be past.
  void _speak(NavSpokenInstruction? spoken) {
    if (spoken == null || !_spoken.add(spoken.utteranceId)) return;
    switch (_state) {
      case NavigationState.onRoute || NavigationState.arrived:
        unawaited(_speaker.speak(spoken.text));
      case NavigationState.offRoute ||
          NavigationState.rerouting ||
          NavigationState.acquiring ||
          NavigationState.idle:
        break;
    }
  }

  void _leave() {
    _state = NavigationState.offRoute;
    // Whatever is being said, or waiting to be, is about the route you just left: cut it
    // off rather than let it run on while you're heading somewhere else.
    unawaited(_speaker.stop());
    // Said once as you leave, not on each try for a new route, so a reroute that keeps
    // failing doesn't keep repeating it. Only in the route's own language: with no
    // translation for it, staying quiet beats another language in this voice.
    final rerouting = localizationsFor(_language!)?.rerouting;
    if (rerouting != null) unawaited(_speaker.speak(rerouting));
    _hold?.cancel();
    _hold = Timer(offRouteHold, _holdEnded);
  }

  void _backOn() {
    _state = NavigationState.onRoute;
    _hold?.cancel();
    _hold = null;
    _waitingForInterval = false;
    _rerouteFailure = null;
    _rerouteError = null;
    _backoff = rerouteBackoff;
  }

  void _arrive() {
    _state = NavigationState.arrived;
    _hold?.cancel();
    _hold = null;
    _waitingForInterval = false;
  }

  void _holdEnded() {
    _hold = null;
    if (_state != NavigationState.offRoute) return;
    if (_interval != null) {
      _waitingForInterval = true;
      return;
    }
    unawaited(_reroute(_trip));
  }

  void _intervalEnded() {
    _interval = null;
    if (!_waitingForInterval) return;
    _waitingForInterval = false;
    _holdEnded();
  }

  Future<void> _reroute(int trip) async {
    final session = _session!;
    final from = session.rerouteOrigin();
    _state = NavigationState.rerouting;
    _interval = Timer(minRerouteInterval, _intervalEnded);
    notifyListeners();
    try {
      final route = await _routes.reroute(
        mode: _mode!,
        from: LngLat(from.lng, from.lat),
        to: _destination!,
        request: (adapter) =>
            _call(() async => session.rerouteRequest(adapter)),
      );
      if (trip != _trip) return;
      // From where you are now, which may be past where the new route was asked from.
      final at = _lastFix!;
      final update = await _call(
        () => session.replaceRoute(route.navigable, at),
      );
      if (trip != _trip) return;
      _route = route;
      _steps = route.navigable.steps();
      // The same as the old route's, unless the device's language changed on the way.
      final language = route.navigable.language();
      if (language != _language) {
        _language = language;
        unawaited(_speaker.prepare(language));
      }
      _backOn();
      _apply(update);
    } on Object catch (e) {
      if (trip != _trip) return;
      debugPrint('navigation: reroute: $e');
      _rerouteFailure = RerouteFailure.of(e);
      _rerouteError = e;
      _state = NavigationState.offRoute;
      _hold = Timer(_backoff, _holdEnded);
      _backoff = Duration(
        microseconds: math.min(
          _backoff.inMicroseconds * 2,
          maxRerouteBackoff.inMicroseconds,
        ),
      );
      notifyListeners();
    }
  }

  /// Back to idle, letting go of everything the trip held.
  Future<void> _stop() {
    _trip++;
    _state = NavigationState.idle;
    _hold?.cancel();
    _hold = null;
    _interval?.cancel();
    _interval = null;
    _waitingForInterval = false;
    _backoff = rerouteBackoff;
    _pending = null;
    _handling = false;
    final session = _session;
    _session = null;
    if (session != null) unawaited(_call(() async => session.dispose()));
    final fixes = _fixes;
    _fixes = null;
    _mode = null;
    _navigationMode = null;
    _route = null;
    _steps = const [];
    _language = null;
    _update = null;
    _lastFix = null;
    _window.clear();
    _lastFixAt = null;
    _locationProblem = null;
    _rerouteFailure = null;
    _rerouteError = null;
    _destination = null;
    _spoken.clear();
    return Future.wait([
      if (fixes != null) fixes.cancel(),
      _speaker.stop(),
      _screen.release(),
    ]);
  }

  @override
  void dispose() {
    if (_state != NavigationState.idle) unawaited(_stop());
    super.dispose();
  }
}
