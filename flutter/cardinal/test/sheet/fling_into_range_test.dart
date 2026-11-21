import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import 'held_harness.dart';

/// The header anchor, holding lightly enough for an ordinary fling to pass it.
const _waypoint = SheetAnchor.section(#header, gravity: 1200);
const _range = SheetRange(_waypoint, full);
const _carrying = SheetRange(_waypoint, full, carriesIntoScroll: true);

/// Where the anchors rest: the footer under the handle, and the header over it.
const _low = navigationBar + handle + footerHeight;
const _high = _low + headerHeight;

const _frame = Duration(milliseconds: 16);

/// The sheet's physics in these tests: a fling slows as Android's does.
const _physics = SheetPhysics();

Future<SheetController> _pump(
  WidgetTester tester, {
  SheetRange range = _range,
  int initialAnchor = 0,
}) async {
  final sheet = await pumpHarness(
    tester,
    (key) => Harness(
      key: key,
      anchors: const [footerAnchor, _waypoint],
      freeRanges: [range],
      initialAnchor: initialAnchor,
    ),
  );
  expect(sheet.anchors.map((a) => a.extent), [_low, _high]);
  expect(sheet.ranges.single.lower, _high);
  expect(sheet.ranges.single.upper, screen.height);
  return sheet;
}

ScrollPosition _scroll(WidgetTester tester) =>
    tester.state<ScrollableState>(find.byType(Scrollable).last).position;

/// Lets go of the sheet where it is at [velocity], and says where it was each frame
/// from then until it came to rest.
Future<List<double>> _release(
  WidgetTester tester,
  SheetController sheet,
  double velocity,
) async {
  sheet
    ..hold()
    ..release(velocity);
  final extents = [sheet.extent];
  // The frame its motion starts on.
  await tester.pump();
  for (var i = 0; i < 600 && !sheet.isResting; i++) {
    await tester.pump(_frame);
    extents.add(sheet.extent);
  }
  expect(sheet.isResting, isTrue);
  return extents;
}

/// Flings from [from] by a finger, and says where the sheet was each frame from when
/// it was let go until it came to rest.
Future<List<double>> _fling(
  WidgetTester tester,
  SheetController sheet,
  Offset from,
  Offset offset,
  double speed,
) async {
  await tester.flingFrom(from, offset, speed);
  final extents = [sheet.extent];
  // The frame its motion starts on.
  await tester.pump();
  for (var i = 0; i < 600 && !sheet.isResting; i++) {
    await tester.pump(_frame);
    extents.add(sheet.extent);
  }
  expect(sheet.isResting, isTrue);
  return extents;
}

/// Expects [extents], a frame apart, to be one motion [direction]wards (1 up, -1 down)
/// that only slows: never back, never still before the end, and never faster from one
/// frame to the next.
void _expectOneMotion(List<double> extents, double direction) {
  var last = double.infinity;
  for (var i = 1; i < extents.length; i++) {
    final step = (extents[i] - extents[i - 1]) * direction;
    expect(step, greaterThanOrEqualTo(0), reason: 'frame $i goes back');
    expect(step, lessThanOrEqualTo(last + 0.01), reason: 'frame $i speeds up');
    if (i < extents.length - 2) {
      expect(step, greaterThan(0), reason: 'frame $i stands still');
    }
    last = step;
  }
}

void main() {
  group('a fling up from the lowest anchor', () {
    testWidgets('that reaches the header faster than it holds glides on into '
        'the range, as one coast from where it was let go', (tester) async {
      final sheet = await _pump(tester);
      const speed = 1500.0;
      final landing = _physics.landing(_low, speed);
      expect(landing, inExclusiveRange(_high + 100, screen.height - 100));
      expect(_physics.speedAt(_low, speed, _high), greaterThan(1200));

      final extents = await _release(tester, sheet, speed);
      expect(sheet.restingIn, _range);
      expect(sheet.restingAt, isNull);
      expect(sheet.extent, moreOrLessEquals(landing, epsilon: 0.5));

      // The fling's own coast all the way: over the anchor at the speed it has there.
      final coast = _physics.coast(_low, speed);
      for (var i = 0; i < extents.length - 1; i++) {
        expect(
          extents[i],
          moreOrLessEquals(coast.x(i * 0.016), epsilon: 0.01),
          reason: 'frame $i',
        );
      }
      _expectOneMotion(extents, 1);
      expect(extents.where((e) => e < _high), isNotEmpty);
      expect(extents.where((e) => e > _high && e < landing), isNotEmpty);
    });

    testWidgets('by a finger does the same', (tester) async {
      final sheet = await _pump(tester);
      final extents = await _fling(
        tester,
        sheet,
        const Offset(200, footerTop + footerHeight / 2),
        const Offset(0, -60),
        1800,
      );
      expect(sheet.restingIn, _range);
      expect(sheet.extent, inExclusiveRange(_high + 50, screen.height - 50));
      _expectOneMotion(extents, 1);
      expect(_scroll(tester).pixels, 0);
    });

    testWidgets('that reaches the header slower than it holds stops there', (
      tester,
    ) async {
      final sheet = await _pump(tester);
      const speed = 1300.0;
      expect(_physics.landing(_low, speed), greaterThan(_high + 50));
      expect(_physics.speedAt(_low, speed, _high), lessThan(1200));
      await _release(tester, sheet, speed);
      expect(sheet.restingAt, _waypoint);
      expect(sheet.restingIn, isNull);
      expect(sheet.extent, _high);
    });

    testWidgets('that runs out before the header stops there', (tester) async {
      final sheet = await _pump(tester);
      expect(_physics.landing(_low, 600), lessThan(_high));
      await _release(tester, sheet, 600);
      expect(sheet.restingAt, _waypoint);
      expect(sheet.extent, _high);
    });

    testWidgets('let go slowly settles on the nearer anchor', (tester) async {
      final sheet = await _pump(tester);
      sheet
        ..hold()
        ..dragBy(30)
        ..release(100);
      await tester.pumpAndSettle();
      expect(sheet.restingAt, footerAnchor);
    });

    testWidgets("that would carry past the range's top stops at the top", (
      tester,
    ) async {
      final sheet = await _pump(tester);
      expect(_physics.landing(_low, 3000), greaterThan(screen.height));
      final extents = await _release(tester, sheet, 3000);
      expect(sheet.restingIn, _range);
      expect(sheet.extent, screen.height);
      _expectOneMotion(extents, 1);
      expect(_scroll(tester).pixels, 0);
    });

    testWidgets('that would carry past the top of a range that carries into '
        'scroll scrolls the content on with the speed left', (tester) async {
      final sheet = await _pump(tester, range: _carrying);
      final extents = <double>[];
      await tester.flingFrom(
        const Offset(200, footerTop + footerHeight / 2),
        const Offset(0, -60),
        3000,
      );
      // What shows moves by the sheet rising, and then by its content scrolling.
      double shown() => sheet.extent + _scroll(tester).pixels;
      extents.add(shown());
      // The frame its motion starts on.
      await tester.pump();
      // The frame the sheet reaches the top on, where the content takes over.
      int? handover;
      for (var i = 1; i < 600; i++) {
        await tester.pump(_frame);
        extents.add(shown());
        if (sheet.isResting) handover ??= i;
        if (sheet.isResting && !_scroll(tester).isScrollingNotifier.value) {
          break;
        }
      }
      expect(sheet.restingIn, _carrying);
      expect(sheet.extent, screen.height);
      expect(_scroll(tester).pixels, greaterThan(100));
      // One motion over the anchor, over the top and on into the list, only slowing:
      // the frame the content takes over on is a step short of a whole one, as it is
      // for a fling from within the range, so the next is longer than that one, but
      // no longer than the one before it.
      for (var i = 2; i < extents.length - 2; i++) {
        final step = extents[i] - extents[i - 1];
        final before = i == handover! + 1 ? i - 2 : i - 1;
        expect(step, greaterThan(0), reason: 'frame $i stands still');
        expect(
          step,
          lessThanOrEqualTo(extents[before] - extents[before - 1] + 0.01),
          reason: 'frame $i speeds up',
        );
      }
    });
  });

  group('a fling down from the range', () {
    Future<SheetController> pumpInRange(WidgetTester tester) async {
      final sheet = await _pump(tester, initialAnchor: 1);
      sheet
        ..hold()
        ..dragBy(300)
        ..release(0);
      await tester.pumpAndSettle();
      expect(sheet.restingIn, _range);
      expect(sheet.extent, _high + 300);
      return sheet;
    }

    testWidgets('that reaches the header faster than it holds goes on to the '
        'lowest anchor', (tester) async {
      final sheet = await pumpInRange(tester);
      expect(_physics.speedAt(_high + 300, -3000, _high), greaterThan(1200));
      await _release(tester, sheet, -3000);
      expect(sheet.restingAt, footerAnchor);
      expect(sheet.extent, _low);
    });

    testWidgets('that reaches the header slower than it holds stops there', (
      tester,
    ) async {
      final sheet = await pumpInRange(tester);
      expect(_physics.landing(_high + 300, -1500), lessThan(_high));
      expect(_physics.speedAt(_high + 300, -1500, _high), lessThan(1200));
      await _release(tester, sheet, -1500);
      expect(sheet.restingAt, _waypoint);
      expect(sheet.extent, _high);
    });

    testWidgets('that runs out within the range stops where it runs out', (
      tester,
    ) async {
      final sheet = await pumpInRange(tester);
      final landing = _physics.landing(_high + 300, -1000);
      expect(landing, greaterThan(_high));
      await _release(tester, sheet, -1000);
      expect(sheet.restingIn, _range);
      expect(sheet.extent, moreOrLessEquals(landing, epsilon: 0.5));
    });
  });

  group('a fling down into a range below', () {
    // The header is the top of a range up from the footer, under an anchor at full.
    // It's a short range, so only a header that holds very lightly lets by a fling
    // that then runs out within it.
    const light = SheetAnchor.section(#header, gravity: 300);

    Future<SheetController> pumpAbove(
      WidgetTester tester, [
      SheetAnchor header = _waypoint,
    ]) async {
      final sheet = await pumpHarness(
        tester,
        (key) => Harness(
          key: key,
          anchors: [header, full],
          freeRanges: [SheetRange(footerAnchor, header)],
          initialAnchor: 1,
          // Nothing to scroll, so the sheet is all a fling moves.
          rows: 0,
        ),
      );
      expect(sheet.restingAt, full);
      expect(sheet.ranges.single.lower, _low);
      expect(sheet.ranges.single.upper, _high);
      return sheet;
    }

    const below = SheetRange(footerAnchor, _waypoint);

    testWidgets('that passes the anchor at its top lands in it', (
      tester,
    ) async {
      final sheet = await pumpAbove(tester, light);
      const speed = -2000.0;
      final landing = _physics.landing(screen.height, speed);
      expect(landing, inExclusiveRange(_low + 10, _high - 10));
      expect(_physics.speedAt(screen.height, speed, _high), greaterThan(300));
      final extents = await _release(tester, sheet, speed);
      expect(sheet.restingIn, const SheetRange(footerAnchor, light));
      expect(sheet.extent, moreOrLessEquals(landing, epsilon: 0.5));
      _expectOneMotion(extents, -1);
      final coast = _physics.coast(screen.height, speed);
      for (var i = 0; i < extents.length - 1; i++) {
        expect(
          extents[i],
          moreOrLessEquals(coast.x(i * 0.016), epsilon: 0.01),
          reason: 'frame $i',
        );
      }
    });

    testWidgets('that the anchor at its top catches stops there', (
      tester,
    ) async {
      final sheet = await pumpAbove(tester);
      expect(_physics.landing(screen.height, -2000), lessThan(_high));
      expect(_physics.speedAt(screen.height, -2000, _high), lessThan(1200));
      await _release(tester, sheet, -2000);
      expect(sheet.restingAt, _waypoint);
    });

    testWidgets('that would carry past its bottom runs on past it, and comes '
        'back', (tester) async {
      final sheet = await pumpAbove(tester);
      expect(_physics.landing(screen.height, -4000), lessThan(_low));
      final extents = await _release(tester, sheet, -4000);
      expect(sheet.restingIn, below);
      expect(sheet.extent, _low);
      expect(extents.reduce((a, b) => a < b ? a : b), lessThan(_low - 5));
    });
  });
}
