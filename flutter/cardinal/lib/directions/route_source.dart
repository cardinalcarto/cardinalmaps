import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../map/location.dart';
import '../offline/offline_area.dart';
import '../offline/realtime.dart';
import '../store/store.dart';
import 'transit_plan.dart';

/// A way of traveling, in the order the sheet's chips show them.
enum TravelMode {
  driving('auto', 'Driving', Icons.directions_car),

  /// Planned by [TransitSource] rather than routed.
  transit(null, 'Transit', Icons.directions_transit),
  walking('pedestrian', 'Walking', Icons.directions_walk),
  cycling('bicycle', 'Cycling', Icons.directions_bike);

  const TravelMode(this.costing, this.label, this.icon);

  /// The street router's name for it, or null for transit.
  final String? costing;
  final String label;
  final IconData icon;

  bool get available => true;

  /// Whether it's walking by wheelchair, when walking is by [wheelchair].
  bool byWheelchair({required bool wheelchair}) =>
      wheelchair && this == walking;

  /// What its chip says: Wheelchair for walking by [wheelchair].
  String labelFor({required bool wheelchair}) =>
      byWheelchair(wheelchair: wheelchair) ? 'Wheelchair' : label;

  IconData iconFor({required bool wheelchair}) =>
      byWheelchair(wheelchair: wheelchair) ? Icons.accessible : icon;
}

/// The way of traveling last chosen, which directions start with.
final lastTravelMode = Setting.choice(
  'directions.mode',
  TravelMode.values,
  TravelMode.driving,
);

/// How transit directions last got to and from stops, or null if that's never been
/// chosen.
final lastTransitAccess = Setting.optionalChoice(
  'directions.transitAccess',
  TransitAccess.values,
);

/// How transit gets to and from stops: as [remembered], or if nothing is, by
/// [wheelchair] or else on foot.
TransitAccess transitAccess(
  TransitAccess? remembered, {
  required bool wheelchair,
}) =>
    remembered ??
    (wheelchair ? TransitAccess.wheelchair : TransitAccess.walking);

/// The trip has an end the offline area's streets don't reach.
class OutsideOfflineArea implements Exception {
  const OutsideOfflineArea();
}

/// The app has no offline area to route in.
class NoOfflineArea implements Exception {
  const NoOfflineArea();
}

/// Where routes come from, behind what the app needs of it, so tests can fake it.
abstract class RouteSource {
  /// The routes [from] to [to], the best first, by wheelchair if [mode] is walking and
  /// [wheelchair] is set. Throws a [DirectionsError] if there are none,
  /// [OutsideOfflineArea] or [NoOfflineArea] if they can't be looked for, and
  /// [http.RequestAbortedException] if [abort] completes first.
  Future<List<DirectionsRoute>> routes({
    required TravelMode mode,
    required bool wheelchair,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  });
}

/// Where transit plans come from, so tests can fake it.
abstract class TransitSource {
  /// Gets ready to plan, if that takes a while, so the first plan doesn't wait as long.
  void prepare() {}

  /// Journeys [from] to [to] leaving in the next hour, getting to and from stops by
  /// [access]. Throws [TransitPlanError] if the plan fails, [OutsideOfflineArea] or
  /// [NoOfflineArea] if it can't be made, and [http.RequestAbortedException] if [abort]
  /// completes first.
  Future<TransitPlan> plan({
    required TransitAccess access,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  });
}

/// Why a transit plan, or another question to the transit router, failed.
class TransitPlanError implements Exception {
  const TransitPlanError(this.kind, this.message);

  /// The router's error kind, such as `no_street_nearby` or `timed_out`.
  final String kind;
  final String message;

  @override
  String toString() => 'TransitPlanError($kind: $message)';
}

/// Throws [http.RequestAbortedException] if [abort] completes before [work] does.
Future<T> abortable<T>(Future<T> work, Future<void>? abort) {
  if (abort == null) return work;
  return Future.any([
    work,
    abort.then<T>((_) => throw http.RequestAbortedException()),
  ]);
}

/// Where the offline area comes from: null until there is one.
typedef OfflineAreaSource = Future<OfflineArea?> Function();

/// The offline area, or [NoOfflineArea] if there's none.
Future<OfflineArea> needOfflineArea(OfflineAreaSource area) async =>
    await area() ?? (throw const NoOfflineArea());

Future<void> _checkCovers(OfflineArea area, LngLat from, LngLat to) async {
  if (!await area.covers(from.lat, from.lon) ||
      !await area.covers(to.lat, to.lon)) {
    throw const OutsideOfflineArea();
  }
}

/// [RouteSource] on the device, over the offline area's streets: Ferrostar makes the
/// request and reads the answer, as it would from a server.
class OfflineRouteSource implements RouteSource {
  OfflineRouteSource(this._area, {this.used});

  final OfflineAreaSource _area;

  /// Told the line of each route found, so the tiles it passes through count as used
  /// (`OfflineDownloads.touchRoute`).
  final void Function(List<DirectionsPoint> shape)? used;

