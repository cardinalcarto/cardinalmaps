import 'package:cardinal/directions/elevation_profile.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/speed_profile.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_routes.dart';
import 'picker_harness.dart';

/// The rows of the directions sheet with what Valinor says of each route's roads:
/// their kinds, where they're tolled, and the ground they go over.
void main() {
  late Picker picker;

  setUp(() {
    picker = Picker();
    picker.routes.immediatelyBy
      ..[TravelMode.driving] = detailedDrivingRoutes
      ..[TravelMode.cycling] = detailedCyclingRoutes
      ..[TravelMode.walking] = detailedWalkingRoutes;
  });

  Finder inRow(int index, Finder finder) =>
      find.descendant(of: rows.at(index), matching: finder);

  group('a drive', () {
    testWidgets('has "Toll" on a tolled stretch wide enough, and then doesn\'t '
        'say "Tolls"', (tester) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      expect(inRow(1, find.text(tollLabel)), findsOneWidget);
      expect(
        inRow(1, find.text('14 mi · 3 min slower · Shortest')),
        findsOneWidget,
      );
      // The word is on the stretch: the middle 10 of its 25 minutes.
      final profile = tester.getRect(inRow(1, find.byType(SpeedProfile)));
      final word = tester.getRect(inRow(1, find.text(tollLabel)));
      expect(
        (word.center.dx - profile.left) / profile.width,
        moreOrLessEquals(10 / 25, epsilon: 0.01),
      );
      expect(word.top, greaterThan(profile.top));
      expect(word.bottom, lessThanOrEqualTo(profile.bottom));
      // Screen readers, which aren't shown the word, are told.
      expect(
        tester.getSemantics(rows.at(1)).label,
        endsWith('14 mi, 3 min slower, Shortest, Tolls'),
      );
    });

    testWidgets('says "Tolls" where the stretch is too short for the word', (
      tester,
    ) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      // A bridge of 12 seconds, and a ferry, which is tolled but no road.
      expect(inRow(2, find.text(tollLabel)), findsNothing);
      expect(
        inRow(
          2,
          find.text('19 mi · 23 min slower · Ferry · No freeways · Tolls'),
        ),
        findsOneWidget,
      );
    });

    testWidgets('and in a small window, where the stretch is narrower', (
      tester,
    ) async {
      await picker.open(tester, size: const Size(320, 568));
      await moveTo(tester, const SheetAnchor.full());
      expect(find.text(tollLabel), findsNothing);
      expect(
        inRow(1, find.text('14 mi · 3 min slower · Shortest · Tolls')),
        findsOneWidget,
      );
    });

    testWidgets('without the road details, never has the word', (tester) async {
      picker.routes.immediatelyBy[TravelMode.driving] = drivingRoutes;
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      expect(find.text(tollLabel), findsNothing);
      expect(
        inRow(1, find.text('14 mi · 3 min slower · Tolls')),
        findsOneWidget,
      );
    });
  });

  group('a ride', () {
    testWidgets('says how far each climbs, which climbs least, and which is '
        'mostly trails', (tester) async {
      await picker.open(tester);
      await showMode(tester, TravelMode.cycling);
      await moveTo(tester, const SheetAnchor.full());
      expect(
        inRow(0, find.text('16 mi · Fastest · 490 ft climb')),
        findsOneWidget,
      );
      expect(
        inRow(
          1,
          find.text(
            '16 mi · 6 min slower · Least climbing, 130 ft · Mostly trails',
          ),
        ),
        findsOneWidget,
      );
      expect(
        inRow(2, find.text('15 mi · 13 min slower · Shortest · 920 ft climb')),
        findsOneWidget,
      );
    });

    testWidgets('draws each one\'s ground to the same scale, along its time', (
      tester,
    ) async {
      await picker.open(tester);
      await showMode(tester, TravelMode.cycling);
      await moveTo(tester, const SheetAnchor.full());
      final profiles = tester
          .widgetList<ElevationProfile>(find.byType(ElevationProfile))
          .toList();
      expect(profiles, hasLength(3));
      for (final profile in profiles) {
        expect(profile.range, (low: 10, high: 290));
        expect(profile.elevation, isNotNull);
      }
      final widths = [
        for (final element in find.byType(ElevationProfile).evaluate())
          element.size!.width,
      ];
      expect(widths[0] / widths[2], moreOrLessEquals(4680 / 5460));
    });
  });

  group('a walk', () {
    testWidgets('says its stairs and its climb, and nothing of its paths', (
      tester,
    ) async {
      await picker.open(tester);
      await showMode(tester, TravelMode.walking);
      await moveTo(tester, const SheetAnchor.full());
      expect(
        inRow(
          0,
          find.text('1.3 mi · Fastest · Shortest · 80 ft climb · Stairs'),
        ),
        findsOneWidget,
      );
      expect(
        inRow(1, find.text('1.4 mi · 3 min slower · Least climbing')),
        findsOneWidget,
      );
      expect(find.textContaining('trails'), findsNothing);
    });
  });

  group('the legend', () {
    Finder legend() => find.byType(WayLegend);

    for (final (mode, names) in [
      (TravelMode.driving, ['Freeway', 'Main roads', 'Streets']),
      (TravelMode.cycling, ['Trail', 'Bike lane', 'Shared road']),
      (TravelMode.walking, ['Path', 'Sidewalk', 'Stairs']),
    ]) {
      testWidgets('names the tones under the last row, $mode', (tester) async {
        await picker.open(tester);
        await showMode(tester, mode);
        await moveTo(tester, const SheetAnchor.full());
        expect(legend(), findsOneWidget);
        for (final name in names) {
          expect(
            find.descendant(of: legend(), matching: find.text(name)),
            findsOneWidget,
          );
        }
        expect(
          tester.getRect(legend()).top,
          greaterThanOrEqualTo(slotRects(tester).last.bottom),
        );
        // It describes the graphs, which screen readers aren't given.
        expect(find.bySemanticsLabel(names.first), findsNothing);
      });
    }

    testWidgets('isn\'t there without the kinds of way, or while loading', (
      tester,
    ) async {
      picker.routes.immediatelyBy[TravelMode.driving] = drivingRoutes;
      picker.routes.immediatelyBy.remove(TravelMode.cycling);
      await picker.open(tester);
      expect(legend(), findsNothing);
      await showMode(tester, TravelMode.cycling);
      expect(legend(), findsNothing);
      picker
          .asked(TravelMode.cycling)
          .single
          .answer
          .complete(detailedCyclingRoutes);
      await tester.pumpAndSettle();
      expect(legend(), findsOneWidget);
    });

    testWidgets('moves no row and no anchor', (tester) async {
      picker.answerNothing();
      await picker.open(tester);
      final sheet = sheetOf(tester);
      List<double> anchors() => [
        extentOf(sheet, footerAnchor),
        extentOf(sheet, headerAnchor),
      ];
      final loading = anchors();
      final row = OptionRowMetrics.heightOf(tester.element(tabs));
      picker
          .asked(TravelMode.driving)
          .single
          .answer
          .complete(detailedDrivingRoutes);
      await tester.pumpAndSettle();
      expect(legend(), findsOneWidget);
      expect(anchors(), loading);
      for (final slot in slots.evaluate()) {
        expect(slot.size!.height, row);
      }
      final before = slotRects(tester);
      // Nor does it change between ways that have one.
      await showMode(tester, TravelMode.walking);
      picker
          .asked(TravelMode.walking)
          .single
          .answer
          .complete(detailedWalkingRoutes);
      await tester.pumpAndSettle();
      expect(legend(), findsOneWidget);
      expect(anchors(), loading);
      expect(slotRects(tester), before.sublist(0, 2));
    });
  });

  testWidgets('paints no graph again as the sheet moves', (tester) async {
    await picker.open(tester);
    for (final (mode, graph) in [
      (TravelMode.driving, SpeedProfile),
      (TravelMode.cycling, ElevationProfile),
    ]) {
      await showMode(tester, mode);
      final boundaries = [
        for (final element
            in find
                .descendant(
                  of: find.byType(graph),
                  matching: find.byType(RepaintBoundary),
                )
                .evaluate())
          element.renderObject! as RenderRepaintBoundary,
      ];
      expect(boundaries, hasLength(3), reason: '$mode');
      for (final boundary in boundaries) {
        boundary.debugResetMetrics();
      }
      final gesture = await tester.startGesture(tester.getCenter(tabs));
      for (var i = 0; i < 20; i++) {
        await gesture.moveBy(const Offset(0, 20));
        await tester.pump(const Duration(milliseconds: 16));
      }
      for (var i = 0; i < 40; i++) {
        await gesture.moveBy(const Offset(0, -20));
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await tester.pumpAndSettle();
      for (final boundary in boundaries) {
        expect(boundary.debugSymmetricPaintCount, 0, reason: '$mode');
        expect(boundary.debugAsymmetricPaintCount, 0, reason: '$mode');
      }
    }
  });
}
