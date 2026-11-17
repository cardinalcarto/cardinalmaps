import 'dart:convert';
import 'dart:typed_data';

import 'package:cardinal/directions/transit_plan.dart' show TransitRoute;

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/home/navigation_bar.dart';
import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/map/map.dart';
import 'package:cardinal/map/map_drawing.dart';
import 'package:cardinal/map/overlays.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/navigation/navigation_page.dart';
import 'package:cardinal/navigation/navigation_scope.dart';
import 'package:cardinal/offline/downloads/engine.dart';
import 'package:cardinal/offline/geocoder_models.dart';
import 'package:cardinal/offline/offline_scope.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/transit/nearby_page.dart';
import 'package:cardinal/transit/nearby_source.dart';
import 'package:cardinal/transit/vehicle_markers.dart';
import 'package:cardinal/ui/glass.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart' show SpriteImageInput;

import 'fake_geocoder.dart';
import 'fake_map.dart';
import 'fake_navigation.dart';
import 'fake_location.dart';
import 'fake_nearby.dart';
import 'fake_place_store.dart';
import 'fake_routes.dart';
import 'fake_saved_store.dart';
import 'fake_transit.dart';

const appScreen = Size(400, 800);
const appNavigationBar = 24.0;
const appBarHeight = 56.0;

/// Vehicle markers without drawing them, which a widget test can't wait for: a pixel
/// each, by the names the device's would have.
Future<List<SpriteImageInput>> fakeVehicleMarkers(TransitRoute? route) async =>
    [
      for (final name in [vehicleMarkerName(route), vehicleHeadingName(route)])
        SpriteImageInput(
          name: name,
          pixels: Uint8List(4),
          width: 1,
          height: 1,
          pixelRatio: 1,
        ),
    ];

/// A store of fakes.
Store fakeStore() {
  final places = FakePlaceStore();
  return Store.withoutDatabase(places: places, saved: FakeSavedStore(places));
}

/// The app's real sheet pages and navigation bar, over a blank body, starting at
/// [location].
Future<GoRouter> pumpFakeApp(
  WidgetTester tester, {
  Store? store,
  String location = '/',
  RouteSource? routes,
  TransitSource? transit,
  NearbyTransitSource? nearby,
  DateTime Function()? clock,
  VehicleMarkerPainter? vehicleMarkers,
  LocationSource? locationSource,
  FakeMap? map,
  MapOverlays? overlays,
  MapDrawing? drawing,
  Future<OfflineDownloads?>? downloads,
  Future<int> Function()? freeBytes,
  TargetPlatform? platform,
  NavigationController? navigation,
  Geocoder? online,
  Geocoder? offline,
  bool offerModels = false,
  bool locationButton = false,
}) async {
  final here = locationSource ?? FakeLocation();
  tester.view
    ..physicalSize = appScreen * 2
    ..devicePixelRatio = 2
    ..padding = const FakeViewPadding(bottom: appNavigationBar * 2)
    ..viewPadding = const FakeViewPadding(bottom: appNavigationBar * 2);
  addTearDown(tester.view.reset);
  final router = GoRouter(
    initialLocation: location,
    routes: [
      ShellRoute(
        observers: [sheetPages],
        builder: (context, state, child) {
          final scaffold = Scaffold(
            resizeToAvoidBottomInset: false,
            body: SheetScaffold(
              body: const SizedBox.expand(),
              floating: NavigationLayer(
                child: MapDrawingLayer(
                  // With its offer of your location at startup, if the test wants it.
                  child: locationButton
                      ? MyLocationButton(source: here)
                      : const SizedBox.shrink(),
                ),
              ),
              bottomBar: const AppNavigationBar(),
              bottomBarHeight: appBarHeight,
              backdrop: sheetGlassOf(context),
              sheet: child,
            ),
          );
          // As the app offers them, if the test wants the offer.
          return offerModels ? GeocoderModelsPrompt(child: scaffold) : scaffold;
        },
        routes: sheetRoutes(),
      ),
    ],
  );
  addTearDown(router.dispose);
  await tester.pumpWidget(
    StoreScope(
      store: store ?? fakeStore(),
      child: GeocoderScope(
        online:
            online ??
            OnlineGeocoder(
              Uri.parse('http://localhost:3000'),
              client: MockClient(
                (_) async => http.Response(jsonEncode({'results': []}), 200),
              ),
            ),
        offline: offline ?? FakeGeocoder(),
        child: DirectionsScope(
          routes: routes ?? FakeRoutes(),
          transit: transit ?? FakeTransit(),
          location: here,
          child: MapScope(
            controller: map ?? FakeMap(),
            overlays: overlays ?? MapOverlays(),
            drawing: drawing ?? MapDrawing(),
            child: OfflineScope(
              // A build without downloads, unless the test gives some.
              downloads: downloads ?? Future.value(),
              freeBytes: freeBytes ?? () async => 64 * 1000 * 1000 * 1000,
              child: NavigationScope(
                controller: navigation ?? fakeNavigationController(),
                screen: NavigationScreen(),
                child: NearbyTransitScope(
                  source: nearby ?? FakeNearbySource(),
                  clock: clock ?? DateTime.now,
                  vehicleMarkers: vehicleMarkers ?? fakeVehicleMarkers,
                  child: MaterialApp.router(
                    debugShowCheckedModeBanner: false,
                    // The app's theme for [platform], or the test's default theme.
                    theme: platform == null
                        ? null
                        : appTheme(Brightness.light, platform: platform),
                    routerConfig: router,
                    // The app's strings, as `main.dart` gives them.
                    localizationsDelegates:
                        AppLocalizations.localizationsDelegates,
                    supportedLocales: AppLocalizations.supportedLocales,
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return router;
}
