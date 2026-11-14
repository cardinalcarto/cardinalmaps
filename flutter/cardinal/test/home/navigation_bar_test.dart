import 'dart:async';

import 'package:cardinal/home/navigation_bar.dart';
import 'package:cardinal/home/stub_page.dart';
import 'package:cardinal/offline/offline_page.dart';
import 'package:cardinal/transit/nearby_page.dart';
import 'package:cardinal/place/place_page.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/saved/saved_page.dart';
import 'package:cardinal/search/search_sheet.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';

Store _store() {
  final store = fakeStore();
  unawaited(
    store.places.put(
      const Place(id: 'poi:1', kind: 'poi', name: 'A', center: LngLat(0, 0)),
    ),
  );
  return store;
}

double _barTop(WidgetTester tester) =>
    tester.getTopLeft(find.byType(AppNavigationBar)).dy;

Finder _tab(String tooltip) => find.byTooltip(tooltip);

void main() {
  testWidgets('sits over the navigation bar, the sheet resting above it', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    expect(_barTop(tester), appScreen.height - appNavigationBar - appBarHeight);
    final search = tester.getBottomLeft(find.byType(SearchBar)).dy;
    expect(search, lessThanOrEqualTo(_barTop(tester)));
  });

  testWidgets('has the five pages, in order, with Search in the middle', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    final xs = [
      for (final tip in [
        'Saved places',
        'Nearby places',
        'Search',
        'Nearby transit',
        'Offline maps',
      ])
        tester.getCenter(_tab(tip)).dx,
    ];
    expect(xs, orderedEquals([...xs]..sort()));
    expect(xs[2], closeTo(appScreen.width / 2, 1));
  });

  for (final (tip, title) in [('Nearby places', 'Nearby places')]) {
    testWidgets('$tip opens its page, and Search comes back', (tester) async {
      await pumpFakeApp(tester, store: _store());
      await tester.tap(_tab(tip));
      await tester.pumpAndSettle();
      expect(find.widgetWithText(StubPage, title), findsOneWidget);
      expect(find.byType(SearchBar), findsNothing);

      await tester.tap(_tab('Search'));
      await tester.pumpAndSettle();
      expect(find.byType(SearchBar), findsOneWidget);
      expect(find.byType(StubPage), findsNothing);
    });
  }

  testWidgets('Nearby transit opens its page, and Search comes back', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    await tester.tap(_tab('Nearby transit'));
    await tester.pumpAndSettle();
    expect(find.byType(NearbyTransitPage), findsOneWidget);
    expect(find.byType(SearchBar), findsNothing);

    await tester.tap(_tab('Search'));
    await tester.pumpAndSettle();
    expect(find.byType(SearchBar), findsOneWidget);
    expect(find.byType(NearbyTransitPage), findsNothing);
  });

  testWidgets('Offline maps opens its page, and Search comes back', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    await tester.tap(_tab('Offline maps'));
    await tester.pumpAndSettle();
    expect(find.byType(OfflinePage), findsOneWidget);
    expect(find.byType(SearchBar), findsNothing);

    await tester.tap(_tab('Search'));
    await tester.pumpAndSettle();
    expect(find.byType(SearchBar), findsOneWidget);
    expect(find.byType(OfflinePage), findsNothing);
  });

  testWidgets('switching pages replaces the page, with the sheet in between', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    await tester.tap(_tab('Saved places'));
    await tester.pumpAndSettle();
    final sheet = SheetScaffold.controllerOf(
      tester.element(find.byType(SavedPage)),
    );
    expect(sheet.extent, appScreen.height);

    await tester.tap(_tab('Nearby places'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 16));
    // On its way down from where it was, not from the search page's height.
    expect(sheet.extent, greaterThan(appScreen.height * 0.7));
    await tester.pumpAndSettle();
    expect(sheet.extent, lessThan(appScreen.height / 2));
    expect(find.text('Nearby places'), findsOneWidget);
    expect(find.byType(SavedPage), findsNothing);
  });

  testWidgets('back on a page goes to search', (tester) async {
    final router = await pumpFakeApp(tester, store: _store());
    await tester.tap(_tab('Offline maps'));
    await tester.pumpAndSettle();

    final handled = await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(handled, isTrue);
    expect(router.state.uri.path, '/');
    expect(find.byType(SearchBar), findsOneWidget);
  });

  testWidgets('Search on the search page starts searching', (tester) async {
    await pumpFakeApp(tester, store: _store());
    await tester.tap(_tab('Search'));
    await tester.pumpAndSettle();
    final field = tester.widget<TextField>(find.byType(TextField));
    expect(field.focusNode!.hasFocus, isTrue);
    expect(
      SheetScaffold.controllerOf(tester.element(find.byType(SearchSheet)))
          .extent,
      appScreen.height,
    );
  });

  testWidgets('a place card slides the bar away, and closing brings it back', (
    tester,
  ) async {
    final router = await pumpFakeApp(tester, store: _store());
    final peeking = _barTop(tester);

    unawaited(router.push(placeLocation('poi:1')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 120));
    expect(_barTop(tester), greaterThan(peeking));
    await tester.pumpAndSettle();
    expect(find.byType(PlacePage), findsOneWidget);
    expect(find.byType(AppNavigationBar), findsNothing);

    router.pop();
    await tester.pumpAndSettle();
    expect(_barTop(tester), peeking);
  });
}
