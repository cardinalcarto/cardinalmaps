import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import 'held_harness.dart';

/// Where a held row rests against the footer.
const _aboveFooter = footerTop - rowHeight;

Future<void> _hold(WidgetTester tester, Object? id) async {
  harness.currentState!.change(() => harness.currentState!.held = id);
  // The frame that takes the change up.
  await tester.pump();
}

ScrollPosition _scroll(WidgetTester tester) =>
    tester.state<ScrollableState>(find.byType(Scrollable).last).position;

double _headerBottom(WidgetTester tester) =>
    tester.getRect(find.byKey(const ValueKey('header'))).bottom;

void main() {
  group('a held slot', () {
    testWidgets('shows in place while its slot is in view', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2, initialSection: #three),
      );
      final extent = sheet.extent;
      expect(sheet.restingIn, freeRange);
      for (var i = 0; i < 4; i++) {
        expect(rowTop(tester, i), slotTop(extent, i));
        expect(painted(tester, i)!.opacity, 1);
      }
      expect(tester.getRect(row(2)).width, screen.width);
      expect(rowState(tester, 2).displacement, 0);
    });

    testWidgets('stops above the footer as the sheet lowers', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
      );
      // At the header, the row is directly under it, over the first row's slot.
      expect(sheet.restingAt, headerAnchor);
      expect(rowTop(tester, 2), _aboveFooter);
      expect(_headerBottom(tester), _aboveFooter);
      expect(rowTop(tester, 0), _aboveFooter);
      expect(
        rowState(tester, 2).displacement,
        slotTop(sheet.extent, 2) - _aboveFooter,
      );

      // It holds still as the rest moves down behind it.
      final gesture = await tester.startGesture(tester.getCenter(row(2)));
      await gesture.moveBy(const Offset(0, 30));
      await tester.pump();
      final before = _headerBottom(tester);
      await gesture.moveBy(const Offset(0, 30));
      await tester.pump();
      expect(_headerBottom(tester), before + 30);
      expect(rowTop(tester, 2), _aboveFooter);
      await gesture.up();
      await tester.pumpAndSettle();

      // At the footer anchor, the handle is directly on it.
      await moveTo(tester, sheet, footerAnchor);
      expect(rowTop(tester, 2), _aboveFooter);
      expect(contentTop(sheet.extent), _aboveFooter);
      expect(
        rowState(tester, 2).displacement,
        slotTop(sheet.extent, 2) - _aboveFooter,
      );
    });

    testWidgets('travels with the list again once its slot is back in view', (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
      );
      final gesture = await tester.startGesture(tester.getCenter(row(2)));
      var met = false;
      for (var i = 0; i < 30; i++) {
        await gesture.moveBy(const Offset(0, -15));
        await tester.pump(const Duration(milliseconds: 16));
        final slot = slotTop(sheet.extent, 2);
        // Never below the top of the footer, and never away from a slot in view.
        expect(
          rowTop(tester, 2),
          moreOrLessEquals(slot < _aboveFooter ? slot : _aboveFooter),
        );
        met = met || slot < _aboveFooter;
      }
      expect(met, isTrue);
      await gesture.up();
      await tester.pumpAndSettle();
    });

    testWidgets('stops at the top once scrolled past at full height', (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
      );
      await moveTo(tester, sheet, full);
      expect(rowTop(tester, 2), statusBar + headerHeight + 2 * rowHeight);

      await tester.timedDrag(
        find.text('header'),
        const Offset(0, -300),
        const Duration(seconds: 1),
      );
      await tester.pumpAndSettle();
      final scrolled = _scroll(tester).pixels;
      expect(scrolled, greaterThan(250));
      expect(sheet.extent, screen.height);
      // At the top of the content, with the list passing behind it.
      expect(rowTop(tester, 2), statusBar);
      expect(
        rowTop(tester, 6),
        statusBar + headerHeight + 6 * rowHeight - scrolled,
      );
      expect(
        rowState(tester, 2).displacement,
        headerHeight + 2 * rowHeight - scrolled,
      );
      expect(rowState(tester, 2).displacement, lessThan(0));

      // And comes back down with its slot.
      _scroll(tester).jumpTo(50);
      await tester.pump();
      expect(rowTop(tester, 2), statusBar + headerHeight + 2 * rowHeight - 50);
      await tester.pump();
      expect(rowState(tester, 2).displacement, 0);
    });

    testWidgets("keeps its child's state as it's taken up and put back", (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2, initialSection: #three),
      );
      final state = rowState(tester, 2);
      await tester.tap(row(2));
      await tester.pump();
      expect(state.taps, 1);

      await moveTo(tester, sheet, footerAnchor);
      expect(rowTop(tester, 2), _aboveFooter);
      expect(rowState(tester, 2), same(state));
      expect(find.text('row 2: 1'), findsOneWidget);

      await _hold(tester, 5);
      await tester.pumpAndSettle();
      expect(rowState(tester, 2), same(state));
      expect(state.taps, 1);
      expect(state.mounted, isTrue);

      await _hold(tester, 2);
      await tester.pumpAndSettle();
      expect(rowState(tester, 2), same(state));
      expect(find.text('row 2: 1'), findsOneWidget);
    });

    testWidgets('is hit where it is painted', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
      );
      // Over the first row's slot.
      final paints = rowState(tester, 2).paints;
      await tester.tapAt(const Offset(200, _aboveFooter + rowHeight / 2));
      await tester.pump();
      expect(rowState(tester, 2).taps, 1);
      expect(rowState(tester, 0).taps, 0);
      // It paints again where it's held, when it has something new to show.
      expect(rowState(tester, 2).paints, paints + 1);
      expect(rowTop(tester, 2), _aboveFooter);

      // A horizontal drag is the child's own.
      await tester.dragFrom(
        const Offset(200, _aboveFooter + rowHeight / 2),
        const Offset(-100, 0),
      );
      await tester.pumpAndSettle();
      expect(rowState(tester, 2).swiped, lessThan(-50));
      expect(sheet.restingAt, headerAnchor);

      // At the top, over whichever row is passing behind.
      await moveTo(tester, sheet, full);
      _scroll(tester).jumpTo(300);
      await tester.pump();
      expect(rowTop(tester, 2), statusBar);
      await tester.tapAt(const Offset(200, statusBar + rowHeight / 2));
      await tester.pump();
      expect(rowState(tester, 2).taps, 2);
      for (var i = 3; i < 8; i++) {
        expect(rowState(tester, i).taps, 0);
      }
    });

    testWidgets('a vertical drag that starts on it moves the sheet', (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
      );
      final start = sheet.extent;
      await tester.timedDragFrom(
        const Offset(200, _aboveFooter + rowHeight / 2),
        const Offset(0, -200),
        const Duration(seconds: 1),
      );
      await tester.pumpAndSettle();
      expect(sheet.extent, greaterThan(start + 150));
      expect(sheet.restingIn, freeRange);
      expect(rowState(tester, 2).taps, 0);

      // And down again, to the anchor below.
      await tester.flingFrom(
        tester.getCenter(row(2)),
        const Offset(0, 200),
        2000,
      );
      await tester.pumpAndSettle();
      expect(sheet.extent, lessThanOrEqualTo(start));
    });

    testWidgets('follows the sheet and the scroll without building, laying '
        'out or painting its child', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
      );
      final state = rowState(tester, 2);
      final (builds, layouts, paints) = (
        state.builds,
        state.layouts,
        state.paints,
      );
      state.displacements.clear();
      // Up to full height, and on into the scroll until it stops at the top.
      final gesture = await tester.startGesture(tester.getCenter(row(2)));
      for (var i = 0; i < 60; i++) {
        await gesture.moveBy(const Offset(0, -20));
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await tester.pumpAndSettle();
      expect(sheet.extent, screen.height);
      expect(_scroll(tester).pixels, greaterThan(300));
      expect(rowTop(tester, 2), statusBar);
      expect(
        (state.builds, state.layouts, state.paints),
        (builds, layouts, paints),
      );
      // Held up from below, in place, then held down from above.
      expect(state.displacements.first, greaterThan(0));
      expect(state.displacements, contains(0));
      expect(state.displacements.last, lessThan(0));
    });

    testWidgets('is as wide as the content in a wide window', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
        size: const Size(1000, 800),
      );
      expect(sheet.restingAt, headerAnchor);
      expect(
        tester.getRect(row(2)),
        const Rect.fromLTRB(16, _aboveFooter, 416, footerTop),
      );
      expect(painted(tester, 2)!.offset, const Offset(16, _aboveFooter));
      await moveTo(tester, sheet, footerAnchor);
      expect(
        tester.getRect(row(2)),
        const Rect.fromLTRB(16, _aboveFooter, 416, footerTop),
      );
      expect(contentTop(sheet.extent), _aboveFooter);
      // The top of the column, where the content starts below the handle.
      await moveTo(tester, sheet, full);
      _scroll(tester).jumpTo(300);
      await tester.pump();
      expect(rowTop(tester, 2), contentTop(sheet.extent));
    });

    testWidgets('without a footer, stops above the navigation bar', (
      tester,
    ) async {
      await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2, footer: false),
      );
      expect(tester.getRect(row(2)).bottom, screen.height - navigationBar);
    });

    testWidgets('outside a slot, nothing is displaced', (tester) async {
      await pumpHarness(tester, (key) => Harness(key: key, held: 2));
      final context = tester.element(find.text('header'));
      expect(SheetSlot.displacementOf(context).value, 0);
    });
  });

  group('when the held slot changes', () {
    testWidgets('holds from the first frame, with nothing to show for it', (
      tester,
    ) async {
      await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 5),
        settle: false,
      );
      expect(rowTop(tester, 5), _aboveFooter);
      expect(painted(tester, 5)!.opacity, 1);
      await tester.pump(const Duration(milliseconds: 100));
      expect(rowTop(tester, 5), _aboveFooter);
    });

    testWidgets('a row out of view rises from behind the footer', (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 1, initialSection: #three),
      );
      final extent = sheet.extent;
      final header = _headerBottom(tester);
      final anchors = sheet.anchors;
      expect(rowTop(tester, 1), slotTop(extent, 1));

      await _hold(tester, 8);
      // From where it is in the list.
      expect(rowTop(tester, 8), slotTop(extent, 8));
      var last = rowTop(tester, 8);
      for (var i = 0; i < 4; i++) {
        await tester.pump(const Duration(milliseconds: 40));
        final top = rowTop(tester, 8);
        expect(top, inExclusiveRange(_aboveFooter, last));
        expect(painted(tester, 8)!.opacity, 1);
        last = top;
        // The row held before is where it was, and nothing else has moved.
        expect(rowTop(tester, 1), slotTop(extent, 1));
        expect(painted(tester, 1)!.opacity, 1);
        expect(_headerBottom(tester), header);
        expect(sheet.extent, extent);
        expect(sheet.anchors, anchors);
      }
      await tester.pump(const Duration(milliseconds: 40));
      expect(rowTop(tester, 8), _aboveFooter);
      await tester.pumpAndSettle();
      expect(rowTop(tester, 8), _aboveFooter);
      expect(rowTop(tester, 1), slotTop(extent, 1));
      expect(sheet.extent, extent);
      expect(
        rowState(tester, 8).displacement,
        slotTop(extent, 8) - _aboveFooter,
      );
      expect(rowState(tester, 1).displacement, 0);
    });

    testWidgets('a held row slides back behind the footer when a row in view '
        'is held', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 8, initialSection: #three),
      );
      final extent = sheet.extent;
      final header = _headerBottom(tester);
      expect(rowTop(tester, 8), _aboveFooter);
      expect(slotTop(extent, 1), lessThan(_aboveFooter));

      await _hold(tester, 1);
      expect(rowTop(tester, 8), _aboveFooter);
      var last = _aboveFooter;
      for (var i = 0; i < 4; i++) {
        await tester.pump(const Duration(milliseconds: 40));
        final top = rowTop(tester, 8);
        expect(top, inExclusiveRange(last, slotTop(extent, 8)));
        last = top;
        expect(rowTop(tester, 1), slotTop(extent, 1));
        expect(_headerBottom(tester), header);
        expect(sheet.extent, extent);
      }
      await tester.pumpAndSettle();
      expect(rowTop(tester, 8), slotTop(extent, 8));
      expect(rowTop(tester, 1), slotTop(extent, 1));
      expect(rowState(tester, 8).displacement, 0);
      expect(sheet.extent, extent);
    });

    testWidgets('a row cut off by the footer rises to sit whole above it', (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(
          key: key,
          held: 0,
          initialSection: #three,
          initialMaxFraction: 0.5,
        ),
      );
      final extent = sheet.extent;
      expect(extent, screen.height / 2);
      final slot = slotTop(extent, 3);
      expect(slot, inExclusiveRange(_aboveFooter, footerTop));

      await _hold(tester, 3);
      expect(rowTop(tester, 3), slot);
      await tester.pump(const Duration(milliseconds: 100));
      expect(rowTop(tester, 3), inExclusiveRange(_aboveFooter, slot));
      expect(painted(tester, 3)!.opacity, 1);
      for (var i = 0; i < 3; i++) {
        expect(rowTop(tester, i), slotTop(extent, i));
      }
      await tester.pumpAndSettle();
      // Whole above the footer, over part of the row above; the list hasn't moved.
      expect(rowTop(tester, 3), _aboveFooter);
      expect(rowTop(tester, 2), slotTop(extent, 2));
      expect(sheet.extent, extent);
    });

    testWidgets('rows both wholly behind the footer fade one into the other', (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
      );
      await moveTo(tester, sheet, footerAnchor);
      final extent = sheet.extent;

      await _hold(tester, 5);
      expect(painted(tester, 2), (
        offset: const Offset(0, _aboveFooter),
        opacity: 1.0,
      ));
      for (var i = 0; i < 4; i++) {
        await tester.pump(const Duration(milliseconds: 40));
        final from = painted(tester, 2)!;
        final to = painted(tester, 5)!;
        // Both where the held row sits, neither moving.
        expect(from.offset, const Offset(0, _aboveFooter));
        expect(to.offset, const Offset(0, _aboveFooter));
        expect(from.opacity, inExclusiveRange(0, 1));
        expect(to.opacity, inExclusiveRange(0, 1));
        expect(from.opacity + to.opacity, moreOrLessEquals(1, epsilon: 0.01));
        expect(sheet.extent, extent);
      }
      await tester.pumpAndSettle();
      expect(rowTop(tester, 5), _aboveFooter);
      expect(painted(tester, 5)!.opacity, 1);
      // Back in the list, behind the footer.
      expect(rowTop(tester, 2), slotTop(extent, 2));
      expect(painted(tester, 2)!.opacity, 1);
      expect(sheet.extent, extent);
      expect(sheet.restingAt, footerAnchor);
    });

    testWidgets('rows raised into view while they fade are solid in their '
        'slots', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 2),
      );
      await moveTo(tester, sheet, footerAnchor);
      await _hold(tester, 5);
      await tester.pump(const Duration(milliseconds: 40));
      expect(painted(tester, 2)!.opacity, inExclusiveRange(0, 1));

      // Up to full height before the fade is over: each row is always painted, in
      // one place, where its slot is or else above the footer, and solid in its slot.
      sheet.animateTo(full);
      var raised = false;
      for (var i = 0; i < 40; i++) {
        await tester.pump(const Duration(milliseconds: 16));
        // Below where the sheet starts turning into a full screen.
        if (sheet.extent > 740) break;
        for (final i in [2, 5]) {
          final slot = slotTop(sheet.extent, i);
          expect(
            rowTop(tester, i),
            moreOrLessEquals(slot < _aboveFooter ? slot : _aboveFooter),
          );
          if (slot <= _aboveFooter) {
            expect(painted(tester, i)!.opacity, 1);
            raised = true;
          }
        }
      }
      expect(raised, isTrue);
      await tester.pumpAndSettle();
      expect(sheet.extent, screen.height);
      for (final i in [2, 5]) {
        expect(rowTop(tester, i), statusBar + headerHeight + i * rowHeight);
        expect(painted(tester, i)!.opacity, 1);
        expect(rowState(tester, i).displacement, 0);
      }
    });

    testWidgets('rows both wholly above the top fade one into the other', (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 0),
      );
      await moveTo(tester, sheet, full);
      _scroll(tester).jumpTo(400);
      await tester.pumpAndSettle();
      expect(rowTop(tester, 0), statusBar);

      await _hold(tester, 1);
      await tester.pump(const Duration(milliseconds: 80));
      final from = painted(tester, 0)!;
      final to = painted(tester, 1)!;
      expect(from.offset, const Offset(0, statusBar));
      expect(to.offset, const Offset(0, statusBar));
      expect(from.opacity + to.opacity, moreOrLessEquals(1, epsilon: 0.01));
      expect(to.opacity, inExclusiveRange(0, 1));
      await tester.pumpAndSettle();
      expect(rowTop(tester, 1), statusBar);
      expect(painted(tester, 1)!.opacity, 1);
    });

    testWidgets('a row in place gives way to one that rises over it', (
      tester,
    ) async {
      // At the header the first row is held where its slot is.
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 0),
      );
      final extent = sheet.extent;
      expect(rowTop(tester, 0), _aboveFooter);
      expect(rowState(tester, 0).displacement, 0);

      await _hold(tester, 5);
      await tester.pump(const Duration(milliseconds: 80));
      expect(painted(tester, 0), (
        offset: const Offset(0, _aboveFooter),
        opacity: 1.0,
      ));
      expect(
        rowTop(tester, 5),
        inExclusiveRange(_aboveFooter, slotTop(extent, 5)),
      );
      expect(painted(tester, 5)!.opacity, 1);
      await tester.pumpAndSettle();
      expect(rowTop(tester, 5), _aboveFooter);
      expect(rowTop(tester, 0), _aboveFooter);
      expect(sheet.extent, extent);
    });

    testWidgets('a first hold after the first frame rises too', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, initialSection: #three),
      );
      final extent = sheet.extent;
      await _hold(tester, 8);
      // The header anchor has the held row under it now, and so the range's lower end
      // is higher: the sheet keeps its place in the range, as when any anchor moves.
      await tester.pump();
      expect(sheet.extent, isNot(extent));
      expect(rowTop(tester, 8), slotTop(sheet.extent, 8));
      await tester.pump(const Duration(milliseconds: 100));
      expect(
        rowTop(tester, 8),
        inExclusiveRange(_aboveFooter, slotTop(sheet.extent, 8)),
      );
      await tester.pumpAndSettle();
      expect(rowTop(tester, 8), _aboveFooter);
      expect(sheet.restingIn, freeRange);
    });

    testWidgets('holding none puts the row back in its slot', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 8, initialSection: #three),
      );
      expect(rowTop(tester, 8), _aboveFooter);
      await _hold(tester, null);
      await tester.pump(const Duration(milliseconds: 100));
      expect(
        rowTop(tester, 8),
        inExclusiveRange(_aboveFooter, slotTop(sheet.extent, 8)),
      );
      await tester.pumpAndSettle();
      expect(rowTop(tester, 8), slotTop(sheet.extent, 8));
      expect(rowState(tester, 8).displacement, 0);
    });

    testWidgets('a change back before the first has finished turns round '
        'where it is', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 1, initialSection: #three),
      );
      final extent = sheet.extent;
      await _hold(tester, 8);
      await tester.pump(const Duration(milliseconds: 60));
      final risen = rowTop(tester, 8);
      await _hold(tester, 1);
      expect(rowTop(tester, 8), risen);
      await tester.pump(const Duration(milliseconds: 60));
      expect(rowTop(tester, 8), inExclusiveRange(risen, slotTop(extent, 8)));
      expect(rowTop(tester, 1), slotTop(extent, 1));
      await tester.pumpAndSettle();
      expect(rowTop(tester, 8), slotTop(extent, 8));
      expect(rowTop(tester, 1), slotTop(extent, 1));
    });
  });

  group('anchors', () {
    double extentOf(SheetController sheet, SheetAnchor anchor) =>
        sheet.anchors.firstWhere((a) => a.anchor == anchor).extent;

    testWidgets('rest with the held row and the footer under what they show', (
      tester,
    ) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 5),
      );
      const bare = navigationBar + handle;
      expect(extentOf(sheet, footerAnchor), bare + footerHeight + rowHeight);
      expect(
        extentOf(sheet, headerAnchor),
        bare + footerHeight + rowHeight + headerHeight,
      );

      await _hold(tester, null);
      await tester.pumpAndSettle();
      expect(extentOf(sheet, footerAnchor), bare + footerHeight);
      expect(extentOf(sheet, headerAnchor), bare + footerHeight + headerHeight);

      harness.currentState!.change(() => harness.currentState!.footer = false);
      await tester.pumpAndSettle();
      expect(extentOf(sheet, footerAnchor), bare);
      expect(extentOf(sheet, headerAnchor), bare + headerHeight);

      await _hold(tester, 5);
      await tester.pumpAndSettle();
      expect(extentOf(sheet, footerAnchor), bare + rowHeight);
      expect(extentOf(sheet, headerAnchor), bare + rowHeight + headerHeight);
      // The sheet rested at the header throughout.
      expect(sheet.restingAt, headerAnchor);
      expect(sheet.extent, extentOf(sheet, headerAnchor));
    });

    testWidgets('only count a held slot at or below what they show', (
      tester,
    ) async {
      const three = SheetAnchor.section(#three);
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(
          key: key,
          held: 1,
          anchors: const [footerAnchor, headerAnchor, three, full],
          freeRanges: const [],
        ),
      );
      const inset = footerHeight + navigationBar + handle;
      const shown = headerHeight + 4 * rowHeight;
      // Above the section's end, the held row shows in place.
      expect(extentOf(sheet, three), shown + inset);
      expect(extentOf(sheet, headerAnchor), headerHeight + rowHeight + inset);
      await moveTo(tester, sheet, three);
      expect(rowTop(tester, 1), slotTop(sheet.extent, 1));
      expect(tester.getRect(row(3)).bottom, footerTop);

      // The section's own slot starts above its end too.
      await _hold(tester, 3);
      await tester.pumpAndSettle();
      expect(extentOf(sheet, three), shown + inset);

      // Below it, the held row is under the section, above the footer.
      await _hold(tester, 5);
      await tester.pumpAndSettle();
      expect(extentOf(sheet, three), shown + rowHeight + inset);
      expect(sheet.restingAt, three);
      expect(sheet.extent, shown + rowHeight + inset);
      expect(tester.getRect(row(3)).bottom, _aboveFooter);
      expect(rowTop(tester, 5), _aboveFooter);
    });

    testWidgets('stay where they are as the hold changes between slots of one '
        'height below them', (tester) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(key: key, held: 5),
      );
      final anchors = sheet.anchors;
      final ranges = sheet.ranges;
      final extent = sheet.extent;
      expect(anchors, hasLength(2));
      await _hold(tester, 8);
      for (var i = 0; i < 8; i++) {
        expect(sheet.anchors, anchors);
        expect(sheet.ranges, ranges);
        expect(sheet.extent, extent);
        await tester.pump(const Duration(milliseconds: 40));
      }
      await tester.pumpAndSettle();
      expect(sheet.anchors, anchors);
      expect(sheet.ranges, ranges);
      expect(sheet.extent, extent);

      // A taller one moves them.
      harness.currentState!.change(
        () => harness.currentState!.heights = {9: 90},
      );
      await _hold(tester, 9);
      await tester.pumpAndSettle();
      expect(extentOf(sheet, headerAnchor), extent + 30);
      expect(sheet.extent, extent + 30);
      expect(tester.getRect(row(9)).bottom, footerTop);
      expect(rowTop(tester, 9), _headerBottom(tester));
    });

    testWidgets('are ends of a free range as any others', (tester) async {
      const range = SheetRange(footerAnchor, headerAnchor);
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(
          key: key,
          held: 2,
          anchors: const [footerAnchor],
          freeRanges: const [range],
          initialAnchor: 0,
        ),
      );
      const bare = navigationBar + handle + footerHeight + rowHeight;
      expect(sheet.restingAt, footerAnchor);
      expect(sheet.extent, bare);
      expect(sheet.ranges.single.lower, bare);
      expect(sheet.ranges.single.upper, bare + headerHeight);
      await tester.timedDragFrom(
        tester.getCenter(row(2)),
        const Offset(0, -70),
        const Duration(seconds: 1),
      );
      await tester.pumpAndSettle();
      expect(sheet.restingIn, range);
      expect(sheet.extent, inExclusiveRange(bare, bare + headerHeight));
      expect(rowTop(tester, 2), _aboveFooter);
    });
  });
}
