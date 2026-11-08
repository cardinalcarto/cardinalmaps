import 'dart:convert';

import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/home/navigation_bar.dart';
import 'package:cardinal/map/map.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';

import 'fake_map.dart';
import 'fake_place_store.dart';
import 'fake_saved_store.dart';

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
            bottomBar: const AppNavigationBar(),
            bottomBarHeight: appBarHeight,
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
        child: MapScope(
          controller: FakeMap(),
          child: MaterialApp.router(routerConfig: router),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return router;
}
