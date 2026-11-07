import 'dart:math' as math;

import 'package:cardinal/main.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/sheet_test/pages.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

const _phone = Size(400, 800);
const _barLabel = 'Section A: narrows as B shows';

Future<SheetController> _pumpPhone(WidgetTester tester) async {
  tester.view
    ..physicalSize = _phone * 2
    ..devicePixelRatio = 2
    ..padding = const FakeViewPadding(top: 48, bottom: 96)
    ..viewPadding = const FakeViewPadding(top: 48, bottom: 96);
  addTearDown(tester.view.reset);
  sheetTestRouter.go('/');
  await tester.pumpWidget(MainApp(router: sheetTestRouter));
  await tester.pumpAndSettle();
  return SheetScaffold.controllerOf(tester.element(find.byType(SectionsTestPage)));
}

void main() {
  group('restoration', restoration);
  group('cancelled back gestures', cancelledBackGestures);
  group('SheetPhysics.target', () {
    const physics = SheetPhysics();
    final anchors = [
      ResolvedAnchor(const SheetAnchor.fraction(0.1), 100, physics.gravity),
      ResolvedAnchor(const SheetAnchor.fraction(0.4), 400, physics.gravity),
      ResolvedAnchor(const SheetAnchor.full(), 800, physics.gravity),
    ];

    test('slow releases settle on the nearest anchor', () {
      expect(physics.target(anchors, 230, 100), 0);
      expect(physics.target(anchors, 260, -100), 1);
    });

    test('a fling reaches at least the next anchor', () {
      expect(physics.target(anchors, 110, 300), 1);
      expect(physics.target(anchors, 390, -300), 0);
    });

    test('a hard fling skips anchors', () {
      expect(physics.target(anchors, 100, 5000), 2);
    });

    test('gravity catches a fling', () {
      final strong = [
        anchors[0],
        ResolvedAnchor(const SheetAnchor.fraction(0.4), 400, 10000),
        anchors[2],
      ];
      expect(physics.target(strong, 100, 5000), 1);
    });
  });

  group('SheetGeometry', () {
    const geometry = SheetGeometry(
      size: _phone,
      padding: EdgeInsets.only(top: 24, bottom: 48),
      placement: SheetPlacement.bottom,
    );

    test('extentShowing inverts contentShown', () {
      for (final y in [0.0, 100.0, 500.0, 700.0, 720.0]) {
        final extent = geometry.extentShowing(y);
        expect(geometry.contentShown(extent), moreOrLessEquals(y, epsilon: 1e-6));
      }
    });

    test('squares off at full', () {
      expect(geometry.cornerRadiusAt(400), 28);
      expect(geometry.cornerRadiusAt(800), 0);
      expect(geometry.contentOffset(800), 24);
    });
  });

  testWidgets('rests at the first section, with it clear of the navigation bar', (tester) async {
    final sheet = await _pumpPhone(tester);
    expect(sheet.restingAt, const SheetAnchor.section(#a));
    final bar = tester.getRect(find.text(_barLabel));
    expect(bar.bottom, lessThanOrEqualTo(_phone.height - 48));
    expect(bar.top, greaterThan(_phone.height - 200));
  });

  testWidgets('drags up through the anchors and scrolls once full', (tester) async {
    final sheet = await _pumpPhone(tester);
    final start = sheet.extent;
    await tester.drag(find.text(_barLabel), const Offset(0, -100));
    await tester.pumpAndSettle();
    expect(sheet.extent, greaterThan(start));
    expect(sheet.anchors.map((a) => a.anchor), contains(sheet.restingAt));

    await tester.fling(find.text(_barLabel), const Offset(0, -300), 4000);
    await tester.pumpAndSettle();
    expect(sheet.restingAt, const SheetAnchor.full());
    expect(sheet.extent, _phone.height);

    final before = tester.getRect(find.text('Tile 3')).top;
    await tester.drag(find.text('Tile 3'), const Offset(0, -200));
    await tester.pumpAndSettle();
    expect(sheet.extent, _phone.height, reason: 'the content scrolls instead');
    expect(tester.getRect(find.text('Tile 3')).top, lessThan(before - 150));

    // Dragging down scrolls the content back before lowering the sheet.
    await tester.drag(find.text('Tile 3'), const Offset(0, 600));
    await tester.pumpAndSettle();
    expect(sheet.extent, lessThan(_phone.height));
  });

  testWidgets('swaps pages within the sheet and comes back', (tester) async {
    final sheet = await _pumpPhone(tester);
    await tester.drag(find.text(_barLabel), const Offset(0, -60));
    await tester.pumpAndSettle();
    final left = sheet.restingAt;

    await tester.tap(find.text('Long page'));
    await tester.pumpAndSettle();
    expect(sheet.restingAt, const SheetAnchor.fraction(0.4));
    expect(sheet.extent, moreOrLessEquals(_phone.height * 0.4));

    await tester.tap(find.byType(CloseButton));
    await tester.pumpAndSettle();
    expect(find.byType(SectionsTestPage), findsOneWidget);
    expect(sheet.restingAt, left);
  });

  testWidgets('docks on a wide window', (tester) async {
    tester.view
      ..physicalSize = const Size(1000, 700)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    sheetTestRouter.go('/');
    await tester.pumpWidget(MainApp(router: sheetTestRouter));
    await tester.pumpAndSettle();
    final bar = tester.getRect(find.text(_barLabel));
    expect(bar.right, lessThan(500));
  });

  testWidgets('rests above the keyboard, and never goes behind it', (tester) async {
    final sheet = await _pumpPhone(tester);
    final peek = sheet.extent;
    // A 300 logical pixel keyboard, over the 48 pixel navigation bar.
    tester.view.viewInsets = const FakeViewPadding(bottom: 600);
    await tester.pumpAndSettle();
    expect(sheet.extent, closeTo(peek - 48 + 300, 0.5));
    final bar = tester.getRect(find.text(_barLabel));
    expect(bar.bottom, lessThanOrEqualTo(_phone.height - 300));

    final gesture = await tester.startGesture(Offset(200, _phone.height - sheet.extent + 12));
    for (var i = 0; i < 20; i++) {
      await gesture.moveBy(const Offset(0, 40));
      await tester.pump(const Duration(milliseconds: 16));
      expect(sheet.extent, greaterThanOrEqualTo(300));
    }
    await gesture.up();
    await tester.pumpAndSettle();
    expect(sheet.extent, closeTo(peek - 48 + 300, 0.5));

    // Half way from the keyboard's top to the top of the screen.
    sheet.animateTo(const SheetAnchor.fraction(0.5));
    await tester.pumpAndSettle();
    expect(sheet.extent, closeTo(300 + (_phone.height - 300) / 2, 0.5));

    tester.view.resetViewInsets();
    await tester.pumpAndSettle();
    expect(sheet.extent, closeTo(_phone.height / 2, 0.5));
  });

  testWidgets('follows an anchor that moves while it springs there', (tester) async {
    final sheet = await _pumpPhone(tester);
    final peek = sheet.extent;
    tester.view.viewInsets = const FakeViewPadding(bottom: 600);
    await tester.pumpAndSettle();
    sheet.animateTo(const SheetAnchor.fraction(0.5));
    await tester.pumpAndSettle();
    // Spring back to the peek, and hide the keyboard on the way, as tapping a search
    // result does.
    sheet.animateTo(const SheetAnchor.section(#a));
    await tester.pump(const Duration(milliseconds: 50));
    tester.view.resetViewInsets();
    await tester.pumpAndSettle();
    expect(sheet.restingAt, const SheetAnchor.section(#a));
    expect(sheet.extent, moreOrLessEquals(peek, epsilon: 0.5));
  });

  testWidgets('drags slowly from the handle', (tester) async {
    final sheet = await _pumpPhone(tester);
    for (final kind in [PointerDeviceKind.touch, PointerDeviceKind.mouse]) {
      sheet.animateTo(const SheetAnchor.section(#a));
      await tester.pumpAndSettle();
      final before = sheet.extent;
      final gesture = await tester.startGesture(
        Offset(200, _phone.height - sheet.extent + 18),
        kind: kind,
      );
      await tester.pump(const Duration(milliseconds: 200));
      for (var i = 0; i < 30; i++) {
        await gesture.moveBy(const Offset(0, -2));
        await tester.pump(const Duration(milliseconds: 16));
      }
      expect(sheet.isHeld, isTrue, reason: 'still held by the handle once past the slop');
      // 60 pixels, less the touch slop.
      expect(sheet.extent, greaterThan(before + 30));
      await gesture.up();
      await tester.pumpAndSettle();
    }
  });

  testWidgets('a back gesture scrubs the sheet back, and carries on from where it was let go', (
    tester,
  ) async {
    final sheet = await _pumpPhone(tester);
    final below = sheet.extent;
    await tester.tap(find.text('Long page'));
    await tester.pumpAndSettle();
    final above = sheet.extent;
    expect(above, greaterThan(below));

    Future<void> send(String method, [double? progress]) async {
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

    await send('startBackGesture', 0);
    await send('updateBackGestureProgress', 0.5);
    final halfway = sheet.extent;
    expect(halfway, moreOrLessEquals((above + below) / 2, epsilon: 1));
    final fade = tester.widget<Opacity>(
      find.ancestor(of: find.textContaining('Gravity:'), matching: find.byType(Opacity)).first,
    );
    expect(fade.opacity, inExclusiveRange(0, 1), reason: 'the content fades with the gesture');

    await send('commitBackGesture');
    var last = halfway;
    for (var i = 0; i < 30; i++) {
      await tester.pump(const Duration(milliseconds: 16));
      expect(sheet.extent, lessThanOrEqualTo(last + 0.01), reason: 'no jump back up');
      last = sheet.extent;
    }
    await tester.pumpAndSettle();
    expect(find.byType(LongTestPage), findsNothing);
    expect(sheet.extent, moreOrLessEquals(below));
  });
}

/// Sends a predictive back event, as Android's engine does.
Future<void> _sendBack(WidgetTester tester, String method, [double? progress]) async {
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

void cancelledBackGestures() {
  // From a full sheet to a page that rests lower, as search results open a place.
  Future<(SheetController, double)> pushLower(WidgetTester tester) async {
    final sheet = await _pumpPhone(tester);
    sheet.animateTo(const SheetAnchor.full());
    await tester.pumpAndSettle();
    sheetTestRouter.push('/short');
    await tester.pumpAndSettle();
    final resting = sheet.extent;
    expect(resting, lessThan(_phone.height - 1));
    return (sheet, resting);
  }

  Future<void> expectRecovered(WidgetTester tester, SheetController sheet, double resting) async {
    await tester.pumpAndSettle();
    expect(find.byType(ShortTestPage), findsOneWidget);
    // At rest on one of the page's anchors (its readout grows a line once it rests, so
    // not quite where it first rested), well below the page beneath's full height.
    final anchor = sheet.anchors.firstWhere((a) => a.anchor == sheet.restingAt);
    expect(sheet.extent, moreOrLessEquals(anchor.extent));
    expect(sheet.extent, moreOrLessEquals(resting, epsilon: 40));
    expect(sheet.isTransitioning, isFalse);
    final navigator = Navigator.of(tester.element(find.byType(ShortTestPage)));
    expect(navigator.userGestureInProgress, isFalse);
    // Back still goes back.
    await _sendBack(tester, 'startBackGesture', 0);
    await _sendBack(tester, 'commitBackGesture');
    await tester.pumpAndSettle();
    expect(find.byType(ShortTestPage), findsNothing);
  }

  testWidgets('a back gesture cancelled at once', (tester) async {
    final (sheet, resting) = await pushLower(tester);
    await _sendBack(tester, 'startBackGesture', 0);
    await _sendBack(tester, 'cancelBackGesture');
    await expectRecovered(tester, sheet, resting);
  });

  testWidgets('a back gesture cancelled part way', (tester) async {
    final (sheet, resting) = await pushLower(tester);
    await _sendBack(tester, 'startBackGesture', 0);
    await _sendBack(tester, 'updateBackGestureProgress', 0.4);
    await _sendBack(tester, 'cancelBackGesture');
    await expectRecovered(tester, sheet, resting);
  });

  testWidgets('a second back gesture cancelled while the first settles', (tester) async {
    final (sheet, resting) = await pushLower(tester);
    await _sendBack(tester, 'startBackGesture', 0);
    await _sendBack(tester, 'updateBackGestureProgress', 0.4);
    await _sendBack(tester, 'cancelBackGesture');
    await tester.pump(const Duration(milliseconds: 50));
    await _sendBack(tester, 'startBackGesture', 0);
    await _sendBack(tester, 'updateBackGestureProgress', 0.1);
    await _sendBack(tester, 'cancelBackGesture');
    await expectRecovered(tester, sheet, resting);
  });

  testWidgets('back gestures with no progress, one after another', (tester) async {
    final (sheet, resting) = await pushLower(tester);
    for (var i = 0; i < 2; i++) {
      await _sendBack(tester, 'startBackGesture', 0);
      await tester.pump(const Duration(milliseconds: 50));
      await _sendBack(tester, 'cancelBackGesture');
    }
    await expectRecovered(tester, sheet, resting);
  });

  testWidgets('a back gesture cancelled while the page is still coming in', (tester) async {
    final sheet = await _pumpPhone(tester);
    sheet.animateTo(const SheetAnchor.full());
    await tester.pumpAndSettle();
    sheetTestRouter.push('/short');
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    await _sendBack(tester, 'startBackGesture', 0);
    await _sendBack(tester, 'updateBackGestureProgress', 0.2);
    await _sendBack(tester, 'cancelBackGesture');
    await tester.pumpAndSettle();
    final resting = sheet.extent;
    expect(resting, lessThan(_phone.height - 1));
    await expectRecovered(tester, sheet, resting);
  });

  for (final end in ['cancelBackGesture', 'commitBackGesture']) {
    testWidgets('a page closed during a back gesture closes ($end)', (tester) async {
      final (sheet, _) = await pushLower(tester);
      final below = SheetScaffold.controllerOf(tester.element(find.byType(ShortTestPage)));
      await _sendBack(tester, 'startBackGesture', 0);
      // Closed some other way, as by a button, while the gesture is under way.
      sheetTestRouter.pop();
      await _sendBack(tester, 'updateBackGestureProgress', 0);
      await _sendBack(tester, 'updateBackGestureProgress', 0.1);
      await _sendBack(tester, end);
      await tester.pumpAndSettle();
      expect(find.byType(ShortTestPage), findsNothing);
      expect(find.byType(SectionsTestPage), findsOneWidget, reason: 'popped only once');
      final navigator = Navigator.of(tester.element(find.byType(SectionsTestPage)));
      expect(navigator.userGestureInProgress, isFalse);
      expect(below.isTransitioning, isFalse);
      expect(sheet.extent, moreOrLessEquals(_phone.height));
    });
  }
}

/// The sheet test pages in an app that restores where it was, making its router afresh
/// each time it starts, as an app does when the system restarts it.
class _RestorableApp extends StatefulWidget {
  const _RestorableApp();

  @override
  State<_RestorableApp> createState() => _RestorableAppState();
}

class _RestorableAppState extends State<_RestorableApp> {
  static SheetPage<void> page(GoRouterState state, Widget child) =>
      SheetPage(key: state.pageKey, restorationId: state.pageKey.value, child: child);

  late final router = GoRouter(
    restorationScopeId: 'router',
    routes: [
      ShellRoute(
        restorationScopeId: 'sheet',
        builder: (context, state, child) => Scaffold(
          body: SheetScaffold(body: const SizedBox.expand(), sheet: child),
        ),
        routes: [
          GoRoute(
            path: '/',
            pageBuilder: (context, state) => page(state, const SectionsTestPage()),
            routes: [
              GoRoute(
                path: 'short',
                pageBuilder: (context, state) => page(state, const ShortTestPage()),
              ),
            ],
          ),
        ],
      ),
    ],
  );

  @override
  void dispose() {
    router.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      MaterialApp.router(restorationScopeId: 'app', routerConfig: router);
}

void restoration() {
  Future<void> pumpRestorable(WidgetTester tester) async {
    tester.view
      ..physicalSize = _phone * 2
      ..devicePixelRatio = 2;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(const _RestorableApp());
    await tester.pumpAndSettle();
  }

  SheetController sheetOf(WidgetTester tester, Type page) =>
      SheetScaffold.controllerOf(tester.element(find.byType(page)));

  testWidgets('a page restored on top opens as if pushed', (tester) async {
    await pumpRestorable(tester);
    final below = sheetOf(tester, SectionsTestPage).extent;
    GoRouter.of(tester.element(find.byType(SectionsTestPage))).push('/short');
    await tester.pumpAndSettle();
    final resting = sheetOf(tester, ShortTestPage).extent;

    await tester.restartAndRestore();
    expect(find.byType(ShortTestPage), findsOneWidget, reason: 'restored');
    final route = ModalRoute.of(tester.element(find.byType(ShortTestPage)))!;
    expect(route.animation!.value, lessThan(1), reason: 'opening');
    await tester.pump(const Duration(milliseconds: 100));
    final sheet = sheetOf(tester, ShortTestPage);
    expect(sheet.isTransitioning, isTrue);
    expect(
      sheet.extent,
      inExclusiveRange(math.min(resting, below), math.max(resting, below)),
      reason: 'on its way from the page below',
    );

    await tester.pumpAndSettle();
    expect(route.animation!.value, 1);
    expect(sheet.extent, moreOrLessEquals(resting, epsilon: 40));
    final anchor = sheet.anchors.firstWhere((a) => a.anchor == sheet.restingAt);
    expect(sheet.extent, moreOrLessEquals(anchor.extent));

    // And back goes back.
    GoRouter.of(tester.element(find.byType(ShortTestPage))).pop();
    await tester.pumpAndSettle();
    expect(find.byType(ShortTestPage), findsNothing);
    expect(sheetOf(tester, SectionsTestPage).extent, moreOrLessEquals(below, epsilon: 40));
  });

  testWidgets('the bottom page restores without opening', (tester) async {
    await pumpRestorable(tester);
    await tester.restartAndRestore();
    final route = ModalRoute.of(tester.element(find.byType(SectionsTestPage)))!;
    expect(route.animation!.value, 1);
    expect(sheetOf(tester, SectionsTestPage).isTransitioning, isFalse);
  });
}
