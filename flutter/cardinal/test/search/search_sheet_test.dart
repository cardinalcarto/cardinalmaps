import 'dart:convert';

import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/map/map.dart';
import 'package:cardinal/place/place_page.dart';
import 'package:cardinal/search/search_sheet.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_map.dart';
import '../fakes/fake_place_store.dart';
import '../geocoder/geocoder_test.dart' show spaceNeedle;

/// The app's sheet pages over a blank body instead of the map, which needs Flutter GPU.
Future<Store> _pumpApp(WidgetTester tester, {List<Uri>? asked, FakeMap? map}) async {
  tester.view
    ..physicalSize = const Size(400, 800) * 2
    ..devicePixelRatio = 2;
  addTearDown(tester.view.reset);
  final store = Store.withoutDatabase(places: FakePlaceStore());
  final geocoder = Geocoder(
    Uri.parse('http://localhost:3000'),
    client: MockClient((request) async {
      asked?.add(request.url);
      return http.Response(
        jsonEncode({
          'query': {},
          'results': [spaceNeedle()],
        }),
        200,
      );
    }),
  );
  final router = GoRouter(
    routes: [
      ShellRoute(
        builder: (context, state, child) => Scaffold(
          resizeToAvoidBottomInset: false,
          body: SheetScaffold(body: const SizedBox.expand(), sheet: child),
        ),
        routes: [
          GoRoute(
            path: '/',
            pageBuilder: (context, state) =>
                SheetPage(key: state.pageKey, child: const SearchSheet()),
            routes: [
              GoRoute(
                path: 'place/:id',
                pageBuilder: (context, state) => SheetPage(
                  key: state.pageKey,
                  child: PlacePage(id: state.pathParameters['id']!),
                ),
              ),
            ],
          ),
        ],
      ),
    ],
  );
  addTearDown(router.dispose);
  await tester.pumpWidget(
    StoreScope(
      store: store,
      child: GeocoderScope(
        geocoder: geocoder,
        child: MapScope(
          controller: map ?? FakeMap(),
          child: MaterialApp.router(routerConfig: router),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return store;
}

SheetController _sheet(WidgetTester tester) =>
    SheetScaffold.controllerOf(tester.element(find.byType(SearchSheet)));

void main() {
  testWidgets('focusing the search bar pulls the sheet up', (tester) async {
    await _pumpApp(tester);
    expect(_sheet(tester).isAtMax, isFalse);
    await tester.tap(find.byType(SearchBar));
    await tester.pumpAndSettle();
    expect(_sheet(tester).isAtMax, isTrue);
  });

  testWidgets('typing searches as you type, once typing pauses', (tester) async {
    final asked = <Uri>[];
    await _pumpApp(tester, asked: asked);
    await tester.tap(find.byType(SearchBar));
    await tester.enterText(find.byType(TextField), 'space');
    await tester.pump(const Duration(milliseconds: 100));
    await tester.enterText(find.byType(TextField), 'space ne');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    expect(asked.map((u) => u.queryParameters['q']), ['space ne']);
    expect(asked.single.queryParameters, containsPair('prefix', 'true'));
    expect(asked.single.queryParameters, containsPair('typos', 'true'));
    expect(find.text('Space Needle'), findsOneWidget);
    expect(find.text('400 Broad Street, Seattle, WA 98109'), findsOneWidget);
  });

  testWidgets('a result opens its place, stored, and closing it comes back', (tester) async {
    final store = await _pumpApp(tester);
    await tester.tap(find.byType(SearchBar));
    await tester.enterText(find.byType(TextField), 'space ne');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Space Needle'));
    await tester.pumpAndSettle();

    expect(find.byType(PlacePage), findsOneWidget);
    expect(find.widgetWithText(PlacePage, 'Space Needle'), findsOneWidget);
    expect(store.places.cached('poi:97175')?.name, 'Space Needle');

    await tester.tap(find.byTooltip('Close'));
    await tester.pumpAndSettle();
    await tester.pumpAndSettle();
    expect(find.byType(PlacePage), findsNothing);
    // Back to the results as they were, without the keyboard.
    expect(find.text('Space Needle'), findsOneWidget);
    expect(FocusManager.instance.primaryFocus?.context?.widget, isNot(isA<EditableText>()));
    expect(tester.testTextInput.isVisible, isFalse);
  });

  group('closing a place partly pulled up', () {
    Future<SheetController> openPlace(WidgetTester tester) async {
      await _pumpApp(tester);
      await tester.tap(find.byType(SearchBar));
      await tester.enterText(find.byType(TextField), 'space ne');
      await tester.pump(const Duration(milliseconds: 300));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Space Needle'));
      await tester.pumpAndSettle();
      return SheetScaffold.controllerOf(tester.element(find.byType(PlacePage)));
    }

    Future<void> expectClosed(WidgetTester tester) async {
      await tester.pumpAndSettle();
      expect(find.byType(PlacePage), findsNothing);
      expect(find.byType(SearchSheet), findsOneWidget);
    }

    testWidgets('while it springs to an anchor', (tester) async {
      await openPlace(tester);
      final handle = tester.getTopLeft(find.byType(PlacePage)) + const Offset(200, -12);
      final gesture = await tester.startGesture(handle);
      for (var i = 0; i < 10; i++) {
        await gesture.moveBy(const Offset(0, -15));
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await tester.pump(const Duration(milliseconds: 16));
      await tester.tap(find.byTooltip('Close'), warnIfMissed: false);
      await expectClosed(tester);
    });
  });

  testWidgets('a place opened with the keyboard up rests at its header once it hides', (
    tester,
  ) async {
    await _pumpApp(tester);
    await tester.tap(find.byType(SearchBar));
    tester.view.viewInsets = const FakeViewPadding(bottom: 600);
    await tester.enterText(find.byType(TextField), 'space ne');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Space Needle'));
    // The keyboard slides away over the first frames of the transition.
    for (final inset in [500.0, 350.0, 200.0, 80.0, 0.0]) {
      tester.view.viewInsets = FakeViewPadding(bottom: inset);
      await tester.pump(const Duration(milliseconds: 40));
    }
    final sheet = SheetScaffold.controllerOf(tester.element(find.byType(PlacePage)));
    await tester.pumpAndSettle();
    expect(sheet.restingAt, sheet.anchors.first.anchor);
    expect(sheet.extent, moreOrLessEquals(sheet.anchors.first.extent));
  });

  group('the map lets go of a place as its page leaves', () {
    Future<void> sendBack(WidgetTester tester, String method, [double? progress]) async {
      await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
        'flutter/backgesture',
        const StandardMethodCodec().encodeMethodCall(
          MethodCall(method, {
            if (progress != null) ...{
              'touchOffset': [5.0, 300.0],
              'progress': progress,
              'swipeEdge': 0,
            },
          }),
        ),
        (_) {},
      );
      await tester.pump();
    }

    Future<FakeMap> openPlace(WidgetTester tester) async {
      final map = FakeMap();
      await _pumpApp(tester, map: map);
      await tester.tap(find.byType(SearchBar));
      await tester.enterText(find.byType(TextField), 'space ne');
      await tester.pump(const Duration(milliseconds: 300));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Space Needle'));
      await tester.pumpAndSettle();
      expect(map.subject, isTrue, reason: 'the place is the subject once it opens');
      expect(map.subjectsSet, 1, reason: 'once');
      return map;
    }

    testWidgets('and keeps it again if a back gesture is let go of', (tester) async {
      final map = await openPlace(tester);
      await sendBack(tester, 'startBackGesture', 0);
      await sendBack(tester, 'updateBackGestureProgress', 0.3);
      expect(map.subject, isFalse);
      await sendBack(tester, 'cancelBackGesture');
      await tester.pumpAndSettle();
      expect(find.byType(PlacePage), findsOneWidget);
      expect(map.subject, isTrue);
      expect(map.subjectsSet, 2);
    });

    testWidgets('for good once a back gesture goes back', (tester) async {
      final map = await openPlace(tester);
      await sendBack(tester, 'startBackGesture', 0);
      await sendBack(tester, 'updateBackGestureProgress', 0.3);
      await sendBack(tester, 'commitBackGesture');
      await tester.pumpAndSettle();
      expect(find.byType(PlacePage), findsNothing);
      expect(map.subject, isFalse);
      expect(map.subjectsSet, 1);
    });

    testWidgets('as soon as it closes', (tester) async {
      final map = await openPlace(tester);
      await tester.tap(find.byTooltip('Close'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 16));
      expect(map.subject, isFalse, reason: 'before the sheet grows back');
      await tester.pumpAndSettle();
      expect(map.subjectsSet, 1);
    });

    testWidgets('and not again if the map had been moved by hand', (tester) async {
      final map = await openPlace(tester);
      map.subject = false;
      await sendBack(tester, 'startBackGesture', 0);
      await sendBack(tester, 'updateBackGestureProgress', 0.3);
      await sendBack(tester, 'cancelBackGesture');
      await tester.pumpAndSettle();
      expect(map.subject, isFalse);
      expect(map.subjectsSet, 1);
    });
  });
}
