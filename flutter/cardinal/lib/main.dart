import 'dart:async';

import 'package:flutter/services.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import 'config.dart';
import 'geocoder/geocoder.dart';
import 'map/map.dart';
import 'routes.dart';
import 'store/store.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  // Draw under the system bars; the sheet keeps its content clear of them.
  SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
  final store = Store.open();
  unawaited(store.prune());
  final map = SparkleMapController();
  runApp(
    StoreScope(
      store: store,
      child: GeocoderScope(
        geocoder: Geocoder(Uri.parse(geocoderUrl)),
        child: MapScope(
          controller: map,
          child: MainApp(router: appRouter(map)),
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
      theme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: Colors.blueGrey)),
      darkTheme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.blueGrey, brightness: Brightness.dark),
      ),
      routerConfig: router,
    );
  }
}
