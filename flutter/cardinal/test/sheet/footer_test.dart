import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import 'held_harness.dart';

Finder get _footer => find.byKey(const ValueKey('footer'));

double _extentOf(SheetController sheet, SheetAnchor anchor) =>
    sheet.anchors.firstWhere((a) => a.anchor == anchor).extent;

void main() {
  testWidgets('stays at the bottom of the window wherever the sheet rests', (
    tester,
  ) async {
    final sheet = await pumpHarness(tester, (key) => Harness(key: key));
    const place = Rect.fromLTRB(0, footerTop, 400, footerTop + footerHeight);
    expect(sheet.restingAt, headerAnchor);
    expect(tester.getRect(_footer), place);

    await moveTo(tester, sheet, footerAnchor);
    expect(sheet.restingAt, footerAnchor);
    expect(tester.getRect(_footer), place);

    await moveTo(tester, sheet, full);
    expect(sheet.extent, screen.height);
    expect(tester.getRect(_footer), place);
  });

  testWidgets('stays put while the sheet is dragged', (tester) async {
    final sheet = await pumpHarness(tester, (key) => Harness(key: key));
    final start = sheet.extent;
    final gesture = await tester.startGesture(
      tester.getCenter(find.text('header')),
    );
    for (var i = 0; i < 20; i++) {
      await gesture.moveBy(const Offset(0, -20));
      await tester.pump(const Duration(milliseconds: 16));
      expect(tester.getRect(_footer).top, footerTop);
    }
    expect(sheet.extent, greaterThan(start + 300));
    // And pulled down past the lowest anchor.
    for (var i = 0; i < 40; i++) {
      await gesture.moveBy(const Offset(0, 20));
      await tester.pump(const Duration(milliseconds: 16));
      expect(tester.getRect(_footer).top, footerTop);
    }
    expect(sheet.extent, lessThan(_extentOf(sheet, footerAnchor)));
    await gesture.up();
    await tester.pumpAndSettle();
    expect(tester.getRect(_footer).top, footerTop);
  });

  testWidgets('sits above the keyboard', (tester) async {
    final sheet = await pumpHarness(tester, (key) => Harness(key: key));
    // A 300 logical pixel keyboard, over the navigation bar.
    tester.view.viewInsets = const FakeViewPadding(bottom: 600);
    await tester.pumpAndSettle();
    expect(tester.getRect(_footer).bottom, screen.height - 300);
    expect(tester.getRect(_footer).height, footerHeight);
    // The header shows above it.
    expect(
      sheet.extent,
      moreOrLessEquals(headerHeight + footerHeight + 300 + handle),
    );
    expect(
      tester.getRect(find.byKey(const ValueKey('header'))).bottom,
      moreOrLessEquals(screen.height - 300 - footerHeight),
    );

    tester.view.resetViewInsets();
    await tester.pumpAndSettle();
    expect(tester.getRect(_footer).top, footerTop);
  });

  testWidgets('the anchors keep clear of it, and of it alone without it', (
    tester,
  ) async {
    final sheet = await pumpHarness(tester, (key) => Harness(key: key));
    expect(
      _extentOf(sheet, footerAnchor),
      footerHeight + navigationBar + handle,
    );
    expect(
      _extentOf(sheet, headerAnchor),
      headerHeight + footerHeight + navigationBar + handle,
    );
    expect(
      tester.getRect(find.byKey(const ValueKey('header'))).bottom,
      footerTop,
    );

    harness.currentState!.change(() => harness.currentState!.footer = false);
    await tester.pumpAndSettle();
    expect(_footer, findsNothing);
    expect(_extentOf(sheet, footerAnchor), navigationBar + handle);
    expect(
      _extentOf(sheet, headerAnchor),
      headerHeight + navigationBar + handle,
    );
    expect(sheet.extent, _extentOf(sheet, headerAnchor));
  });

  testWidgets('the anchors follow its height', (tester) async {
    final sheet = await pumpHarness(tester, (key) => Harness(key: key));
    harness.currentState!.change(() => harness.currentState!.footerExtent = 80);
    await tester.pumpAndSettle();
    expect(tester.getRect(_footer).top, screen.height - navigationBar - 80);
    expect(sheet.extent, headerHeight + 80 + navigationBar + handle);
    expect(
      tester.getRect(find.byKey(const ValueKey('header'))).bottom,
      screen.height - navigationBar - 80,
    );
  });

  testWidgets('the end of the content scrolls clear of it', (tester) async {
    final sheet = await pumpHarness(tester, (key) => Harness(key: key));
    await moveTo(tester, sheet, full);
    final position = tester
        .state<ScrollableState>(find.byType(Scrollable).last)
        .position;
    // All of the content, and room for the footer and the navigation bar, less what
    // shows below the status bar.
    expect(
      position.maxScrollExtent,
      headerHeight +
          20 * rowHeight +
          footerHeight +
          navigationBar -
          (screen.height - statusBar),
    );
    position.jumpTo(position.maxScrollExtent);
    await tester.pump();
    expect(tester.getRect(row(19)).bottom, footerTop);
  });

  testWidgets('a drag that starts on it moves the sheet', (tester) async {
    final sheet = await pumpHarness(tester, (key) => Harness(key: key));
    final start = sheet.extent;
    await tester.timedDrag(
      _footer,
      const Offset(0, -200),
      const Duration(seconds: 1),
    );
    await tester.pumpAndSettle();
    expect(sheet.extent, greaterThan(start + 150));
    expect(sheet.restingIn, freeRange);
    expect(tester.getRect(_footer).top, footerTop);
  });

  testWidgets('runs along the bottom of the column in a wide window', (
    tester,
  ) async {
    final sheet = await pumpHarness(
      tester,
      (key) => Harness(key: key),
      size: const Size(1000, 800),
    );
    const place = Rect.fromLTRB(16, footerTop, 416, footerTop + footerHeight);
    expect(tester.getRect(_footer), place);
    expect(sheet.extent, headerHeight + footerHeight + navigationBar + handle);
    await moveTo(tester, sheet, full);
    expect(tester.getRect(_footer), place);
    await moveTo(tester, sheet, footerAnchor);
    expect(tester.getRect(_footer), place);
  });
}
