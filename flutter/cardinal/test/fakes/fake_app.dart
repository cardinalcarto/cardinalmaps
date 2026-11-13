import 'dart:convert';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/home/navigation_bar.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/map/map.dart';
import 'package:cardinal/map/map_drawing.dart';
import 'package:cardinal/map/overlays.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/ui/glass.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';

import 'fake_map.dart';
import 'fake_location.dart';
import 'fake_place_store.dart';
import 'fake_routes.dart';
import 'fake_saved_store.dart';
import 'fake_transit.dart';

const appScreen = Size(400, 800);
const appNavigationBar = 24.0;
const appBarHeight = 56.0;

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
  LocationSource? locationSource,
  FakeMap? map,
  MapOverlays? overlays,
  MapDrawing? drawing,
  TargetPlatform? platform,
}) async {
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
        builder: (context, state, child) => Scaffold(
          resizeToAvoidBottomInset: false,
          body: SheetScaffold(
            body: const SizedBox.expand(),
            floating: const MapDrawingLayer(child: SizedBox.shrink()),
            bottomBar: const AppNavigationBar(),
            bottomBarHeight: appBarHeight,
            backdrop: sheetGlassOf(context),
            sheet: child,
          ),
        ),
        routes: sheetRoutes(),
      ),
    ],
  );
  addTearDown(router.dispose);
  await tester.pumpWidget(
    StoreScope(
      store: store ?? fakeStore(),
      child: GeocoderScope(
        geocoder: Geocoder(
          Uri.parse('http://localhost:3000'),
          client: MockClient(
            (_) async => http.Response(jsonEncode({'results': []}), 200),
          ),
        ),
        child: DirectionsScope(
          routes: routes ?? FakeRoutes(),
          transit: transit ?? FakeTransit(),
          location: locationSource ?? FakeLocation(),
          child: MapScope(
            controller: map ?? FakeMap(),
            overlays: overlays ?? MapOverlays(),
            drawing: drawing ?? MapDrawing(),
            child: MaterialApp.router(
              // The app's theme for [platform], or the test's default theme.
              theme: platform == null
                  ? null
                  : appTheme(Brightness.light, platform: platform),
              routerConfig: router,
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return router;
}
