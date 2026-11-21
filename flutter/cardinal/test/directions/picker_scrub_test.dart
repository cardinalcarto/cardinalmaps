import 'package:cardinal/directions/directions_page.dart';
import 'package:cardinal/directions/elevation_profile.dart';
import 'package:cardinal/directions/journey_list.dart';
import 'package:cardinal/directions/mode_tabs.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/scrub.dart';
import 'package:cardinal/directions/scrub_marker.dart';
import 'package:cardinal/directions/speed_profile.dart';
import 'package:cardinal/directions/transit_timeline.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';
import 'package:sparkle/sparkle.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_routes.dart';
import 'picker_harness.dart';

void main() {
  late Picker picker;

  setUp(() => picker = Picker());

  Finder journeyRows() => find.byType(JourneyRow);
  Finder timelineOf(int index) => find.descendant(
    of: journeyRows().at(index),
    matching: find.byType(JourneyTimeline),
  );

  Rect rowRect(WidgetTester tester, int index) =>
      tester.getRect(rows.at(index));

  /// The graph of the row at [index].
  Finder graphOf(int index, {Type type = SpeedProfile}) =>
      find.descendant(of: rows.at(index), matching: find.byType(type));

  /// The painter of the readout of the row at [index], which knows its scrub.
  ReadoutPainter readoutOf(WidgetTester tester, Finder row) => tester
      .widgetList<CustomPaint>(
        find.descendant(of: row, matching: find.byType(CustomPaint)),
      )
      .map((paint) => paint.painter)
      .whereType<ReadoutPainter>()
      .first;

  RowScrub scrubOf(WidgetTester tester, Finder row) =>
      readoutOf(tester, row).scrub;

  /// How much the readout of [row] shows, and the text it replaces.
  ({double readout, double replaced}) fadesOf(WidgetTester tester, Finder row) {
    final scrub = scrubOf(tester, row);
    return (readout: scrub.shown.value, replaced: 1 - scrub.shown.value);
  }

  /// The marker as the map was last given it: where, and how opaque, or null if
  /// there's none.
  ({double lng, double lat, double opacity})? marker() {
    final features =
        (picker.map.geoJson[scrubSource] as Map?)?['features'] as List?;
    if (features == null || features.isEmpty) return null;
    final feature = features.single as Map;
    final coordinates = (feature['geometry'] as Map)['coordinates'] as List;
    return (
      lng: coordinates[0] as double,
      lat: coordinates[1] as double,
      opacity: ((feature['properties'] as Map)['opacity'] as num).toDouble(),
    );
  }

  int markerWrites() =>
      picker.map.calls.where((call) => call == scrubSource).length;

  List<Object?> drawnSelected() => [
    for (final feature
        in (picker.map.geoJson[routeSource]! as Map)['features'] as List)
      if (((feature as Map)['properties'] as Map)['role'] == 'line')
        (feature['properties'] as Map)['selected'],
  ];

  /// Lets the marker's time after a scrub pass, and its fade.
  Future<void> letMarkerFade(WidgetTester tester) async {
    await tester.pump(ScrubMarker.linger + const Duration(milliseconds: 50));
    await tester.pumpAndSettle();
  }

  /// Drags the sheet up by about [delta], past the slop, slowly enough that it rests where it's let go.
  Future<void> raiseBy(WidgetTester tester, double delta) async {
    final gesture = await tester.startGesture(tester.getCenter(tabs));
    await gesture.moveBy(const Offset(0, -20));
    await tester.pump(const Duration(milliseconds: 16));
    await gesture.moveBy(Offset(0, -delta));
    await tester.pump(const Duration(milliseconds: 16));
    await tester.pump(const Duration(milliseconds: 500));
    await gesture.up();
    await tester.pumpAndSettle();
  }

  /// Puts a finger down [at] of the way across [graph] and drags it sideways past the
  /// slop, so a scrub starts, then moves it to [at] again.
  Future<TestGesture> scrub(
    WidgetTester tester,
    Finder graph, {
    double at = 0.3,
  }) async {
    final rect = tester.getRect(graph);
    final start = Offset(rect.left + rect.width * at, rect.center.dy);
    final gesture = await tester.startGesture(start);
    await gesture.moveBy(const Offset(24, 0));
    await tester.pump();
    await gesture.moveTo(start);
    await tester.pump();
    return gesture;
  }

  group('a drag on a graph', () {
    testWidgets('that starts sideways scrubs, and the sheet stays put however '
        'the finger then moves', (tester) async {
      await picker.open(tester);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final places = slotRects(tester);
      final gesture = await scrub(tester, graphOf(0));
      final state = scrubOf(tester, rows.at(0));
      expect(state.active, isTrue);
      await gesture.moveBy(const Offset(10, -120));
      await tester.pump();
      await gesture.moveBy(const Offset(10, 200));
      await tester.pump();
      expect(sheet.extent, extent);
      expect(slotRects(tester), places);
      expect(state.active, isTrue);
      await gesture.up();
      await tester.pumpAndSettle();
      expect(state.active, isFalse);
      expect(sheet.extent, extent);
    });

    testWidgets('that starts up or down moves the sheet, and doesn\'t scrub', (
      tester,
    ) async {
      await picker.open(tester);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final rect = tester.getRect(graphOf(0));
      final gesture = await tester.startGesture(rect.center);
      await gesture.moveBy(const Offset(0, 30));
      await tester.pump();
      await gesture.moveBy(const Offset(40, 60));
      await tester.pump();
      expect(sheet.extent, lessThan(extent));
      expect(scrubOf(tester, rows.at(0)).active, isFalse);
      await gesture.up();
      await tester.pumpAndSettle();
      expect(marker(), isNull);
    });

    testWidgets('a tap on it selects its row', (tester) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(graphOf(2));
      await tester.pumpAndSettle();
      expect(selectedRows(tester), [false, false, true, false]);
      expect(scrubOf(tester, rows.at(2)).active, isFalse);
      expect(marker(), isNull);
    });

    testWidgets('its touch zone reaches above and below the graph, to a '
        'finger\'s height', (tester) async {
      await picker.open(tester);
      final rect = tester.getRect(graphOf(0));
      expect(rect.height, routeGraphHeight);
      final gesture = await tester.startGesture(
        Offset(rect.left + 40, rect.top - 5),
      );
      await gesture.moveBy(const Offset(30, 0));
      await tester.pump();
      expect(scrubOf(tester, rows.at(0)).active, isTrue);
      await gesture.up();
      await tester.pumpAndSettle();
      final below = await tester.startGesture(
        Offset(rect.left + 40, rect.bottom + 5),
      );
      await below.moveBy(const Offset(30, 0));
      await tester.pump();
      expect(scrubOf(tester, rows.at(0)).active, isTrue);
      await below.up();
      await tester.pumpAndSettle();
      expect(
        scrubZoneHeight,
        lessThanOrEqualTo(routeGraphHeight + 2 * OptionRowMetrics.gap),
      );
    });

    testWidgets(
      'from the edge the system takes for going back doesn\'t scrub',
      (tester) async {
        tester.view.systemGestureInsets = const FakeViewPadding(left: 120);
        addTearDown(tester.view.resetSystemGestureInsets);
        await picker.open(tester);
        final rect = tester.getRect(graphOf(0));
        // In the edge, over the graph: the system's.
        final edge = await tester.startGesture(
          Offset(rect.left + 30, rect.center.dy),
        );
        await edge.moveBy(const Offset(40, 0));
        await tester.pump();
        expect(scrubOf(tester, rows.at(0)).active, isFalse);
        await edge.up();
        await tester.pumpAndSettle();
        // Clear of it: a scrub.
        final clear = await tester.startGesture(
          Offset(rect.left + 120, rect.center.dy),
        );
        await clear.moveBy(const Offset(40, 0));
        await tester.pump();
        expect(scrubOf(tester, rows.at(0)).active, isTrue);
        await clear.up();
        await tester.pumpAndSettle();
      },
    );
  });

  group('a scrub', () {
    testWidgets('of a row not selected selects it as it starts', (
      tester,
    ) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      final gesture = await scrub(tester, graphOf(2));
      expect(selectedRows(tester), [false, false, true, false]);
      expect(drawnSelected(), [false, false, true, false]);
      await gesture.up();
      await tester.pumpAndSettle();
      expect(selectedRows(tester), [false, false, true, false]);
    });

    testWidgets('of a row cut off by the buttons selects it where it is, and '
        'it rises only once the finger lifts', (tester) async {
      await picker.open(tester);
      final sheet = sheetOf(tester);
      final dock = footerTop(tester);
      expect(slotRects(tester)[1].bottom, greaterThan(dock), reason: 'cut off');
      // Up just so far that its graph shows over the buttons.
      for (var i = 0; i < 4; i++) {
        final over = tester.getRect(graphOf(1)).bottom - dock;
        if (over < -4) break;
        await raiseBy(tester, over + 8);
      }
      final extent = sheet.extent;
      final places = slotRects(tester);
      final header = tester.getRect(tabs);
      expect(places[1].bottom, greaterThan(dock), reason: 'still cut off');
      expect(
        tester.getRect(graphOf(1)).bottom,
        lessThan(dock),
        reason: 'its graph shows',
      );

      final gesture = await scrub(tester, graphOf(1));
      expect(selectedRows(tester), [false, true, false, false]);
      for (var i = 0; i < 10; i++) {
        await gesture.moveBy(const Offset(8, 0));
        await tester.pump(const Duration(milliseconds: 50));
        expect(
          rowRect(tester, 1),
          places[1],
          reason: 'still, under the finger',
        );
        expect(rowRect(tester, 0), places[0]);
      }
      await gesture.up();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      expect(
        rowRect(tester, 1).bottom,
        inExclusiveRange(dock, places[1].bottom),
        reason: 'rising',
      );
      await tester.pumpAndSettle();
      expect(rowRect(tester, 1).bottom, moreOrLessEquals(dock));
      expect(rowRect(tester, 0), places[0]);
      expect(slotRects(tester), places);
      expect(tester.getRect(tabs), header);
      expect(sheet.extent, extent);
    });

    testWidgets('ends with its row, if another way is shown under it', (
      tester,
    ) async {
      await picker.open(tester);
      Object? held() => tester
          .widget<SheetContent>(
            find.descendant(
              of: find.byType(DirectionsPage),
              matching: find.byType(SheetContent),
            ),
          )
          .held;
      final gesture = await scrub(tester, graphOf(0));
      tester.widget<ModeTabs>(tabs).onChanged(TravelMode.walking);
      await tester.pumpAndSettle();
      await gesture.up();
      await tester.pumpAndSettle();
      routeTaps.add(1);
      await tester.pumpAndSettle();
      expect(selectedRows(tester), [false, true]);
      expect(held(), OptionList.slotId(1));
      await letMarkerFade(tester);
      expect(marker(), isNull);
    });

    testWidgets('puts the hairline at the finger, kept to the graph', (
      tester,
    ) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      // The fastest's graph is its share of the slowest's time.
      final rect = tester.getRect(graphOf(0));
      final gesture = await scrub(tester, graphOf(0), at: 0.4);
      final state = scrubOf(tester, rows.at(0));
      expect(state.x, moreOrLessEquals(rect.width * 0.4));
      await gesture.moveTo(Offset(rect.right + 80, rect.center.dy));
      await tester.pump();
      expect(state.x, moreOrLessEquals(rect.width));
      await gesture.moveTo(Offset(rect.left - 30, rect.center.dy));
      await tester.pump();
      expect(state.x, 0);
      await gesture.up();
      await tester.pumpAndSettle();
    });

    testWidgets('builds nothing and paints nothing of the row again as it goes '
        'but the hairline and the readout', (tester) async {
      await picker.open(tester);
      final profile =
          tester
                  .element(
                    find.descendant(
                      of: graphOf(0),
                      matching: find.byType(RepaintBoundary),
                    ),
                  )
                  .renderObject!
              as RenderRepaintBoundary;
      profile.debugResetMetrics();
      final built = <Type>[];
      debugOnRebuildDirtyWidget = (element, _) =>
          built.add(element.widget.runtimeType);
      addTearDown(() => debugOnRebuildDirtyWidget = null);
      final gesture = await scrub(tester, graphOf(0), at: 0.1);
      // The readout's fade in, as the arrival fades out, paints the profile's layer
      // again as it starts and ends, and no more.
      await tester.pump(const Duration(milliseconds: 400));
      expect(
        profile.debugSymmetricPaintCount + profile.debugAsymmetricPaintCount,
        lessThanOrEqualTo(2),
      );
      profile.debugResetMetrics();
      built.clear();
      for (var i = 0; i < 20; i++) {
        await gesture.moveBy(const Offset(5, 0));
        await tester.pump(const Duration(milliseconds: 16));
      }
      expect(profile.debugSymmetricPaintCount, 0);
      expect(profile.debugAsymmetricPaintCount, 0);
      expect(built, isEmpty);
      await gesture.up();
      await letMarkerFade(tester);
    });

    testWidgets('reads out a drive\'s mile, road and speed in the arrival\'s '
        'place, fading in and back', (tester) async {
      picker.routes.immediatelyBy[TravelMode.driving] = [
        fakeRoute(
          const [
            [0.5, 2.0],
            [1.0, 4.0],
          ],
          distance: 22850,
          duration: 1320,
          runs: [
            fakeRun(3, 8),
            fakeRun(15, 27, motorway: true),
            fakeRun(4, 12),
          ],
          steps: const [
            DirectionsStep(
              instruction: 'Drive south.',
              distance: 640,
              duration: 90,
              roadName: 'Airport Way',
            ),
            DirectionsStep(
              instruction: 'Take I-5.',
              distance: 22210,
              duration: 1230,
              roadName: 'I-5 South',
            ),
          ],
        ),
      ];
      await picker.open(tester);
      final arrival = tester.getRect(find.textContaining('Arrive'));
      final gesture = await scrub(tester, graphOf(0), at: 0.5);
      await tester.pump(const Duration(milliseconds: 60));
      final fades = fadesOf(tester, rows.at(0));
      expect(fades.readout, inExclusiveRange(0, 1));
      await tester.pump(const Duration(milliseconds: 300));
      expect(fadesOf(tester, rows.at(0)).readout, 1);
      expect(scrubOf(tester, rows.at(0)).readout, [
        'Mile 7.1',
        'I-5 South',
        '60 mph',
      ]);
      // As much of it as fits the arrival's room, from the first.
      expect(
        'Mile 7.1 · I-5 South · 60 mph',
        startsWith(readoutOf(tester, rows.at(0)).painted!),
      );
      // In the arrival's room, which hasn't moved.
      final box = tester.getRect(
        find
            .ancestor(
              of: find.byWidgetPredicate(
                (w) => w is CustomPaint && w.painter is ReadoutPainter,
              ),
              matching: find.byType(Positioned),
            )
            .first,
      );
      expect(box.right, moreOrLessEquals(arrival.right));
      expect(tester.getRect(find.textContaining('Arrive')), arrival);
      await gesture.up();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      expect(fadesOf(tester, rows.at(0)).readout, inExclusiveRange(0, 1));
      await tester.pumpAndSettle();
      expect(fadesOf(tester, rows.at(0)).readout, 0);
    });

    testWidgets('reads out a ride\'s mile, height and grade', (tester) async {
      picker.routes.immediatelyBy[TravelMode.cycling] = [
        fakeRoute(
          const [
            [0.5, 2.0],
            [1.0, 4.0],
          ],
          distance: 900,
          duration: 300,
          elevation: fakeElevation([
            for (var i = 0; i <= 30; i++) 100.0 + 3 * i,
          ]),
        ),
      ];
      await picker.open(tester);
      await showMode(tester, TravelMode.cycling);
      final gesture = await scrub(
        tester,
        graphOf(0, type: ElevationProfile),
        at: 0.5,
      );
      await tester.pump(const Duration(milliseconds: 300));
      expect(scrubOf(tester, rows.at(0)).readout, [
        'Mile 0.3',
        '480 ft',
        '10% uphill',
      ]);
      await gesture.up();
      await tester.pumpAndSettle();
    });

    testWidgets('reads out in kilometers where the device does', (
      tester,
    ) async {
      tester.platformDispatcher.localeTestValue = const Locale('de', 'DE');
      addTearDown(tester.platformDispatcher.clearLocaleTestValue);
      await picker.open(tester);
      final gesture = await scrub(tester, graphOf(0), at: 0.5);
      await tester.pump(const Duration(milliseconds: 300));
      final readout = scrubOf(tester, rows.at(0)).readout;
      expect(readout.first, startsWith('km '));
      expect(readout.last, endsWith(' km/h'));
      await gesture.up();
      await tester.pumpAndSettle();
    });
  });

  group('the marker', () {
    testWidgets(
      'moves along the route as it\'s scrubbed, at most once a frame',
      (tester) async {
        await picker.open(tester);
        expect(marker(), isNull);
        final rect = tester.getRect(graphOf(0));
        final gesture = await scrub(tester, graphOf(0), at: 0);
        // The line runs from (0.5, 2) to (1, 4).
        expect(marker()!.lng, moreOrLessEquals(0.5));
        expect(marker()!.lat, moreOrLessEquals(2));
        expect(marker()!.opacity, 1);
        final writes = markerWrites();
        for (var i = 0; i < 3; i++) {
          await gesture.moveBy(Offset(rect.width / 6, 0));
        }
        await tester.pump();
        expect(markerWrites(), writes + 1);
        await gesture.moveTo(Offset(rect.right, rect.center.dy));
        await tester.pump();
        expect(marker()!.lng, moreOrLessEquals(1));
        expect(marker()!.lat, moreOrLessEquals(4));
        await gesture.up();
        await tester.pumpAndSettle();
      },
    );

    testWidgets('fades a second after the finger lifts', (tester) async {
      await picker.open(tester);
      final gesture = await scrub(tester, graphOf(0));
      await gesture.up();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 900));
      expect(marker()!.opacity, 1);
      await tester.pump(const Duration(milliseconds: 150));
      await tester.pump(const Duration(milliseconds: 100));
      await tester.pump();
      expect(marker()!.opacity, inExclusiveRange(0, 1));
      await tester.pump(ScrubMarker.fade);
      await tester.pump();
      expect(marker(), isNull);
    });

    testWidgets('a scrub again before it fades brings it back whole', (
      tester,
    ) async {
      await picker.open(tester);
      var gesture = await scrub(tester, graphOf(0));
      await gesture.up();
      await tester.pump(const Duration(milliseconds: 1150));
      await tester.pump(const Duration(milliseconds: 50));
      await tester.pump();
      expect(marker()!.opacity, lessThan(1));
      gesture = await scrub(tester, graphOf(0), at: 0.6);
      expect(marker()!.opacity, 1);
      await tester.pump(const Duration(seconds: 3));
      expect(marker()!.opacity, 1, reason: 'held while the finger is down');
      await gesture.up();
      await letMarkerFade(tester);
      expect(marker(), isNull);
    });
  });

  group('the camera', () {
    // The fake map's ground is the screen's pixels in hundredths of a degree, so the
    // route, from (0.5, 2) to (1, 4), is on screen from (50, 200) to (100, 400).
    setUp(() {
      picker.map.cameraState = const CameraState(
        lng: 2,
        lat: 2,
        zoom: 13,
        bearing: 0,
        pitch: 0,
      );
    });

    /// The bottom of what shows of the map, less the marker's margin, in degrees.
    double shownBottom(WidgetTester tester) =>
        (appScreen.height -
            SheetScaffold.viewOf(tester.element(tabs)).obscured.bottom -
            32) /
        100;

    testWidgets('holds still while the marker is in view', (tester) async {
      await picker.open(tester);
      final subjects = picker.map.subjectsSet;
      final gesture = await scrub(tester, graphOf(0), at: 0);
      await gesture.moveBy(const Offset(4, 0));
      await tester.pump();
      expect(picker.map.eases, isEmpty);
      expect(picker.map.subjectsSet, subjects);
      await gesture.up();
      await tester.pumpAndSettle();
    });

    testWidgets('pans as little as it must, at the same zoom, when the marker '
        'leaves the view, and stays on release', (tester) async {
      await picker.open(tester);
      final subjects = picker.map.subjectsSet;
      final rect = tester.getRect(graphOf(0));
      final gesture = await scrub(tester, graphOf(0), at: 0);
      await gesture.moveTo(Offset(rect.right, rect.center.dy));
      await tester.pump();
      final ease = picker.map.eases.single;
      expect(ease.zoom, 13);
      expect(ease.lng, moreOrLessEquals(2));
      // The marker at 4°, the view's bottom less its margin brought down to it.
      expect(ease.lat, moreOrLessEquals(2 + 4 - shownBottom(tester)));
      await gesture.up();
      await tester.pumpAndSettle();
      expect(picker.map.eases, hasLength(1));
      expect(picker.map.subjectsSet, subjects, reason: 'no fit again');
    });

    testWidgets('doesn\'t pan while the map is moved by hand', (tester) async {
      await picker.open(tester);
      final rect = tester.getRect(graphOf(0));
      final onMap = await tester.startGesture(
        const Offset(200, 120),
        pointer: 9,
      );
      final gesture = await scrub(tester, graphOf(0), at: 0);
      await gesture.moveTo(Offset(rect.right, rect.center.dy));
      await tester.pump();
      expect(picker.map.eases, isEmpty);
      await onMap.up();
      await tester.pump(const Duration(milliseconds: 100));
      await gesture.moveBy(const Offset(-1, 0));
      await tester.pump();
      expect(picker.map.eases, isEmpty, reason: 'still settling');
      await tester.pump(const Duration(milliseconds: 400));
      await gesture.moveBy(const Offset(1, 0));
      await tester.pump();
      expect(picker.map.eases, hasLength(1));
      await gesture.up();
      await tester.pumpAndSettle();
    });
  });

  testWidgets('a readout too long for its room leaves off parts from the end', (
    tester,
  ) async {
    // The test font's letters are as wide as it's tall.
    const style = TextStyle(fontSize: 10);
    String fit(double width) => fittedReadout(
      ['Mile 4.1', '330 ft', '6% uphill'],
      width,
      style: style,
      textScaler: TextScaler.noScaling,
      direction: TextDirection.ltr,
    );
    expect(fit(500), 'Mile 4.1 · 330 ft · 6% uphill');
    expect(fit(200), 'Mile 4.1 · 330 ft');
    expect(fit(100), 'Mile 4.1');
    expect(fit(20), 'Mile 4.1', reason: 'the first, which is cut short');
  });

  group('to a screen reader', () {
    testWidgets('a graph is a slider that steps along the route, saying the '
        'readout', (tester) async {
      final handle = tester.ensureSemantics();
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      final slider = find.descendant(
        of: rows.at(2),
        matching: find.bySemanticsLabel('Along the route'),
      );
      expect(
        tester.getSemantics(slider),
        isSemantics(
          label: 'Along the route',
          isSlider: true,
          value: 'Mile 0.0, 23 mph',
          hasIncreaseAction: true,
          hasDecreaseAction: false,
        ),
      );
      final node = tester.getSemantics(slider);
      node.owner!.performAction(node.id, SemanticsAction.increase);
      await tester.pump();
      await tester.pump();
      // Selected, and on the map, as a scrub.
      expect(selectedRows(tester), [false, false, true, false]);
      expect(marker(), isNotNull);
      expect(
        tester.getSemantics(slider),
        isSemantics(
          isSlider: true,
          hasIncreaseAction: true,
          hasDecreaseAction: true,
        ),
      );
      expect(tester.getSemantics(slider).value, startsWith('Mile 1.3'));
      // And on, the row already selected.
      final next = tester.getSemantics(slider);
      next.owner!.performAction(next.id, SemanticsAction.increase);
      await tester.pump();
      expect(tester.getSemantics(slider).value, startsWith('Mile 2.6'));
      expect(scrubOf(tester, rows.at(2)).readout.first, 'Mile 2.6');
      // A second after the last step the scrub ends, and the marker fades.
      await tester.pump(scrubStepLinger + const Duration(milliseconds: 50));
      await letMarkerFade(tester);
      expect(marker(), isNull);
      expect(scrubOf(tester, rows.at(2)).active, isFalse);
      handle.dispose();
    });

    testWidgets('a row still reads out all of it, and taps', (tester) async {
      final handle = tester.ensureSemantics();
      await picker.open(tester);
      expect(
        tester.getSemantics(rows.at(1)),
        isSemantics(isButton: true, hasTapAction: true),
      );
      handle.dispose();
    });
  });

  group('transit', () {
    testWidgets('a scrub reads out the time and what\'s happening, in the '
        'times\' place, selects its journey and draws it, where the map is', (
      tester,
    ) async {
      await picker.open(tester);
      await showMode(tester, TravelMode.transit);
      await moveTo(tester, const SheetAnchor.full());
      final subjects = picker.map.subjectsSet;
      final row = journeyRows().at(1);
      final gesture = await scrub(tester, timelineOf(1), at: 0.75);
      expect(
        tester.widgetList<JourneyRow>(journeyRows()).map((r) => r.selected),
        [false, true],
      );
      expect(picker.map.subjectsSet, subjects, reason: 'no fit to it');
      await tester.pump(const Duration(milliseconds: 300));
      final readout = scrubOf(tester, row).readout;
      expect(readout.first, matches(RegExp(r'^\d+:\d\d [AP]M$')));
      expect(readout, hasLength(2));
      expect(readoutOf(tester, row).painted, startsWith(readout.first));
      expect(marker(), isNotNull);
      await gesture.up();
      await tester.pumpAndSettle();
    });

    testWidgets('a scrub across a journey says each thing it does', (
      tester,
    ) async {
      await picker.open(tester);
      await showMode(tester, TravelMode.transit);
      final rect = tester.getRect(timelineOf(0));
      final gesture = await scrub(tester, timelineOf(0), at: 0);
      final seen = <String>{};
      for (var x = rect.left; x <= rect.right; x += 4) {
        await gesture.moveTo(Offset(x, rect.center.dy));
        await tester.pump();
        seen.add(scrubOf(tester, journeyRows().at(0)).readout.last);
      }
      expect(
        seen,
        containsAll(['walking', startsWith('waiting at '), 'on 40', 'on 7']),
      );
      await gesture.up();
      await tester.pumpAndSettle();
    });
  });

  /// Shows [mode]'s rows, by its tab, or as the tab would if the tab is out of
  /// reach at the largest text.
  Future<void> show(WidgetTester tester, TravelMode mode) async {
    await tester.tap(
      find.descendant(of: tabs, matching: find.byIcon(mode.icon)),
      warnIfMissed: false,
    );
    await tester.pumpAndSettle();
    if (tester.widget<ModeTabs>(tabs).selected != mode) {
      tester.widget<ModeTabs>(tabs).onChanged(mode);
      await tester.pumpAndSettle();
    }
  }

  /// Leaves out what the pages under directions overflow by at large text, which
  /// isn't this page's to fix.
  void ignoreOtherPages() {
    final report = FlutterError.onError;
    FlutterError.onError = (details) {
      final text = details.toString();
      if (text.contains('lib/search/search_sheet.dart') ||
          text.contains('lib/home/navigation_bar.dart')) {
        return;
      }
      report?.call(details);
    };
    addTearDown(() => FlutterError.onError = report);
  }

  group('nothing moves', () {
    for (final (platform, scale) in [
      for (final platform in [TargetPlatform.android, TargetPlatform.iOS])
        for (final scale in [1.0, 1.3, 1.5, 2.0, 3.1]) (platform, scale),
    ]) {
      for (final size in [appScreen, const Size(320, 568)]) {
        testWidgets('as a row is scrubbed in every way, at $scale× in '
            '${size.width.round()}×${size.height.round()} on '
            '${platform.name}', (tester) async {
          tester.platformDispatcher.textScaleFactorTestValue = scale;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          ignoreOtherPages();
          await picker.open(tester, size: size, platform: platform);
          for (final mode in TravelMode.values) {
            await moveTo(tester, headerAnchor);
            await show(tester, mode);
            await moveTo(tester, footerAnchor);
            final transit = mode == TravelMode.transit;
            final row = transit ? journeyRows().first : rows.first;
            final graph = transit
                ? timelineOf(0)
                : find.descendant(
                    of: row,
                    matching: find.byType(
                      mode == TravelMode.driving
                          ? SpeedProfile
                          : ElevationProfile,
                    ),
                  );
            final places = slotRects(tester);
            final held = tester.getRect(row);
            final texts = {
              for (final text
                  in find
                      .descendant(of: row, matching: find.byType(Text))
                      .evaluate())
                text: tester.getRect(find.byElementPredicate((e) => e == text)),
            };
            final gesture = await scrub(tester, graph, at: 0.6);
            await tester.pump(const Duration(milliseconds: 300));
            expect(tester.takeException(), isNull, reason: '$mode');
            expect(slotRects(tester), places, reason: '$mode');
            expect(tester.getRect(row), held, reason: '$mode');
            for (final MapEntry(key: text, value: rect) in texts.entries) {
              expect(
                tester.getRect(find.byElementPredicate((e) => e == text)),
                rect,
                reason: '$mode',
              );
            }
            final readout = readoutOf(tester, row);
            expect(readout.painted, isNotEmpty, reason: '$mode');
            await gesture.up();
            await tester.pumpAndSettle();
            expect(slotRects(tester), places, reason: '$mode, after');
          }
        });
      }
    }
  });
}
