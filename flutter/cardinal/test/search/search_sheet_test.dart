import 'dart:convert';

import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/map/map.dart';
import 'package:cardinal/map/overlays.dart';
import 'package:cardinal/place/place_page.dart';
import 'package:cardinal/routes.dart';
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
import '../fakes/fake_saved_store.dart';
import '../geocoder/geocoder_test.dart' show spaceNeedle;

/// The app's sheet pages over a blank body instead of the map, which needs Flutter GPU.
Future<Store> _pumpApp(
  WidgetTester tester, {
  List<Uri>? asked,
  FakeMap? map,
  Widget? body,
}) async {
  tester.view
    ..physicalSize = const Size(400, 800) * 2
    ..devicePixelRatio = 2;
  addTearDown(tester.view.reset);
  final places = FakePlaceStore();
  final store = Store.withoutDatabase(
    places: places,
    saved: FakeSavedStore(places),
  );
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
        observers: [sheetPages],
        builder: (context, state, child) => Scaffold(
          resizeToAvoidBottomInset: false,
          body: SheetScaffold(
            body: body ?? const SizedBox.expand(),
            sheet: child,
          ),
        ),
        routes: [
          GoRoute(
            path: '/',
            pageBuilder: (context, state) => SheetPage(
              key: state.pageKey,
              requestFocus: false,
              child: const SearchSheet(),
            ),
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
          overlays: MapOverlays(),
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
    final peek = _sheet(tester).extent;
    await tester.tap(find.byType(SearchBar));
    await tester.pumpAndSettle();
    expect(_sheet(tester).extent, greaterThan(peek));
    expect(_sheet(tester).isAtMax, isTrue);
  });

  testWidgets('typing searches as you type, once typing pauses', (
    tester,
  ) async {
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

  testWidgets('a result opens its place, stored, and closing it comes back', (
    tester,
  ) async {
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
    // Back to the results as they were, ready to search again.
    expect(find.text('Space Needle'), findsOneWidget);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'search');
    expect(_sheet(tester).isAtMax, isTrue);
  });

  testWidgets('a result closed with back comes back to searching too', (
    tester,
  ) async {
    await _pumpApp(tester);
    await tester.tap(find.byType(SearchBar));
    await tester.enterText(find.byType(TextField), 'space ne');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Space Needle'));
    await tester.pumpAndSettle();
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    await tester.pumpAndSettle();
    expect(find.byType(PlacePage), findsNothing);
    expect(find.text('Space Needle'), findsOneWidget);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'search');
    expect(_sheet(tester).isAtMax, isTrue);
  });

  testWidgets('back stops searching, keeping the query for next time', (
    tester,
  ) async {
    await _pumpApp(tester);
    final peek = _sheet(tester).extent;
    await tester.tap(find.byType(SearchBar));
    await tester.enterText(find.byType(TextField), 'space ne');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(_sheet(tester).extent, moreOrLessEquals(peek));
    expect(FocusManager.instance.primaryFocus?.debugLabel, isNot('search'));
    expect(find.text('space ne'), findsOneWidget);

    await tester.tap(find.byType(SearchBar));
    await tester.pumpAndSettle();
    expect(_sheet(tester).isAtMax, isTrue);
    expect(find.text('Space Needle'), findsOneWidget);
  });

  for (final close in ['X', 'back']) {
    for (final collapse in ['dragging down', 'back']) {
      testWidgets(
        'a place tapped on the map after searching, then $collapse, closes with $close to the sheet as it was',
        (tester) async {
          // The map takes focus when it's pressed, as SparkleMap does.
          final mapFocus = FocusNode(debugLabel: 'map');
          addTearDown(mapFocus.dispose);
          final store = await _pumpApp(
            tester,
            body: Focus(
              focusNode: mapFocus,
              autofocus: true,
              child: const SizedBox.expand(),
            ),
          );
          final peek = _sheet(tester).extent;
          await tester.tap(find.byType(SearchBar));
          await tester.pumpAndSettle();
          if (collapse == 'back') {
            await tester.binding.handlePopRoute();
          } else {
            final gesture = await tester.startGesture(
              Offset(200, 800 - _sheet(tester).extent + 12),
            );
            for (var i = 0; i < 20; i++) {
              await gesture.moveBy(const Offset(0, 40));
              await tester.pump(const Duration(milliseconds: 16));
            }
            await gesture.up();
          }
          await tester.pumpAndSettle();
          expect(_sheet(tester).extent, moreOrLessEquals(peek));
          // Dragged down, it's still searching, keyboard and all; back stops.
          final searching = collapse == 'dragging down';
          expect(
            FocusManager.instance.primaryFocus?.debugLabel == 'search',
            searching,
          );

          final router = GoRouter.of(tester.element(find.byType(SearchSheet)));
          await store.places.put(spacePlace);
          await store.places.put(
            const Place(
              id: 'poi:2',
              kind: 'poi',
              name: 'Other',
              center: LngLat(-122.34, 47.61),
              tags: {},
            ),
          );
          for (final id in [spacePlace.id, 'poi:2']) {
            mapFocus.requestFocus();
            openPlaceFromMap(router, id);
            await tester.pumpAndSettle();
          }
          if (close == 'X') {
            await tester.tap(find.byTooltip('Close'));
          } else {
            await tester.binding.handlePopRoute();
          }
          // Every frame of the way back, the sheet never rises above the bar.
          for (var i = 0; i < 60; i++) {
            await tester.pump(const Duration(milliseconds: 16));
            expect(
              _sheet(tester).extent,
              lessThanOrEqualTo(800 * 0.6),
              reason: 'frame $i',
            );
          }
          await tester.pumpAndSettle();
          expect(find.byType(PlacePage), findsNothing);
          expect(_sheet(tester).extent, moreOrLessEquals(peek));
          // As it was when the card opened: the map had taken focus, so no keyboard,
          // still searching if it was.
          expect(
            FocusManager.instance.primaryFocus?.debugLabel,
            isNot('search'),
          );
          await tester.tap(find.byType(SearchBar));
          await tester.pumpAndSettle();
          expect(_sheet(tester).isAtMax, isTrue);
        },
      );
    }
  }

  testWidgets('clearing a query kept from before only clears it', (
    tester,
  ) async {
    await _pumpApp(tester);
    final peek = _sheet(tester).extent;
    await tester.tap(find.byType(SearchBar));
    await tester.enterText(find.byType(TextField), 'space ne');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('Clear'));
    await tester.pumpAndSettle();
    expect(find.text('space ne'), findsNothing);
    expect(FocusManager.instance.primaryFocus?.debugLabel, isNot('search'));
    expect(_sheet(tester).extent, moreOrLessEquals(peek));
  });

  testWidgets('dragged down while searching, the keyboard stays', (
    tester,
  ) async {
    await _pumpApp(tester);
    final peek = _sheet(tester).extent;
    await tester.tap(find.byType(SearchBar));
    await tester.pumpAndSettle();
    await _dragDown(tester);
    expect(_sheet(tester).extent, moreOrLessEquals(peek));
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'search');
  });

  testWidgets(
    'a page over the sheet collapsed with the keyboard up closes back to just that',
    (tester) async {
      final store = await _pumpApp(tester);
      final peek = _sheet(tester).extent;
      await tester.tap(find.byType(SearchBar));
      await tester.pumpAndSettle();
      await _dragDown(tester);
      // Opened without anything taking focus first.
      await store.places.put(spacePlace);
      GoRouter.of(tester.element(find.byType(SearchSheet)))
          .push(placeLocation(spacePlace.id));
      await tester.pumpAndSettle();
      expect(FocusManager.instance.primaryFocus?.debugLabel, isNot('search'));
      await tester.binding.handlePopRoute();
      for (var i = 0; i < 60; i++) {
        await tester.pump(const Duration(milliseconds: 16));
        expect(
          _sheet(tester).extent,
          lessThanOrEqualTo(800 * 0.6),
          reason: 'frame $i',
        );
      }
      await tester.pumpAndSettle();
      expect(_sheet(tester).extent, moreOrLessEquals(peek));
      expect(FocusManager.instance.primaryFocus?.debugLabel, 'search');
    },
  );

  testWidgets('a page over the sheet while browsing closes back to browsing', (
    tester,
  ) async {
    final store = await _pumpApp(tester);
    final peek = _sheet(tester).extent;
    await store.places.put(spacePlace);
    openPlaceFromMap(
      GoRouter.of(tester.element(find.byType(SearchSheet))),
      spacePlace.id,
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Close'));
    await tester.pumpAndSettle();
    expect(_sheet(tester).extent, moreOrLessEquals(peek));
    expect(FocusManager.instance.primaryFocus?.debugLabel, isNot('search'));
  });

  testWidgets(
    'a result closed with a back gesture comes back with the keyboard up',
    (tester) async {
      await _pumpApp(tester);
      await tester.tap(find.byType(SearchBar));
      await tester.enterText(find.byType(TextField), 'space ne');
      await tester.pump(const Duration(milliseconds: 300));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Space Needle'));
      await tester.pumpAndSettle();
      expect(tester.testTextInput.isVisible, isFalse);

      await _sendBack(tester, 'startBackGesture', 0);
      await _sendBack(tester, 'updateBackGestureProgress', 0.5);
      await _sendBack(tester, 'commitBackGesture');
      await tester.pumpAndSettle();
      expect(find.byType(PlacePage), findsNothing);
      expect(FocusManager.instance.primaryFocus?.debugLabel, 'search');
      expect(
        tester.testTextInput.isVisible,
        isTrue,
        reason: 'the keyboard is asked for',
      );
      expect(_sheet(tester).isAtMax, isTrue);
    },
  );

  testWidgets('clearing the search keeps it ready to type', (tester) async {
    await _pumpApp(tester);
    await tester.tap(find.byType(SearchBar));
    await tester.enterText(find.byType(TextField), 'space ne');
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    FocusManager.instance.primaryFocus?.unfocus();
    await tester.pumpAndSettle();
    expect(_sheet(tester).isAtMax, isTrue, reason: 'it has results');

    await tester.tap(find.byTooltip('Clear'));
    await tester.pumpAndSettle();
    expect(find.text('Space Needle'), findsNothing);
    expect(FocusManager.instance.primaryFocus?.debugLabel, 'search');
    expect(_sheet(tester).isAtMax, isTrue);
  });

  testWidgets('the sheet only peeks until searching, and never rests between', (
    tester,
  ) async {
    await _pumpApp(tester);
    final sheet = _sheet(tester);
    final peek = sheet.extent;
    // Nothing to pull up to.
    await tester.tapAt(Offset(200, 800 - peek + 4));
    await tester.pumpAndSettle();
    expect(sheet.extent, moreOrLessEquals(peek));

    await tester.tap(find.byType(SearchBar));
    await tester.pumpAndSettle();
    expect(sheet.isAtMax, isTrue);

    // Pulled partway down, it settles at one end or the other.
    final gesture = await tester.startGesture(
      Offset(200, 800 - sheet.extent + 12),
    );
    await gesture.moveBy(const Offset(0, 300));
    await tester.pump(const Duration(milliseconds: 16));
    await gesture.up();
    await tester.pumpAndSettle();
    expect(
      sheet.isAtMax || (sheet.extent - peek).abs() < 1,
      isTrue,
      reason: '${sheet.extent}',
    );

    // Back stops searching, and it goes back to peeking.
    if (!sheet.isAtMax) {
      await tester.tap(find.byType(SearchBar));
      await tester.pumpAndSettle();
    }
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(sheet.extent, moreOrLessEquals(peek));
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
      final handle =
          tester.getTopLeft(find.byType(PlacePage)) + const Offset(200, -12);
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

  testWidgets(
    'a place opened with the keyboard up rests at its header once it hides',
    (tester) async {
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
      final sheet = SheetScaffold.controllerOf(
        tester.element(find.byType(PlacePage)),
      );
      await tester.pumpAndSettle();
      expect(sheet.restingAt, sheet.anchors.first.anchor);
      expect(sheet.extent, moreOrLessEquals(sheet.anchors.first.extent));
    },
  );

  group('the map lets go of a place as its page leaves', () {
    Future<void> sendBack(
      WidgetTester tester,
      String method, [
      double? progress,
    ]) async {
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
      expect(
        map.subject,
        isTrue,
        reason: 'the place is the subject once it opens',
      );
      expect(map.subjectsSet, 1, reason: 'once');
      return map;
    }

    testWidgets('and keeps it again if a back gesture is let go of', (
      tester,
    ) async {
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

    testWidgets('and not again if the map had been moved by hand', (
      tester,
    ) async {
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

/// The Space Needle as a map pin knows it.
const spacePlace = Place(
  id: 'poi:97175',
  kind: 'poi',
  name: 'Space Needle',
  center: LngLat(-122.3493, 47.6205),
  tags: {'tourism': 'attraction'},
);

/// Drags the sheet down to the bar.
Future<void> _dragDown(WidgetTester tester) async {
  final gesture = await tester.startGesture(
    Offset(200, 800 - _sheet(tester).extent + 12),
  );
  for (var i = 0; i < 20; i++) {
    await gesture.moveBy(const Offset(0, 40));
    await tester.pump(const Duration(milliseconds: 16));
  }
  await gesture.up();
  await tester.pumpAndSettle();
}

/// Sends a predictive back event, as Android's engine does.
Future<void> _sendBack(
  WidgetTester tester,
  String method, [
  double? progress,
]) async {
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
