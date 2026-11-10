import 'dart:async';

import 'package:flutter/services.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import 'config.dart';
import 'directions/route_source.dart';
import 'geocoder/geocoder.dart';
import 'map/location.dart';
import 'map/map.dart';
import 'map/overlays.dart';
import 'offline/offline_area.dart';
import 'routes.dart';
import 'store/store.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  // Draw under the system bars; the sheet keeps its content clear of them.
  SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
  final store = Store.open();
  unawaited(store.prune());
  final map = SparkleMapController();
  final offlineArea = configuredOfflineArea();
  runApp(
    StoreScope(
      store: store,
      child: GeocoderScope(
        geocoder: Geocoder(Uri.parse(geocoderUrl)),
        child: DirectionsScope(
          routes: OfflineRouteSource(offlineArea),
          transit: OfflineTransitSource(offlineArea),
          location: const GeolocatorSource(),
          child: MapScope(
            controller: map,
            overlays: MapOverlays(),
            child: MainApp(router: appRouter(map)),
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
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.blueGrey),
      ),
      darkTheme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: Colors.blueGrey,
          brightness: Brightness.dark,
        ),
      ),
      routerConfig: router,
    );
  }
}
