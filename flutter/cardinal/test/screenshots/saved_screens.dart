// Screenshots of Saved's screens, for review: see `render.dart`.
import 'package:cardinal/saved/saved_page.dart';
import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoIcons;
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import 'render.dart';

/// Saved with two lists, one nested, and three places, one pinned with an emoji.
Future<Store> _saved() async {
  final store = fakeStore();
  final food = await store.saved.createList('Food');
  await store.saved.createList('Coffee', parent: food);
  await store.saved.createList('Trips');
  for (final (id, name) in [
    ('poi:1', 'Cafe Allegro'),
    ('poi:2', 'Space Needle'),
    ('poi:3', 'Gas Works Park'),
  ]) {
    await store.places.put(
      Place(id: id, kind: 'poi', name: name, center: const LngLat(0, 0)),
    );
    await store.saved.savePlace(id);
  }
  final cafe = (await store.saved.watch().first).copiesOf('poi:1').single;
  await store.saved.edit(cafe, name: '', emoji: '☕');
  await store.saved.setPinned([cafe.entryId], true);
  return store;
}

/// A test of screens, drawn with real shadows rather than the outlines tests draw.
void _screens(String name, Future<void> Function(WidgetTester tester) body) =>
    testWidgets(name, (tester) async {
      debugDisableShadows = false;
      try {
        await body(tester);
      } finally {
        debugDisableShadows = true;
      }
    });

/// A long press frame by frame, as a finger's: iOS's context menu opens as it goes.
Future<void> _hold(WidgetTester tester, Finder finder) async {
  final gesture = await tester.startGesture(tester.getCenter(finder));
  for (var i = 0; i < 60; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
  await gesture.up();
}

SheetController _sheet(WidgetTester tester) =>
    SheetScaffold.controllerOf(tester.element(find.byType(SavedPage).last));

void main() {
  setUpAll(loadRenderFonts);
  // As on a phone, with no keyboard: no focus highlights.
  setUp(
    () => FocusManager.instance.highlightStrategy =
        FocusHighlightStrategy.alwaysTouch,
  );

  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    final os = platform == TargetPlatform.iOS ? 'ios' : 'android';

    _screens('$os browsing', (tester) async {
      await pumpFakeApp(
        tester,
        store: await _saved(),
        platform: platform,
        location: '/saved',
      );
      await screenshot(tester, 'saved-$os-1-browsing-40');
      _sheet(tester).animateTo(const SheetAnchor.full());
      await screenshot(tester, 'saved-$os-2-browsing-full');
      await tester.tap(find.text('Food'));
      await screenshot(tester, 'saved-$os-3-nested-full');
    });

    _screens('$os selecting', (tester) async {
      await pumpFakeApp(
        tester,
        store: await _saved(),
        platform: platform,
        location: '/saved',
      );
      if (platform == TargetPlatform.android) {
        await tester.longPress(find.text('Food'));
      } else {
        await tester.tap(find.byIcon(CupertinoIcons.ellipsis_circle));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Select'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Food'));
      }
      await screenshot(tester, 'saved-$os-4-selecting-40');
    });
  }

  for (final (name, shot) in [
    ('ios header menu', 'saved-ios-5-header-menu'),
    ('ios row menu', 'saved-ios-6-context-menu'),
    ('ios swipe left', 'saved-ios-7-swipe-left'),
    ('ios swipe right', 'saved-ios-8-swipe-right'),
    ('ios back menu', 'saved-ios-9-back-menu'),
  ]) {
    _screens(name, (tester) async {
      await pumpFakeApp(
        tester,
        store: await _saved(),
        platform: TargetPlatform.iOS,
        location: '/saved',
      );
      _sheet(tester).animateTo(const SheetAnchor.full());
      await tester.pumpAndSettle();
      switch (name) {
        case 'ios header menu':
          await tester.tap(find.byIcon(CupertinoIcons.ellipsis_circle));
        case 'ios row menu':
          await _hold(tester, find.text('Space Needle'));
        case 'ios swipe left':
          await tester.drag(find.text('Space Needle'), const Offset(-120, 0));
        case 'ios swipe right':
          await tester.drag(find.text('Space Needle'), const Offset(90, 0));
        case 'ios back menu':
          await tester.tap(find.text('Food'));
          await tester.pumpAndSettle();
          await tester.tap(find.text('Coffee'));
          await tester.pumpAndSettle();
          await _hold(tester, find.text('Food'));
      }
      await screenshot(tester, shot);
    });
  }

  _screens('android row menu', (tester) async {
    await pumpFakeApp(
      tester,
      store: await _saved(),
      platform: TargetPlatform.android,
      location: '/saved',
    );
    await tester.tap(
      find.descendant(
        of: find.widgetWithText(ListTile, 'Food'),
        matching: find.byTooltip('More options'),
      ),
    );
    await screenshot(tester, 'saved-android-5-row-menu');
  });
}
