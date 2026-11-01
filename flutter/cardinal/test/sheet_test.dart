import 'package:cardinal/main.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/sheet_test/pages.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
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
  await tester.pumpWidget(const MainApp());
  await tester.pumpAndSettle();
  return SheetScaffold.controllerOf(tester.element(find.byType(SectionsTestPage)));
}

void main() {
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
    await tester.pumpWidget(const MainApp());
    await tester.pumpAndSettle();
    final bar = tester.getRect(find.text(_barLabel));
    expect(bar.right, lessThan(500));
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
    final content = tester.getTopLeft(find.textContaining('Gravity:'));
    expect(content.dx, greaterThan(100), reason: 'the content slides with the gesture');

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
