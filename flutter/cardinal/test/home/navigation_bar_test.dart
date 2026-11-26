import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import 'package:cardinal/home/navigation_bar.dart';
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

const _tooltips = ['Saved places', 'Search', 'Nearby transit', 'Offline maps'];

/// The tabs the bar shows as the current page: filled, and selected to a screen reader.
List<String> _selected(WidgetTester tester) => [
  for (final tip in _tooltips)
    if (find
        .ancestor(
          of: _tab(tip),
          matching: find.byWidgetPredicate(
            (w) => w is Semantics && (w.properties.selected ?? false),
          ),
        )
        .evaluate()
        .isNotEmpty)
      tip,
];

/// Whether [tip]'s button is filled.
bool _filled(WidgetTester tester, String tip) {
  final button = tester.widget<IconButton>(
    find.ancestor(of: _tab(tip), matching: find.byType(IconButton)),
  );
  return tester
          .widget<Material>(
            find
                .descendant(
                  of: find.byWidget(button),
                  matching: find.byType(Material),
                )
                .first,
          )
          .color !=
      Colors.transparent;
}

SheetController _sheet(WidgetTester tester) =>
    SheetScaffold.controllerOf(tester.element(find.byType(AppNavigationBar)));

/// Flings the sheet down from its handle, hard enough to collapse it.
Future<void> _collapse(WidgetTester tester) async {
  final sheet = _sheet(tester);
  await tester.flingFrom(
    Offset(appScreen.width / 2, appScreen.height - sheet.extent + 12),
    const Offset(0, 60),
    3000,
  );
  await tester.pumpAndSettle();
  expect(sheet.extent, 0);
  expect(sheet.isCollapsed, isTrue);
}

