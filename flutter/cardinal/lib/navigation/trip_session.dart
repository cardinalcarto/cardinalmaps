import 'package:routing/routing.dart';

/// What the navigation controller needs of Ferrostar's [NavigationSession], so tests can
/// fake it without the routing library.
///
/// The calls that change the session are async and must not overlap: each is awaited
/// before the next is made, since two in flight may run in either order.
abstract class TripSession {
  /// Where navigation stands now.
  NavUpdate current();

  /// Moves navigation on to [location], which was [matched] to the roads there if it
  /// could be. The session decides whether to follow the match or the fix.
  Future<NavUpdate> updateLocation(
    NavLocation location,
    MatchedPosition? matched,
  );

  /// Where a new route would start from now: the latest fix's matched position, or the
  /// fix as it came.
  NavLocation rerouteOrigin();

  /// The request for a new route from [rerouteOrigin] through the stops not yet passed,
  /// made with [adapter]. Throws a [DirectionsError] once the trip is over.
  DirectionsRequest rerouteRequest(DirectionsAdapter adapter);

  /// Navigates [route] instead, from [location].
  Future<NavUpdate> replaceRoute(NavigableRoute route, NavLocation location);

  /// Frees the session. Nothing else may be called after.
  void dispose();
}

/// Starts a [TripSession] along [route] from [location], traveling by [mode].
typedef TripSessionStarter = Future<TripSession> Function(
  NavigableRoute route,
  NavigationMode mode,
  NavLocation location,
);

/// [TripSession] on Ferrostar's navigation controller, in the routing library.
class RustTripSession implements TripSession {
  RustTripSession._(this._session);

  final NavigationSession _session;

  /// A [TripSessionStarter] for the routing library, which must be loaded
  /// ([initRouting]).
  static Future<TripSession> start(
    NavigableRoute route,
    NavigationMode mode,
    NavLocation location,
  ) async => RustTripSession._(
    await NavigationSession.start(route: route, mode: mode, location: location),
  );

  @override
  NavUpdate current() => _session.current();

  @override
  Future<NavUpdate> updateLocation(
    NavLocation location,
    MatchedPosition? matched,
  ) => _session.updateLocation(location: location, matched: matched);

  @override
  NavLocation rerouteOrigin() => _session.rerouteOrigin();

  @override
  DirectionsRequest rerouteRequest(DirectionsAdapter adapter) =>
      _session.rerouteRequest(adapter: adapter);

  @override
  Future<NavUpdate> replaceRoute(NavigableRoute route, NavLocation location) =>
      _session.replaceRoute(route: route, location: location);

  @override
  void dispose() => _session.dispose();
}
