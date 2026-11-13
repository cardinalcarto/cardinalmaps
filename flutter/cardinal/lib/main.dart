import 'dart:async';

import 'package:flutter/services.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart' show freeBytes, initRouting;
import 'package:sparkle/sparkle.dart';

import 'config.dart';
import 'directions/route_source.dart';
import 'geocoder/geocoder.dart';
import 'map/location.dart';
import 'map/map.dart';
import 'map/map_drawing.dart';
import 'map/overlays.dart';
import 'offline/downloads/geometry.dart';
import 'offline/offline_area.dart';
import 'offline/offline_scope.dart';
import 'routes.dart';
import 'store/store.dart';
import 'ui/theme.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  // Draw under the system bars; the sheet keeps its content clear of them.
  SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
  final store = Store.open();
  unawaited(store.prune());
  final map = SparkleMapController();
  final offline = OfflineAreas.open();
  runApp(
    StoreScope(
      store: store,
      child: GeocoderScope(
        geocoder: Geocoder(Uri.parse(geocoderUrl)),
        child: DirectionsScope(
          routes: OfflineRouteSource(
            offline.area,
            used: (shape) => unawaited(
              offline.downloads.then(
                (d) => d?.touchRoute([
                  for (final p in shape) GeoPoint(p.lat, p.lng),
                ]),
              ),
            ),
          ),
          transit: OfflineTransitSource(offline.area),
          location: const GeolocatorSource(),
          child: MapScope(
            controller: map,
            overlays: MapOverlays(),
            drawing: MapDrawing(),
            tiles: offline.tiles,
            child: OfflineScope(
              downloads: offline.downloads,
              freeBytes: () async {
                await initRouting();
                return freeBytes(path: await offlineFolder());
              },
              child: MainApp(router: appRouter(map)),
            ),
          ),
        ),
      ),
    ),
  );
}

class MainApp extends StatelessWidget {
  const MainApp({super.key, required this.router});

  final GoRouter router;

  @override
  Widget build(BuildContext context) {
    return MaterialApp.router(
      // Restores where the app was when the system ends it in the background.
      restorationScopeId: 'app',
      debugShowCheckedModeBanner: false,
      theme: appTheme(Brightness.light),
      darkTheme: appTheme(Brightness.dark),
      routerConfig: router,
    );
  }
}