void main() {
  testWidgets('sits over the navigation bar, the sheet resting above it', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    expect(_barTop(tester), appScreen.height - appNavigationBar - appBarHeight);
    final search = tester.getBottomLeft(find.byType(SearchBar)).dy;
    expect(search, lessThanOrEqualTo(_barTop(tester)));
  });

  testWidgets('has the four pages, in order, without Nearby places', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    final xs = [for (final tip in _tooltips) tester.getCenter(_tab(tip)).dx];
    expect(xs, orderedEquals([...xs]..sort()));
    expect(_tab('Nearby places'), findsNothing);
    expect(
      find.descendant(
        of: find.byType(AppNavigationBar),
        matching: find.byType(IconButton),
      ),
      findsNWidgets(4),
    );
  });

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
    sheet.animateTo(const SheetAnchor.full());
    await tester.pumpAndSettle();
    expect(sheet.extent, appScreen.height);

    await tester.tap(_tab('Search'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 16));
    // On its way down from where it was, not from the search page's height.
    expect(sheet.extent, greaterThan(appScreen.height * 0.7));
    await tester.pumpAndSettle();
    expect(sheet.extent, lessThan(appScreen.height / 2));
    expect(find.byType(SearchBar), findsOneWidget);
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

  testWidgets('the current page\'s button is filled, and only that one', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    expect(_selected(tester), ['Search']);
    expect(
      [for (final t in _tooltips) _filled(tester, t)],
      [false, true, false, false],
    );

    await tester.tap(_tab('Nearby transit'));
    await tester.pumpAndSettle();
    expect(_selected(tester), ['Nearby transit']);
    expect(_filled(tester, 'Nearby transit'), isTrue);
    expect(_filled(tester, 'Search'), isFalse);
  });

  testWidgets('collapsed, no page is current; its own button brings it back', (
    tester,
  ) async {
    final router = await pumpFakeApp(tester, store: _store());
    final peek = _sheet(tester).extent;
    await _collapse(tester);
    expect(_selected(tester), isEmpty);
    expect([
      for (final t in _tooltips) _filled(tester, t),
    ], everyElement(false));

    await tester.tap(_tab('Search'));
    await tester.pumpAndSettle();
    expect(router.state.uri.path, '/');
    expect(_sheet(tester).extent, peek);
    expect(_selected(tester), ['Search']);
    // Back as it was, not searching.
    final field = tester.widget<TextField>(find.byType(TextField));
    expect(field.focusNode!.hasFocus, isFalse);
  });

  testWidgets('collapsed, it comes back at the page\'s first anchor', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    await tester.tap(_tab('Saved places'));
    await tester.pumpAndSettle();
    final first = _sheet(tester).extent;
    await _collapse(tester);
    await tester.tap(_tab('Saved places'));
    await tester.pump();
    // Current again as soon as it starts back up.
    expect(_selected(tester), ['Saved places']);
    await tester.pumpAndSettle();
    expect(_sheet(tester).extent, first);
    expect(find.byType(SavedPage), findsOneWidget);
  });

  testWidgets('collapsed, another page\'s button opens it with the sheet up', (
    tester,
  ) async {
    final router = await pumpFakeApp(tester, store: _store());
    await _collapse(tester);
    await tester.tap(_tab('Offline maps'));
    await tester.pumpAndSettle();
    expect(router.state.uri.path, '/offline');
    expect(find.byType(OfflinePage), findsOneWidget);
    expect(_sheet(tester).extent, greaterThan(0));
    expect(_sheet(tester).isCollapsed, isFalse);
    expect(_selected(tester), ['Offline maps']);
  });

  for (final source in ['Search', 'Nearby transit', 'Offline maps']) {
    testWidgets('collapsed $source opens Saved places on the first tap', (
      tester,
    ) async {
      final router = await pumpFakeApp(tester, store: _store());
      if (source != 'Search') {
        await tester.tap(_tab(source));
        await tester.pumpAndSettle();
      }
      await _collapse(tester);
      await tester.tap(_tab('Saved places'));
      await tester.pumpAndSettle();
      expect(router.state.uri.path, '/saved');
      expect(find.byType(SavedPage), findsOneWidget);
      expect(_sheet(tester).isCollapsed, isFalse);
      expect(_sheet(tester).extent, appScreen.height * 0.4);
      expect(_selected(tester), ['Saved places']);
    });
  }

  testWidgets(
    'a page opened over a collapsed one closes back to it collapsed',
    (tester) async {
      final router = await pumpFakeApp(tester, store: _store());
      await _collapse(tester);
      // The bar's away under a place card.
      final sheet = _sheet(tester);
      unawaited(router.push(placeLocation('poi:1')));
      await tester.pumpAndSettle();
      expect(sheet.extent, greaterThan(0));

      router.pop();
      await tester.pumpAndSettle();
      expect(sheet.extent, 0);
      expect(sheet.isCollapsed, isTrue);
      expect(_selected(tester), isEmpty);
    },
  );

  testWidgets('collapsed is an anchor like any other', (tester) async {
    await pumpFakeApp(tester, store: _store());
    final sheet = _sheet(tester);
    final peek = sheet.extent;
    final handle = Offset(appScreen.width / 2, appScreen.height - peek + 12);
    // Let go a little way down, it snaps back to the nearest...
    await tester.drag(
      find.bySemanticsLabel('Resize sheet'),
      const Offset(0, 50),
    );
    await tester.pumpAndSettle();
    expect(sheet.extent, peek);
    // ...and a soft fling down settles on the next one down.
    await tester.flingFrom(handle, const Offset(0, 60), 800);
    await tester.pumpAndSettle();
    expect(sheet.isCollapsed, isTrue);
  });

  for (final (name, grab) in [('the handle', 12.0), ('the content', 40.0)]) {
    testWidgets('a drag from $name follows the finger over the bar and back', (
      tester,
    ) async {
      final router = await pumpFakeApp(tester, store: _store());
      final sheet = _sheet(tester);
      final peek = sheet.extent;
      var y = appScreen.height - peek + grab;
      final gesture = await tester.startGesture(Offset(appScreen.width / 2, y));
      Future<void> move(double dy) async {
        await gesture.moveBy(Offset(0, dy));
        y += dy;
        await tester.pump(const Duration(milliseconds: 16));
      }

      // Past the touch slop, which the sheet doesn't follow.
      await move(20);
      final start = sheet.extent;
      final startY = y;
      // Down over the bar, nearly to the bottom of the screen...
      final barTop = appScreen.height - appNavigationBar - appBarHeight;
      while (y < appScreen.height - 10) {
        await move(10);
        expect(sheet.isHeld, isTrue, reason: 'finger at $y');
        expect(
          sheet.extent,
          moreOrLessEquals(math.max(0, start - (y - startY)), epsilon: 0.01),
          reason: 'finger at $y',
        );
      }
      expect(y, greaterThan(barTop));
      // ...and back up, the same drag all the way.
      while (y > startY) {
        await move(-10);
        expect(sheet.isHeld, isTrue, reason: 'finger at $y');
      }
      expect(sheet.extent, moreOrLessEquals(start, epsilon: 0.01));
      // Down over the bar again, and let go there without a fling: nearest the
      // bottom, it collapses.
      while (y < appScreen.height - 10) {
        await move(10);
      }
      await gesture.up();
      await tester.pumpAndSettle();
      expect(sheet.isCollapsed, isTrue);
      // The bar's buttons never saw the finger.
      expect(router.state.uri.path, '/');
      expect(find.byType(SearchBar), findsOneWidget);
    });
  }

  testWidgets('a fling down that ends over the bar collapses the sheet', (
    tester,
  ) async {
    await pumpFakeApp(tester, store: _store());
    final sheet = _sheet(tester);
    final y = appScreen.height - sheet.extent + 40;
    await tester.flingFrom(
      Offset(appScreen.width / 2, y),
      Offset(0, appScreen.height - 10 - y),
      3000,
    );
    await tester.pumpAndSettle();
    expect(sheet.isCollapsed, isTrue);
  });

  testWidgets('the handle never collapses the sheet', (tester) async {
    await pumpFakeApp(tester, store: _store());
    await tester.tap(_tab('Nearby transit'));
    await tester.pumpAndSettle();
    final handle = find.bySemanticsLabel('Resize sheet');
    for (var i = 0; i < 4; i++) {
      await tester.tap(handle);
      await tester.pumpAndSettle();
      expect(_sheet(tester).extent, greaterThan(0));
    }
  });

  testWidgets('on iOS, the tabs\' labels fit under their icons at every text '
      'size, scaled no further than 1.3×', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await pumpFakeApp(tester, store: _store());
    // The glass bar's labels, rather than the search bar's hint.
    Finder label(String text) => find.descendant(
      of: find.byType(AppNavigationBar),
      matching: find.text(text),
    );
    expect(label('Nearby'), findsNothing);
    final heights = <double, double>{};
    for (final scale in [1.0, 1.3, 1.5, 2.0, 3.1]) {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull, reason: '$scale×');
      heights[scale] = tester.getSize(label('Saved')).height;
      for (final text in ['Saved', 'Search', 'Transit', 'Offline']) {
        // Within its tab, under its icon.
        final tab = tester.getRect(
          find.ancestor(of: label(text), matching: find.byType(Column)).first,
        );
        final rect = tester.getRect(label(text));
        expect(
          rect.top,
          greaterThanOrEqualTo(tab.top),
          reason: '$text, $scale×',
        );
        expect(
          rect.bottom,
          lessThanOrEqualTo(tab.bottom),
          reason: '$text, $scale×',
        );
      }
    }
    expect(heights[1.3], greaterThan(heights[1.0]!));
    expect(heights[1.5], heights[1.3]);
    expect(heights[2.0], heights[1.3]);
    expect(heights[3.1], heights[1.3]);
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('on iOS, the lens leaves while collapsed, and comes back', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await pumpFakeApp(tester, store: _store());
    // The glass bar's tab, rather than the search bar's hint.
    Finder tab(String label) => find.byWidgetPredicate(
      (w) => w is Semantics && w.properties.label == label,
    );
    bool selected(String label) =>
        tester.widget<Semantics>(tab(label)).properties.selected ?? false;
    expect(selected('Search'), isTrue);
    await _collapse(tester);
    expect(selected('Search'), isFalse);
    await tester.tap(tab('Search'));
    await tester.pumpAndSettle();
    expect(selected('Search'), isTrue);
    debugDefaultTargetPlatformOverride = null;
  });
}
