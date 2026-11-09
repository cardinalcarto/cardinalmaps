import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../map/location.dart';
import '../store/store.dart';

/// A way of traveling, in the order the sheet's chips show them.
enum TravelMode {
  driving('auto', 'Driving', Icons.directions_car),

  /// Shown, but there's no trip planning for it yet.
  transit(null, 'Transit', Icons.directions_transit),
  walking('pedestrian', 'Walking', Icons.directions_walk),
  cycling('bicycle', 'Cycling', Icons.directions_bike);

  const TravelMode(this.costing, this.label, this.icon);

  /// The router's name for it, or null if it can't be routed yet.
  final String? costing;
  final String label;
  final IconData icon;

  bool get available => costing != null;
}

/// The way of traveling last chosen, which directions start with.
final lastTravelMode = Setting.choice(
  'directions.mode',
  TravelMode.values,
  TravelMode.driving,
);

/// Where routes come from, behind what the app needs of it, so tests can fake it.
abstract class RouteSource {
  /// The routes [from] to [to], the best first. Throws a [DirectionsError] if there are
  /// none, and [http.RequestAbortedException] if [abort] completes first.
  Future<List<DirectionsRoute>> routes({
    required TravelMode mode,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  });
}

/// [RouteSource] from a Valhalla-compatible server, with Ferrostar's request and parser.
class ValhallaRouteSource implements RouteSource {
  ValhallaRouteSource(this.endpoint, {http.Client? client})
    : _client = client ?? http.Client();

  /// The server's route endpoint.
  final Uri endpoint;
  final http.Client _client;
  final _adapters = <TravelMode, DirectionsAdapter>{};

  /// How many routes to ask for besides the best.
  static const _alternates = 3;

  static DirectionsPoint _point(LngLat p) =>
      DirectionsPoint(lat: p.lat, lng: p.lon);

  @override
  Future<List<DirectionsRoute>> routes({
    required TravelMode mode,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  }) async {
    await initRouting();
    final adapter = _adapters[mode] ??= DirectionsAdapter.valhalla(
      endpointUrl: endpoint.toString(),
      costing: mode.costing!,
      alternates: _alternates,
    );
    final made = adapter.request(from: _point(from), to: _point(to));
    final request =
        http.AbortableRequest('POST', Uri.parse(made.url), abortTrigger: abort)
          ..headers.addAll(made.headers)
          ..bodyBytes = made.body;
    final response = await http.Response.fromStream(
      await _client.send(request),
    );
    return adapter.parse(body: response.bodyBytes);
  }
}

/// Provides what directions need to the widgets below it.
class DirectionsScope extends InheritedWidget {
  const DirectionsScope({
    super.key,
    required this.routes,
    required this.location,
    required super.child,
  });

  final RouteSource routes;
  final LocationSource location;

  static DirectionsScope of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<DirectionsScope>()!;

  @override
  bool updateShouldNotify(DirectionsScope oldWidget) =>
      routes != oldWidget.routes || location != oldWidget.location;
}