  /// Ferrostar's adapters, by mode and whether it's by wheelchair.
  final _adapters = <(TravelMode, bool), DirectionsAdapter>{};

  /// How many routes to ask for besides the best.
  static const _alternates = 3;

  static DirectionsPoint _point(LngLat p) =>
      DirectionsPoint(lat: p.lat, lng: p.lon);

  @override
  Future<List<DirectionsRoute>> routes({
    required TravelMode mode,
    required bool wheelchair,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  }) => abortable(
    _routes(mode, mode.byWheelchair(wheelchair: wheelchair), from, to),
    abort,
  );

  Future<List<DirectionsRoute>> _routes(
    TravelMode mode,
    bool wheelchair,
    LngLat from,
    LngLat to,
  ) async {
    final area = await needOfflineArea(_area);
    await _checkCovers(area, from, to);

    // Ferrostar's adapter is in the routing library too, made before the router opens.
    await initRouting();
    final adapter = _adapters[(mode, wheelchair)] ??=
        DirectionsAdapter.valhalla(
          // Ferrostar wants an endpoint to make the request for; nothing is sent to it.
          endpointUrl: 'offline:route',
          costing: mode.costing!,
          alternates: _alternates,
          wheelchair: wheelchair,
        );
    final made = adapter.request(from: _point(from), to: _point(to));
    final watch = Stopwatch()..start();
    final response = await area.useRouter(
      (router) => router.route(body: made.body),
    );
    debugPrint(
      'cardinal-timing: street route ${mode.name} '
      '${watch.elapsedMilliseconds} ms (${response.status})',
    );
    final routes = await adapter.parse(body: response.body);
    if (routes.isNotEmpty) used?.call(routes.first.geometry);
    return routes;
  }
}

/// [TransitSource] on the device, over the offline area's timetables, with the latest
/// real-time data its agencies publish when there's a network.
class OfflineTransitSource implements TransitSource {
  OfflineTransitSource(this._area, {this.client});

  final OfflineAreaSource _area;

  /// What real-time messages are fetched with.
  final http.Client? client;

  /// The real-time feeds of the area they were made for.
  (OfflineArea, RealtimeFeeds)? _feeds;

  RealtimeFeeds _realtime(OfflineArea area) {
    if (_feeds case (final made, final feeds) when made == area) return feeds;
    final feeds = RealtimeFeeds(() => area.realtimeSources, client: client);
    _feeds = (area, feeds);
    return feeds;
  }

  @override
  void prepare() {
    unawaited(() async {
      try {
        final area = await _area();
        if (area == null) return;
        await Future.wait([area.transit, _realtime(area).prefetch()]);
      } on Object catch (e) {
        debugPrint('transit: $e');
      }
    }());
  }

  @override
  Future<TransitPlan> plan({
    required TransitAccess access,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  }) async {
    final cancel = TransitCancel();
    unawaited(abort?.then((_) => cancel.cancel()));
    return abortable(_plan(access, from, to, cancel), abort);
  }

  Future<TransitPlan> _plan(
    TransitAccess access,
    LngLat from,
    LngLat to,
    TransitCancel cancel,
  ) async {
    final area = await needOfflineArea(_area);
    await _checkCovers(area, from, to);
    // Real-time messages are fetched while the timetables open.
    final fetching = _realtime(area).fresh();
    final opening = area.transit;
    final feeds = await fetching;
    await opening;
    final request = utf8.encode(
      jsonEncode({
        'origin': {'lat': from.lat, 'lon': from.lon},
        'destination': {'lat': to.lat, 'lon': to.lon},
        'mode': access.mode,
      }),
    );
    final watch = Stopwatch()..start();
    final response = await area.useTransit(
      (transit) => transit.plan(request: request, feeds: feeds, cancel: cancel),
    );
    debugPrint(
      'cardinal-timing: transit plan ${access.mode} '
      '${watch.elapsedMilliseconds} ms (${response.status}, '
      '${feeds.length} new real-time messages)',
    );
    return TransitPlan.decode(transitAnswer(response));
  }
}

/// The body of the transit router's [response]: throws [http.RequestAbortedException]
/// if it was cancelled, and [TransitPlanError] if it failed.
List<int> transitAnswer(TransitResponse response) {
  if (response.status == 499) throw http.RequestAbortedException();
  if (response.status != 200) {
    final body = jsonDecode(utf8.decode(response.body));
    final error = body is Map<String, Object?> ? body : const {};
    throw TransitPlanError(
      error['kind'] as String? ?? 'failed',
      error['error'] as String? ?? 'status ${response.status}',
    );
  }
  return response.body;
}

/// Provides what directions need to the widgets below it.
class DirectionsScope extends InheritedWidget {
  const DirectionsScope({
    super.key,
    required this.routes,
    required this.transit,
    required this.location,
    required super.child,
  });

  final RouteSource routes;
  final TransitSource transit;
  final LocationSource location;

  static DirectionsScope of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<DirectionsScope>()!;

  @override
  bool updateShouldNotify(DirectionsScope oldWidget) =>
      routes != oldWidget.routes ||
      transit != oldWidget.transit ||
      location != oldWidget.location;
}
