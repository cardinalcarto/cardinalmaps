import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';

import 'package:flutter/services.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart' show freeBytes, initRouting;
import 'package:sparkle/sparkle.dart';

import 'config.dart';
import 'directions/route_source.dart';
import 'geocoder/geocoder.dart';
import 'l10n/app_localizations.dart';
import 'map/location.dart';
import 'map/map.dart';
import 'map/map_drawing.dart';
import 'map/overlays.dart';
import 'navigation/road_matcher.dart';
import 'navigation/navigation_controller.dart';
import 'navigation/navigation_location.dart';
import 'navigation/navigation_scope.dart';
import 'navigation/speech.dart';
import 'offline/downloads/geometry.dart';
import 'offline/offline_area.dart';
import 'offline/offline_scope.dart';
import 'offline/realtime.dart';
import 'routes.dart';
import 'store/store.dart';
import 'transit/nearby_page.dart';
import 'transit/nearby_source.dart';
import 'ui/theme.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  // Draw under the system bars; the sheet keeps its content clear of them.
  SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
  final store = Store.open();
  unawaited(store.prune());
  final map = SparkleMapController();
  final offline = OfflineAreas.open();
  final routes = OfflineRouteSource(
    offline.area,
    settings: store.settings,
    used: (shape) => unawaited(
      offline.downloads.then(
        (d) => d?.touchRoute([for (final p in shape) GeoPoint(p.lat, p.lng)]),
      ),
    ),
  );
  late final NavigationController navigation;
  navigation = NavigationController(
    routes: routes,
    location: simulateNavigation
        ? SimulatedTripLocation(() => navigation.route!.navigable)
        : const GeolocatorNavigationSource(),
    // flutter_tts has no Linux engine.
    speaker: defaultTargetPlatform == TargetPlatform.linux
        ? const LogSpeaker()
        : PlatformSpeaker(),
    screen: const PlatformScreenWake(),
    matcher: OfflineRoadMatcher(offline.area),
    settings: store.settings,
  );
  // Directions and nearby transit share each area's real-time messages.
  final realtime = AreaRealtimeFeeds();
  final nearby = OfflineNearbySource(offline.area, realtime);
  runApp(
    StoreScope(
      store: store,
      child: GeocoderScope(
        geocoder: Geocoder(Uri.parse(geocoderUrl)),
        child: DirectionsScope(
          routes: routes,
          transit: OfflineTransitSource(offline.area, realtime),
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
              child: NavigationScope(
                controller: navigation,
                screen: NavigationScreen(),
                child: NearbyTransitScope(
                  source: nearby,
                  child: MainApp(router: appRouter(map)),
                ),
              ),
            ),
          ),
        ),
      ),
    ),
  );
  // Nearby transit's timetables open once the first frame is out, so the launch doesn't
  // wait for them, and the tab's first answer doesn't either. They open on the routing
  // library's threads.
  SchedulerBinding.instance.addPostFrameCallback((_) => nearby.prepare());
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
      // English only for now; other languages come as reviewed translations do.
      localizationsDelegates: AppLocalizations.localizationsDelegates,
      supportedLocales: AppLocalizations.supportedLocales,
    );
  }
}
