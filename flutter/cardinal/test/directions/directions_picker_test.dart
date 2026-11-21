import 'package:cardinal/directions/elevation_profile.dart';
import 'package:cardinal/directions/mode_tabs.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/speed_profile.dart';
import 'package:cardinal/directions/transit_timeline.dart';
import 'package:cardinal/directions/trip_card.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/ui/adaptive.dart';
import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoListSection;
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart' show LocationPermission;
import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_location.dart';
import '../fakes/fake_transit.dart';
import 'picker_harness.dart';

void main() {
  late Picker picker;

  setUp(() => picker = Picker());

  Finder tabText(String text) =>
      find.descendant(of: tabs, matching: find.text(text));

  /// The row of the list at [index], wherever it's painted.
  Rect rowRect(WidgetTester tester, int index) =>
      tester.getRect(rows.at(index));

  group('every way is asked at once', () {
    testWidgets('and the sheet is built before any answers', (tester) async {
      picker.answerNothing();
      await picker.open(tester);
      expect(picker.routes.asked.map((question) => question.mode), [
        TravelMode.driving,
        TravelMode.walking,
        TravelMode.cycling,
      ]);
      expect(picker.transit.asked, hasLength(1));

      // The ends, the tabs with no times yet, three placeholders, and the buttons,
      // which have nothing to act on.
      expect(find.text('My Location'), findsOneWidget);
      expect(find.text('Museum of Flight'), findsOneWidget);
      for (final mode in TravelMode.values) {
        expect(tabTime(tester, mode), isNull);
      }
      expect(find.byType(OptionPlaceholder), findsNWidgets(3));
      expect(rows, findsNothing);
      expect(
        tester.widget<FilledButton>(find.widgetWithText(FilledButton, 'Start')),
        isA<FilledButton>().having((b) => b.onPressed, 'onPressed', isNull),
      );
      expect(
        tester
            .widget<FilledButton>(find.widgetWithText(FilledButton, 'Steps'))
            .onPressed,
        isNull,
      );
    });

    testWidgets('each tab gets its time when its answer comes', (tester) async {
      picker.answerNothing();
      // The transit plan's journeys arrive 45 and 55 minutes after it's made.
      picker.now = DateTime.fromMillisecondsSinceEpoch(fakeStart * 1000);
      await picker.open(tester);
      final header = tester.getRect(tabs);

      picker.asked(TravelMode.cycling).single.answer.complete(cyclingRoutes);
      await tester.pump();
      await tester.pump();
      // It fades in, in the room its tab kept for it.
      await tester.pump(const Duration(milliseconds: 50));
      final fading = tester.widget<FadeTransition>(
        find
            .ancestor(
              of: tabText('1 hr 18 min'),
              matching: find.byType(FadeTransition),
            )
            .first,
      );
      expect(fading.opacity.value, inExclusiveRange(0, 1));
      await tester.pumpAndSettle();
      expect(tabText('1 hr 18 min'), findsOneWidget);
      expect(tabTime(tester, TravelMode.driving), isNull);
      expect(tester.getRect(tabs), header);
      // The way that's showing still waits.
      expect(find.byType(OptionPlaceholder), findsNWidgets(3));

      picker.transit.asked.single.answer.complete(fakePlan(journeys: 2));
      await tester.pumpAndSettle();
      expect(tabText('45 min'), findsOneWidget);

      picker.asked(TravelMode.driving).single.answer.complete(drivingRoutes);
      await tester.pumpAndSettle();
      expect(tabText('22 min'), findsOneWidget);
      expect(rows, findsNWidgets(4));
      expect(tabTime(tester, TravelMode.walking), isNull);
      expect(tester.getRect(tabs), header);
    });

    testWidgets('a way with nothing to offer shows a dash', (tester) async {
      picker.answerNothing();
      await picker.open(tester);
      final header = tester.getRect(tabs);
      picker
          .asked(TravelMode.walking)
          .single
          .answer
          .completeError(http.ClientException('offline'));
      picker.asked(TravelMode.cycling).single.answer.complete([]);
      picker.transit.asked.single.answer.complete(emptyPlan('no_service'));
      picker.asked(TravelMode.driving).single.answer.complete(drivingRoutes);
      await tester.pumpAndSettle();
      expect(tabTime(tester, TravelMode.walking), noTimeDash);
      expect(tabTime(tester, TravelMode.cycling), noTimeDash);
      expect(tabTime(tester, TravelMode.transit), noTimeDash);
      expect(tabText(noTimeDash), findsNWidgets(3));
      expect(tabText('22 min'), findsOneWidget);
      expect(tester.getRect(tabs), header);
    });

    testWidgets('transit\'s time counts down to its earliest arrival', (
      tester,
    ) async {
      picker.now = DateTime.fromMillisecondsSinceEpoch(fakeStart * 1000);
      await picker.open(tester);
      expect(tabText('45 min'), findsOneWidget);
      picker.now = picker.now.add(const Duration(minutes: 3));
      await tester.pump(const Duration(seconds: 31));
      await tester.pumpAndSettle();
      expect(tabText('42 min'), findsOneWidget);
    });

    testWidgets('an answer for an older trip is dropped', (tester) async {
      picker.answerNothing();
      await picker.open(tester);
      final old = picker.asked(TravelMode.driving).single;
      await tester.tap(find.byTooltip('Swap start and destination'));
      await tester.pump();
      expect(picker.asked(TravelMode.driving), hasLength(2));
      expect(picker.asked(TravelMode.driving).last.from, destination.center);

      old.answer.complete(drivingRoutes);
      await tester.pumpAndSettle();
      expect(rows, findsNothing);
      expect(find.byType(OptionPlaceholder), findsNWidgets(3));
      expect(tabTime(tester, TravelMode.driving), isNull);

      picker.asked(TravelMode.driving).last.answer.complete([viaStreets]);
      await tester.pumpAndSettle();
      expect(rows, findsOneWidget);
      expect(tabText('34 min'), findsOneWidget);
    });
  });

  group('a row', () {
    testWidgets(
      'says how long, when it arrives, by what roads, and what of it',
      (tester) async {
        await picker.open(tester);
        await moveTo(tester, const SheetAnchor.full());
        Finder inRow(int index, String text) =>
            find.descendant(of: rows.at(index), matching: find.text(text));
        expect(inRow(0, '22 min'), findsOneWidget);
        expect(inRow(0, 'Arrive 3:42 PM'), findsOneWidget);
        expect(inRow(0, 'via I-5 South'), findsOneWidget);
        expect(inRow(0, '14 mi · Fastest'), findsOneWidget);

        expect(inRow(1, '25 min'), findsOneWidget);
        expect(inRow(1, 'Arrive 3:45 PM'), findsOneWidget);
        expect(inRow(1, 'via SR 99 and SR 509'), findsOneWidget);
        expect(inRow(1, '14 mi · 3 min slower · Tolls'), findsOneWidget);

        expect(inRow(2, 'via East Marginal Way South'), findsOneWidget);
        expect(
          inRow(2, '13 mi · 12 min slower · Shortest · No freeways'),
          findsOneWidget,
        );

        // No roads to name it for: the line's room stays, so the row is as tall.
        expect(
          find.descendant(of: rows.at(3), matching: find.textContaining('via')),
          findsNothing,
        );
        expect(
          inRow(3, '19 mi · 23 min slower · Ferry · No freeways'),
          findsOneWidget,
        );
        expect(rowRect(tester, 3).height, rowRect(tester, 0).height);
      },
    );

    testWidgets('reads out all of it', (tester) async {
      await picker.open(tester);
      expect(
        tester.getSemantics(rows.at(1)),
        isSemantics(
          label:
              '25 min, Arrive 3:45 PM, via SR 99 and SR 509, 14 mi, '
              '3 min slower, Tolls',
          isButton: true,
          hasTapAction: true,
        ),
      );
    });

    testWidgets('of a walk or a ride says nothing a driver would ask', (
      tester,
    ) async {
      picker.routes.immediatelyBy[TravelMode.walking] = drivingRoutes;
      await picker.open(tester);
      await showMode(tester, TravelMode.walking);
      await moveTo(tester, const SheetAnchor.full());
      expect(find.text('14 mi · 3 min slower'), findsOneWidget);
      expect(find.text('13 mi · 12 min slower · Shortest'), findsOneWidget);
      // And its ground over a bar where a drive has its speed profile.
      expect(find.byType(SpeedProfile), findsNothing);
      expect(find.byType(ElevationProfile), findsNWidgets(4));
    });

    testWidgets('has a graph as wide as its share of the slowest\'s time', (
      tester,
    ) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      final widths = [
        for (final element in find.byType(SpeedProfile).evaluate())
          element.size!.width,
      ];
      expect(widths, hasLength(4));
      final full = widths.last;
      expect(widths[0], moreOrLessEquals(full * 1320 / 2700, epsilon: 0.01));
      expect(widths[1], moreOrLessEquals(full * 1500 / 2700, epsilon: 0.01));
      expect(widths[2], moreOrLessEquals(full * 2040 / 2700, epsilon: 0.01));
      for (final element in find.byType(SpeedProfile).evaluate()) {
        expect(element.size!.height, routeGraphHeight);
      }
      // Each to the same scale: the fastest any of them goes.
      for (final profile in tester.widgetList<SpeedProfile>(
        find.byType(SpeedProfile),
      )) {
        expect(profile.topSpeed, 27);
      }
    });

    testWidgets('selected is filled, its time in the primary color, and no '
        'other size', (tester) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      final scheme = Theme.of(tester.element(tabs)).colorScheme;
      Color fill(int index) =>
          (tester
                      .widget<AnimatedContainer>(
                        find.descendant(
                          of: rows.at(index),
                          matching: find.byType(AnimatedContainer),
                        ),
                      )
                      .decoration!
                  as BoxDecoration)
              .color!;
      Color? time(int index, String text) => tester
          .renderObject<RenderParagraph>(
            find.descendant(of: rows.at(index), matching: find.text(text)),
          )
          .text
          .style!
          .color;
      expect(fill(0), scheme.primaryContainer);
      expect(fill(1).a, 0);
      expect(time(0, '22 min'), scheme.primary);
      expect(time(1, '25 min'), scheme.onSurface);
      final before = [for (var i = 0; i < 4; i++) rowRect(tester, i)];
      final paragraphs = [
        for (final element
            in find
                .descendant(of: rows, matching: find.byType(RichText))
                .evaluate())
          tester.getRect(find.byElementPredicate((e) => e == element)),
      ];

      await tester.tap(rows.at(1));
      await tester.pumpAndSettle();
      expect(selectedRows(tester), [false, true, false, false]);
      expect(fill(1), scheme.primaryContainer);
      expect(fill(0).a, 0);
      expect(time(1, '25 min'), scheme.primary);
      expect([for (var i = 0; i < 4; i++) rowRect(tester, i)], before);
      expect([
        for (final element
            in find
                .descendant(of: rows, matching: find.byType(RichText))
                .evaluate())
          tester.getRect(find.byElementPredicate((e) => e == element)),
      ], paragraphs);
    });
  });

  group('selection', () {
    testWidgets('each way keeps its own while the trip is the same', (
      tester,
    ) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(rows.at(2));
      await tester.pumpAndSettle();
      await showMode(tester, TravelMode.cycling);
      expect(selectedRows(tester), [true, false, false]);
      await tester.tap(rows.at(1));
      await tester.pumpAndSettle();

      await showMode(tester, TravelMode.driving);
      expect(selectedRows(tester), [false, false, true, false]);
      await showMode(tester, TravelMode.cycling);
      expect(selectedRows(tester), [false, true, false]);
      expect(picker.routes.asked, hasLength(3), reason: 'nothing asked again');
    });

    testWidgets('a changed trip selects the first of each way again', (
      tester,
    ) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(rows.at(2));
      await tester.pumpAndSettle();
      await showMode(tester, TravelMode.cycling);
      await tester.tap(rows.at(1));
      await tester.pumpAndSettle();

      await tester.tap(find.byTooltip('Swap start and destination'));
      await tester.pumpAndSettle();
      expect(picker.routes.asked, hasLength(6));
      expect(selectedRows(tester), [true, false, false]);
      await showMode(tester, TravelMode.driving);
      expect(selectedRows(tester), [true, false, false, false]);
    });

    testWidgets('the map draws every route of the way, the selected one '
        'highlighted', (tester) async {
      await picker.open(tester);
      List<Object?> drawn() => [
        for (final feature
            in (picker.map.geoJson[routeSource]! as Map)['features'] as List)
          if (((feature as Map)['properties'] as Map)['role'] == 'line')
            (feature['properties'] as Map)['selected'],
      ];
      expect(drawn(), [true, false, false, false]);
      routeTaps.add(2);
      await tester.pumpAndSettle();
      expect(drawn(), [false, false, true, false]);
      expect(selectedRows(tester), [false, false, true, false]);
      await showMode(tester, TravelMode.walking);
      expect(drawn(), [true, false]);
    });
  });

  group('the sheet', () {
    testWidgets('opens on the header and the first rows, in at most 60% of '
        'the window', (tester) async {
      await picker.open(tester);
      final sheet = sheetOf(tester);
      expect(sheet.restingIn, isNotNull, reason: 'within the free range');
      expect(sheet.extent, moreOrLessEquals(appScreen.height * 0.6));
      expect(sheet.anchors.map((a) => a.anchor), [footerAnchor, headerAnchor]);
      expect(sheet.ranges.single.range.carriesIntoScroll, isTrue);
      // The selected row is in its place in the list, under the tabs.
      expect(rowRect(tester, 0).top, tester.getRect(tabs).bottom + 8);
      expect(rowRect(tester, 0), slotRects(tester)[0]);
    });

    testWidgets('opens the same before it knows there\'s no start', (
      tester,
    ) async {
      await picker.open(
        tester,
        location: FakeLocation(permission: LocationPermission.denied),
      );
      // With no start there's one row, the message. The sheet opened before that
      // was known, on the placeholders, and stays where it is.
      final sheet = sheetOf(tester);
      expect(find.byType(OptionMessage), findsOneWidget);
      expect(
        tester.getRect(find.byType(OptionMessage)).bottom,
        lessThan(footerTop(tester)),
      );
      expect(sheet.extent, moreOrLessEquals(appScreen.height * 0.6));
      expect(picker.routes.asked, isEmpty);
    });

    testWidgets('rests at the same two anchors in every street way, loading, '
        'and with no route', (tester) async {
      picker.answerNothing();
      await picker.open(tester);
      final sheet = sheetOf(tester);
      List<double> anchors() => [
        extentOf(sheet, footerAnchor),
        extentOf(sheet, headerAnchor),
      ];
      final loading = anchors();
      // The handle, a row and the buttons; and the ends and tabs over those.
      final row = OptionRowMetrics.heightOf(tester.element(tabs));
      expect(
        loading[1] - loading[0],
        tester
            .getSize(
              find.ancestor(of: tabs, matching: find.byType(SheetSection)),
            )
            .height,
      );
      expect(tester.getSize(slots.first).height, row);

      picker.asked(TravelMode.driving).single.answer.complete(drivingRoutes);
      await tester.pumpAndSettle();
      expect(anchors(), loading, reason: 'driving');

      await showMode(tester, TravelMode.walking);
      expect(anchors(), loading, reason: 'walking, loading');
      picker.asked(TravelMode.walking).single.answer.complete(walkingRoutes);
      await tester.pumpAndSettle();
      expect(anchors(), loading, reason: 'walking');

      await showMode(tester, TravelMode.cycling);
      picker
          .asked(TravelMode.cycling)
          .single
          .answer
          .completeError(http.ClientException('offline'));
      await tester.pumpAndSettle();
      expect(find.text("Couldn't get directions. Try again."), findsOneWidget);
      expect(anchors(), loading, reason: 'cycling, failed');

      // And whichever row is selected.
      await showMode(tester, TravelMode.driving);
      routeTaps.add(3);
      await tester.pumpAndSettle();
      expect(anchors(), loading, reason: 'the last row selected');
    });

    testWidgets('at the header anchor shows the ends and tabs, the selected '
        'row under them, and the buttons', (tester) async {
      await picker.open(tester);
      routeTaps.add(2);
      await tester.pumpAndSettle();
      await moveTo(tester, headerAnchor);
      final row = rowRect(tester, 2);
      expect(row.top, moreOrLessEquals(tester.getRect(tabs).bottom + 8));
      expect(row.bottom, moreOrLessEquals(footerTop(tester)));
      expect(find.text('From').hitTestable(), findsOneWidget);
      expect(find.text('34 min').hitTestable(), findsWidgets);
    });

    testWidgets('at the lowest anchor shows the selected row under the '
        'handle, and the buttons', (tester) async {
      await picker.open(tester);
      routeTaps.add(2);
      await tester.pumpAndSettle();
      await moveTo(tester, footerAnchor);
      final sheet = sheetOf(tester);
      final row = rowRect(tester, 2);
      expect(row.bottom, moreOrLessEquals(footerTop(tester)));
      expect(
        appScreen.height - sheet.extent,
        lessThan(row.top),
        reason: 'the handle is over it',
      );
      expect(row.top - (appScreen.height - sheet.extent), lessThan(30));
      // The ends and tabs have passed behind it.
      expect(find.text('From').hitTestable(), findsNothing);
      expect(find.text('Start').hitTestable(), findsOneWidget);
    });

    testWidgets('paints no speed profile again as it moves', (tester) async {
      await picker.open(tester);
      final boundaries = [
        for (final element
            in find
                .descendant(
                  of: find.byType(SpeedProfile),
                  matching: find.byType(RepaintBoundary),
                )
                .evaluate())
          element.renderObject! as RenderRepaintBoundary,
      ];
      expect(boundaries, hasLength(4));
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
        expect(boundary.debugSymmetricPaintCount, 0);
        expect(boundary.debugAsymmetricPaintCount, 0);
      }
    });
  });

  group('the held row', () {
    /// How long into a change of what's held to look at it under way.
    const partway = Duration(milliseconds: 60);

    testWidgets('a callout tapped for a row below the buttons: it slides up '
        'from behind them, and the list stays', (tester) async {
      await picker.open(tester);
      final dock = footerTop(tester);
      final places = slotRects(tester);
      expect(places[3].top, greaterThan(dock), reason: 'out of view');
      expect(rowRect(tester, 0), places[0], reason: 'nothing held away');

      routeTaps.add(3);
      await tester.pump();
      await tester.pump();
      await tester.pump(partway);
      expect(
        rowRect(tester, 3).bottom,
        inExclusiveRange(dock, places[3].bottom),
      );
      await tester.pumpAndSettle();
      expect(rowRect(tester, 3).bottom, moreOrLessEquals(dock));
      expect(selectedRows(tester), [false, false, false, true]);
      expect(slotRects(tester), places);
      expect(rowRect(tester, 0), places[0]);
    });

    testWidgets('a visible row selected while one is held: the held one '
        'slides back down, the new one fills in place', (tester) async {
      await picker.open(tester);
      final dock = footerTop(tester);
      final places = slotRects(tester);
      routeTaps.add(3);
      await tester.pumpAndSettle();

      await tester.tap(rows.at(0));
      await tester.pump();
      await tester.pump(partway);
      expect(selectedRows(tester).first, isTrue);
      expect(rowRect(tester, 0), places[0]);
      expect(
        rowRect(tester, 3).bottom,
        inExclusiveRange(dock, places[3].bottom),
      );
      await tester.pumpAndSettle();
      expect(rowRect(tester, 3), places[3]);
      expect(rowRect(tester, 0), places[0]);
      expect(slotRects(tester), places);
    });

    testWidgets('a callout tapped for another row out of view: the held row '
        'changes in place', (tester) async {
      await picker.open(tester);
      final dock = footerTop(tester);
      final places = slotRects(tester);
      expect(places[2].top, greaterThan(dock), reason: 'out of view too');
      routeTaps.add(3);
      await tester.pumpAndSettle();
      final held = rowRect(tester, 3);

      routeTaps.add(2);
      await tester.pump();
      await tester.pump();
      // It's there from the first, fading in over the other: nothing slides.
      expect(rowRect(tester, 2), held);
      await tester.pump(partway);
      expect(rowRect(tester, 2), held);
      expect(rowRect(tester, 3), held);
      await tester.pumpAndSettle();
      expect(rowRect(tester, 2), held);
      expect(rowRect(tester, 3), places[3]);
    });

    for (final anchor in [footerAnchor, headerAnchor]) {
      testWidgets('a callout tapped at $anchor: the held row changes in '
          'place', (tester) async {
        await picker.open(tester);
        await moveTo(tester, anchor);
        final sheet = sheetOf(tester);
        final extent = sheet.extent;
        final held = rowRect(tester, 0);
        expect(held.bottom, moreOrLessEquals(footerTop(tester)));

        routeTaps.add(1);
        await tester.pump();
        await tester.pump();
        expect(rowRect(tester, 1), held);
        await tester.pump(partway);
        expect(rowRect(tester, 1), held);
        await tester.pumpAndSettle();
        expect(rowRect(tester, 1), held);
        expect(sheet.extent, extent);
        expect(sheet.restingAt, anchor);
      });
    }

    testWidgets('a row cut off by the buttons is tapped: it rises to sit '
        'whole above them, and nothing else moves', (tester) async {
      await picker.open(tester);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final dock = footerTop(tester);
      final places = slotRects(tester);
      final header = tester.getRect(tabs);
      expect(places[1].top, lessThan(dock));
      expect(places[1].bottom, greaterThan(dock), reason: 'cut off');

      await tester.tapAt(Offset(places[1].center.dx, places[1].top + 12));
      await tester.pump();
      expect(selectedRows(tester), [false, true, false, false]);
      await tester.pump(partway);
      expect(
        rowRect(tester, 1).bottom,
        inExclusiveRange(dock, places[1].bottom),
      );
      await tester.pumpAndSettle();
      // Whole above the buttons, over part of the row above it.
      expect(rowRect(tester, 1).bottom, moreOrLessEquals(dock));
      expect(rowRect(tester, 1).top, lessThan(places[0].bottom));
      expect(rowRect(tester, 0), places[0]);
      expect(slotRects(tester), places);
      expect(tester.getRect(tabs), header);
      expect(sheet.extent, extent);
    });

    testWidgets('another tab tapped at the header anchor: the held row fades '
        'into that way\'s selected row, and the tabs stay', (tester) async {
      await picker.open(tester);
      routeTaps.add(2);
      await tester.pumpAndSettle();
      await moveTo(tester, headerAnchor);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final header = tester.getRect(tabs);
      final held = rowRect(tester, 2);

      await tester.tap(
        find.descendant(of: tabs, matching: find.byIcon(Icons.directions_bike)),
      );
      await tester.pump();
      await tester.pump(partway);
      // Cycling's first row, where driving's third was, and that one fading.
      final incoming = find.ancestor(
        of: find.text('1 hr 18 min').last,
        matching: rows,
      );
      expect(tester.getRect(incoming), held);
      expect(
        tester.getRect(
          find.ancestor(of: find.text('34 min').last, matching: rows),
        ),
        held,
      );
      expect(tester.getRect(tabs), header);
      expect(sheet.extent, extent);
      await tester.pumpAndSettle();
      expect(selectedRows(tester), [true, false, false]);
      expect(rowRect(tester, 0), held);
      expect(find.text('34 min'), findsNothing);
      expect(tester.getRect(tabs), header);
      expect(sheet.extent, extent);
      expect(sheet.restingAt, headerAnchor);
    });

    testWidgets('another tab tapped in the free range: the rows fade into '
        'that way\'s in their places', (tester) async {
      await picker.open(tester);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final places = slotRects(tester);

      await tester.tap(
        find.descendant(of: tabs, matching: find.byIcon(Icons.directions_bike)),
      );
      await tester.pump();
      await tester.pump(partway);
      // Both ways' rows, each in its slot, one fading into the other.
      expect(rows, findsNWidgets(7));
      expect(slotRects(tester), places);
      final fades = tester.widgetList<FadeTransition>(
        find.descendant(
          of: slots.at(0),
          matching: find.descendant(
            of: find.byType(AnimatedSwitcher),
            matching: find.byType(FadeTransition),
          ),
        ),
      );
      expect(fades.map((fade) => fade.opacity.value), [
        inExclusiveRange(0, 1),
        inExclusiveRange(0, 1),
      ]);
      await tester.pumpAndSettle();
      expect(rows, findsNWidgets(3));
      // The slot the fourth row had goes once it has faded.
      expect(slotRects(tester), places.take(3));
      expect(sheet.extent, extent);
    });

    testWidgets('while a way loads a placeholder is held, and its row fades '
        'in there', (tester) async {
      picker.answerNothing();
      await picker.open(tester);
      await moveTo(tester, headerAnchor);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final held = tester.getRect(find.byType(OptionPlaceholder).first);
      expect(held.bottom, moreOrLessEquals(footerTop(tester)));
      expect(held.top, moreOrLessEquals(tester.getRect(tabs).bottom + 8));

      picker.asked(TravelMode.driving).single.answer.complete(drivingRoutes);
      await tester.pump();
      await tester.pump();
      await tester.pump(partway);
      expect(rowRect(tester, 0), held);
      expect(find.byType(OptionPlaceholder), findsWidgets);
      await tester.pumpAndSettle();
      expect(find.byType(OptionPlaceholder), findsNothing);
      expect(rowRect(tester, 0), held);
      expect(sheet.extent, extent);
    });

    testWidgets('with no route the message is held in its place, and the '
        'buttons are disabled', (tester) async {
      picker.answerNothing();
      await picker.open(tester);
      await moveTo(tester, headerAnchor);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final held = tester.getRect(find.byType(OptionPlaceholder).first);

      picker.asked(TravelMode.driving).single.answer.complete([]);
      await tester.pumpAndSettle();
      expect(find.text('No route found'), findsOneWidget);
      expect(tester.getRect(find.byType(OptionMessage)), held);
      expect(sheet.extent, extent);
      for (final label in ['Steps', 'Start']) {
        expect(
          tester
              .widget<FilledButton>(find.widgetWithText(FilledButton, label))
              .onPressed,
          isNull,
        );
      }
    });

    testWidgets('an end changes: the rows fade to placeholders, then to the '
        'new routes, the first selected', (tester) async {
      await picker.open(tester);
      routeTaps.add(3);
      await tester.pumpAndSettle();
      await moveTo(tester, headerAnchor);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final held = rowRect(tester, 3);
      picker.answerNothing();

      await tester.tap(find.byTooltip('Swap start and destination'));
      await tester.pump();
      await tester.pump(partway);
      // The placeholder is where the held row was, which fades under it.
      expect(tester.getRect(find.byType(OptionPlaceholder).first), held);
      expect(rows, findsWidgets);
      await tester.pumpAndSettle();
      expect(rows, findsNothing);
      expect(find.byType(OptionPlaceholder), findsNWidgets(3));
      expect(tester.getRect(find.byType(OptionPlaceholder).first), held);
      expect(sheet.extent, extent);

      picker.asked(TravelMode.driving).last.answer.complete([
        viaToll,
        viaStreets,
      ]);
      await tester.pumpAndSettle();
      expect(selectedRows(tester), [true, false]);
      expect(rowRect(tester, 0), held);
      expect(sheet.extent, extent);
    });

    test('casts its shadow over the first 8 pixels it\'s held away', () {
      expect(heldShadowStrength(0), 0);
      expect(heldShadowStrength(4), 0.5);
      expect(heldShadowStrength(8), 1);
      expect(heldShadowStrength(120), 1);
      // Under it, when it's held down from above.
      expect(heldShadowStrength(-2), -0.25);
      expect(heldShadowStrength(-300), -1);
    });
  });

  group('editing an end', () {
    testWidgets('fades the buttons out, and back in with the rows', (
      tester,
    ) async {
      await picker.open(tester);
      final sheet = sheetOf(tester);
      await tester.tap(find.widgetWithText(InkWell, 'To'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      final fading = tester.widget<FadeTransition>(
        find.ancestor(of: footer, matching: find.byType(FadeTransition)).first,
      );
      expect(fading.opacity.value, inExclusiveRange(0, 1));
      await tester.pumpAndSettle();
      expect(footer, findsNothing);
      expect(rows, findsNothing);
      expect(find.text('Use current location'), findsOneWidget);
      expect(sheet.isAtMax, isTrue);
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.selection,
        const TextSelection(baseOffset: 0, extentOffset: 16),
      );

      await tester.binding.handlePopRoute();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      expect(
        tester
            .widget<FadeTransition>(
              find
                  .ancestor(of: footer, matching: find.byType(FadeTransition))
                  .first,
            )
            .opacity
            .value,
        inExclusiveRange(0, 1),
      );
      await tester.pumpAndSettle();
      expect(sheet.restingAt, headerAnchor);
      expect(rows, findsNWidgets(4));
      expect(rowRect(tester, 0).bottom, moreOrLessEquals(footerTop(tester)));
      expect(find.text('Museum of Flight'), findsOneWidget);
    });

    testWidgets('the field\'s own button clears the end', (tester) async {
      await picker.open(tester);
      await tester.tap(find.widgetWithText(InkWell, 'From'));
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('Clear'));
      await tester.pumpAndSettle();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.text('My Location'), findsNothing);
      expect(
        find.text('Enter a start and a destination to get directions'),
        findsOneWidget,
      );
      // Nothing is asked, so no tab has a time or a dash.
      for (final mode in TravelMode.values) {
        expect(tabTime(tester, mode), isNull);
      }
    });
  });

  group('the buttons', () {
    testWidgets('Start navigates the selected route', (tester) async {
      final router = await picker.open(tester);
      await tester.tap(rows.at(1));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Start'));
      await tester.pumpAndSettle();
      expect(router.state.uri.toString(), navigationLocation(destination.id));
      expect(picker.navigation.state, NavigationState.acquiring);
      expect(picker.navigation.route, same(viaToll));
      expect(picker.navigation.mode, TravelMode.driving);
    });

    testWidgets('Start navigates the way that\'s showing', (tester) async {
      await picker.open(tester);
      await showMode(tester, TravelMode.cycling);
      await tester.tap(find.text('Start'));
      await tester.pumpAndSettle();
      expect(picker.navigation.route, same(cyclingRoutes.first));
      expect(picker.navigation.mode, TravelMode.cycling);
    });

    testWidgets('Steps opens the selected route\'s steps, and back returns '
        'to the sheet where it was', (tester) async {
      final router = await picker.open(tester);
      await tester.tap(rows.at(0));
      await tester.pumpAndSettle();
      final sheet = sheetOf(tester);
      // Somewhere in the free range.
      await tester.drag(tabs, const Offset(0, -90));
      await tester.pumpAndSettle();
      final extent = sheet.extent;
      expect(sheet.restingIn, isNotNull);

      await tester.tap(find.text('Steps'));
      await tester.pumpAndSettle();
      expect(router.state.uri.toString(), stepsLocation(destination.id));
      expect(find.text('Drive south.'), findsOneWidget);
      expect(find.text('Take I-5.'), findsOneWidget);
      // The time and the name, on the one line.
      expect(find.text('22 min via I-5 South'), findsOneWidget);
      expect(find.text('0.4 mi'), findsOneWidget);
      expect(footer.hitTestable(), findsNothing);
      expect(sheet.restingAt, const SheetAnchor.fraction(0.4));

      // The steps follow a callout tapped on the map.
      routeTaps.add(1);
      await tester.pumpAndSettle();
      expect(find.text('Take SR 99.'), findsOneWidget);
      expect(find.text('25 min via SR 99 and SR 509'), findsOneWidget);

      await tester.tap(find.byTooltip('Back'));
      await tester.pumpAndSettle();
      expect(router.state.uri.toString(), directionsLocation(destination.id));
      expect(sheetOf(tester).extent, moreOrLessEquals(extent));
      expect(selectedRows(tester), [false, true, false, false]);
    });

    testWidgets('the steps page without a trip is directions', (tester) async {
      final router = await picker.open(tester);
      router.go(stepsLocation(destination.id));
      await tester.pumpAndSettle();
      expect(router.state.uri.toString(), directionsLocation(destination.id));
    });
  });

  group('transit, until it has rows of its own', () {
    testWidgets('lists its journeys under the same ends and tabs, with '
        'nothing held and no buttons', (tester) async {
      await picker.open(tester);
      final header = tester.getRect(tabs);
      await showMode(tester, TravelMode.transit);
      await moveTo(tester, const SheetAnchor.full());
      expect(find.byType(JourneyRow), findsNWidgets(2));
      expect(footer, findsNothing);
      expect(slots, findsNothing);
      expect(tester.getSize(tabs), header.size);
      final sheet = sheetOf(tester);
      expect(sheet.anchors.map((a) => a.anchor), [footerAnchor, headerAnchor]);
      // The lowest shows only the handle, the other only the ends and tabs.
      expect(extentOf(sheet, footerAnchor), lessThan(60));
    });

    testWidgets('a journey opens its Trip details over the page', (
      tester,
    ) async {
      await picker.open(tester);
      await showMode(tester, TravelMode.transit);
      await moveTo(tester, const SheetAnchor.full());
      final sheet = sheetOf(tester);
      await tester.tap(find.byType(JourneyRow).first);
      await tester.pumpAndSettle();
      expect(find.text('Trip details'), findsOneWidget);
      expect(sheet.restingAt, const SheetAnchor.fraction(0.4));
    });
  });

  group('the trip card', () {
    /// What the card of the trip's ends is filled with.
    Color? fill(WidgetTester tester) => tester
        .widget<CupertinoListSection>(
          find.descendant(
            of: find.byType(TripCard),
            matching: find.byType(CupertinoListSection),
          ),
        )
        .decoration!
        .color;

    for (final brightness in Brightness.values) {
      testWidgets('is a tint on iOS, as the cards are, '
          '${brightness.name}', (tester) async {
        tester.platformDispatcher.platformBrightnessTestValue = brightness;
        addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
        await picker.open(tester, platform: TargetPlatform.iOS);
        final context = tester.element(find.byType(TripCard));
        expect(fill(tester), groupedRowTint(context));
        expect(fill(tester)!.a, lessThan(1));
        expect(fill(tester)!.withValues(alpha: 1), groupedRowColor(context));
      });
    }

    testWidgets('is a filled card on Android', (tester) async {
      await picker.open(tester);
      expect(
        find.descendant(of: find.byType(TripCard), matching: find.byType(Card)),
        findsOneWidget,
      );
      expect(
        find.descendant(
          of: find.byType(TripCard),
          matching: find.byType(CupertinoListSection),
        ),
        findsNothing,
      );
    });
  });
}
