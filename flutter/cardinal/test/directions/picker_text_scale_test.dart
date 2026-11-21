import 'package:cardinal/directions/directions_page.dart';
import 'package:cardinal/directions/mode_tabs.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/steps_page.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/trip_buttons.dart';
import 'package:cardinal/directions/trip_card.dart';
import 'package:cardinal/directions/trip_details_page.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoButton, CupertinoListTile;
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_routes.dart';
import '../fakes/fake_transit.dart';
import 'picker_harness.dart';

/// The system text sizes the sheet is built at, from the default to the largest
/// accessibility size.
const _scales = [1.0, 1.3, 1.5, 2.0, 3.1];

/// A current phone's window, and a small one's.
const _windows = [Size(390, 844), Size(320, 568)];

/// Fails if any text of the directions sheet is cut off, or of [page]'s if it's given:
/// taller than the box that holds it, running past its lines where it has no ellipsis
/// to say so, or wider than its one line.
void _expectNoTextCutOff(WidgetTester tester, String state, {Type? page}) {
  final content = find.descendant(
    of: page == null ? find.byType(DirectionsPage) : find.byType(page),
    matching: find.byType(SheetContent),
  );
  final paragraphs = find.descendant(
    of: content,
    matching: find.byType(RichText),
  );
  expect(paragraphs, findsWidgets);
  for (final element in paragraphs.evaluate()) {
    final widget = element.widget as RichText;
    final paragraph = element.renderObject! as RenderParagraph;
    final what = '"${widget.text.toPlainText()}" ($state)';
    // The lines it was given room for are all the lines it's laid out in.
    expect(
      paragraph.textSize.height,
      lessThanOrEqualTo(paragraph.size.height + 0.5),
      reason: 'the height of $what',
    );
    expect(
      paragraph.size.height,
      lessThanOrEqualTo(paragraph.constraints.maxHeight + 0.5),
      reason: 'the box of $what',
    );
    final ellipsis = widget.overflow == TextOverflow.ellipsis;
    if (!ellipsis) {
      // Nothing says it was cut, so none of it may be: all of its lines show, and
      // none is wider than its box.
      expect(
        paragraph.didExceedMaxLines,
        isFalse,
        reason: 'the lines of $what',
      );
      expect(
        paragraph.textSize.width,
        lessThanOrEqualTo(paragraph.size.width + 0.5),
        reason: 'the width of $what',
      );
    }
  }
}

/// Matches extents that are [extents], give or take what adding the same heights up
/// in another order comes to.
Matcher _near(List<double> extents) => pairwiseCompare<double, double>(
  extents,
  (expected, actual) => (expected - actual).abs() < 1e-9,
  'within a billionth of a pixel of',
);

void main() {
  late Picker picker;

  setUp(() => picker = Picker());

  /// Shows [mode] by tapping its tab, which is in reach at every size: where a held
  /// row would cover the tabs wherever the sheet is, none is held.
  Future<void> show(WidgetTester tester, TravelMode mode) async {
    await showMode(tester, mode);
    expect(tester.widget<ModeTabs>(tabs).selected, mode);
  }

  /// Lets the pages around directions overflow as they do at the largest sizes: the
  /// search sheet's header, under the place's card under directions, and iOS's
  /// navigation bar. That's not what these tests are about, and anything else that
  /// overflows still fails them.
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

  for (final (platform, window, scale) in [
    for (final platform in [TargetPlatform.android, TargetPlatform.iOS])
      for (final window in _windows)
        for (final scale in _scales) (platform, window, scale),
  ]) {
    {
      final where =
          'at $scale× in ${window.width.round()}×${window.height.round()} on '
          '${platform.name}';

      Future<void> open(WidgetTester tester) async {
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        ignoreOtherPages();
        picker.answerNothing();
        await picker.open(tester, size: window, platform: platform);
      }

      testWidgets('nothing overflows or is cut off, and the anchors hold, '
          '$where', (tester) async {
        await open(tester);
        final sheet = sheetOf(tester);
        List<double> anchors() => [
          extentOf(sheet, footerAnchor),
          extentOf(sheet, headerAnchor),
        ];
        final row = OptionRowMetrics.heightOf(tester.element(tabs));
        final header = tester.getSize(tabs);
        final buttons = tester.getSize(footer);

        /// One state of the sheet: laid out without error, with every row as tall
        /// as a row is at this scale, and the sheet resting where it did.
        void check(String state, List<double> rests) {
          expect(tester.takeException(), isNull, reason: state);
          _expectNoTextCutOff(tester, state);
          for (final slot in slots.evaluate()) {
            expect(slot.size!.height, row, reason: 'a row, $state');
          }
          expect(tester.getSize(tabs), header, reason: 'the tabs, $state');
          expect(tester.getSize(footer), buttons, reason: 'buttons, $state');
          expect(anchors(), _near(rests), reason: 'the anchors, $state');
        }

        final rests = anchors();
        check('loading', rests);
        expect(isRoomyText(TextScaler.linear(scale)), scale >= 1.5);
        for (final mode in TravelMode.values) {
          await show(tester, mode);
          check('$mode, loading', rests);
        }

        picker.asked(TravelMode.driving).single.answer.complete(drivingRoutes);
        picker.asked(TravelMode.walking).single.answer.complete(walkingRoutes);
        picker.asked(TravelMode.cycling).single.answer.complete(cyclingRoutes);
        picker.transit.asked.single.answer.complete(
          TransitPlan.fromJson({
            ...fakePlanJson(journeys: 2, tags: ['a', 'b']),
            // One with other routes to name over its rides.
            'journeys': [
              ...fakePlanJson(journeys: 2, tags: ['a', 'b'])['journeys']!
                  as List<Object?>,
              ...connectionsPlanJson()['journeys']! as List<Object?>,
            ],
          }),
        );
        await tester.pumpAndSettle();
        for (final mode in TravelMode.values) {
          await show(tester, mode);
          check('$mode', rests);
          // With every row in view, and at each anchor.
          await moveTo(tester, const SheetAnchor.full());
          check('$mode, full', rests);
          await moveTo(tester, footerAnchor);
          check('$mode, at the lowest anchor', rests);
          await moveTo(tester, headerAnchor);
          check('$mode, at the header anchor', rests);
        }
      });

      testWidgets('nor with no route, $where', (tester) async {
        await open(tester);
        final sheet = sheetOf(tester);
        final rests = [
          extentOf(sheet, footerAnchor),
          extentOf(sheet, headerAnchor),
        ];
        final row = OptionRowMetrics.heightOf(tester.element(tabs));
        // The longest of the messages, and the shortest.
        picker
            .asked(TravelMode.driving)
            .single
            .answer
            .completeError(const OutsideOfflineArea());
        picker.asked(TravelMode.walking).single.answer.complete([]);
        picker
            .asked(TravelMode.cycling)
            .single
            .answer
            .completeError(http.ClientException('offline'));
        picker.transit.asked.single.answer.complete(emptyPlan('no_egress'));
        await tester.pumpAndSettle();
        for (final mode in TravelMode.values) {
          await show(tester, mode);
          for (final anchor in [
            const SheetAnchor.full(),
            footerAnchor,
            headerAnchor,
          ]) {
            await moveTo(tester, anchor);
            final state = '$mode with no route, at $anchor';
            expect(tester.takeException(), isNull, reason: state);
            _expectNoTextCutOff(tester, state);
            expect(find.byType(OptionMessage), findsOneWidget);
            expect(tester.getSize(slots).height, row, reason: state);
            expect(
              [extentOf(sheet, footerAnchor), extentOf(sheet, headerAnchor)],
              _near(rests),
              reason: state,
            );
          }
        }
      });
    }
  }

  for (final (platform, window, scale) in [
    for (final platform in [TargetPlatform.android, TargetPlatform.iOS])
      for (final window in _windows)
        for (final scale in _scales) (platform, window, scale),
  ]) {
    final where =
        'at $scale× in ${window.width.round()}×${window.height.round()} on '
        '${platform.name}';

    Future<void> open(WidgetTester tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      ignoreOtherPages();
      await picker.open(tester, size: window, platform: platform);
    }

    /// Checks [page] at each place the sheet rests on it, and with its content
    /// scrolled to its end.
    Future<void> checkPage(WidgetTester tester, Type page) async {
      final sheet = SheetScaffold.controllerOf(
        tester.element(find.byType(page)),
      );
      void check(String state) {
        expect(tester.takeException(), isNull, reason: state);
        _expectNoTextCutOff(tester, state, page: page);
      }

      check('$page, opened');
      sheet.animateTo(const SheetAnchor.full());
      await tester.pumpAndSettle();
      check('$page, full');
      await tester.fling(
        find
            .descendant(
              of: find.byType(page),
              matching: find.byType(Scrollable),
            )
            .first,
        const Offset(0, -20000),
        8000,
      );
      await tester.pumpAndSettle();
      check('$page, scrolled to its end');
    }

    testWidgets('the steps: nothing overflows or is cut off, $where', (
      tester,
    ) async {
      await open(tester);
      // The one with a long step, and one that arrives.
      routeTaps.add(2);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Steps'));
      await tester.pumpAndSettle();
      expect(find.byType(StepsPage), findsOneWidget);
      await checkPage(tester, StepsPage);
    });
  }

  for (final (platform, window, scale) in [
    for (final platform in [TargetPlatform.android, TargetPlatform.iOS])
      for (final window in _windows)
        for (final scale in _scales) (platform, window, scale),
  ]) {
    final where =
        'at $scale× in ${window.width.round()}×${window.height.round()} on '
        '${platform.name}';

    testWidgets('with road details, elevation, tolls and the legend: nothing '
        'overflows or is cut off, and the rows and anchors hold, $where', (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      ignoreOtherPages();
      picker.routes.immediatelyBy
        ..[TravelMode.driving] = detailedDrivingRoutes
        ..[TravelMode.cycling] = detailedCyclingRoutes
        ..[TravelMode.walking] = detailedWalkingRoutes;
      await picker.open(tester, size: window, platform: platform);
      final sheet = sheetOf(tester);
      List<double> anchors() => [
        extentOf(sheet, footerAnchor),
        extentOf(sheet, headerAnchor),
      ];
      final rests = anchors();
      final row = OptionRowMetrics.heightOf(tester.element(tabs));
      for (final mode in [
        TravelMode.driving,
        TravelMode.walking,
        TravelMode.cycling,
      ]) {
        await showMode(tester, mode);
        await moveTo(tester, const SheetAnchor.full());
        final state = '$mode with road details';
        expect(tester.takeException(), isNull, reason: state);
        expect(find.byType(WayLegend), findsOneWidget, reason: state);
        _expectNoTextCutOff(tester, state);
        for (final slot in slots.evaluate()) {
          expect(slot.size!.height, row, reason: 'a row, $state');
        }
        expect(anchors(), rests, reason: 'the anchors, $state');
        // The legend below the rows, wrapped as far as it needs.
        final legend = tester.getRect(find.byType(WayLegend));
        expect(legend.top, greaterThanOrEqualTo(slotRects(tester).last.bottom));
        await moveTo(tester, headerAnchor);
      }

      // And the steps, with their arrows.
      await showMode(tester, TravelMode.driving);
      await tester.tap(find.text('Steps'));
      await tester.pumpAndSettle();
      expect(find.byType(StepsPage), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(StepsPage),
          matching: find.byType(Icon),
        ),
        findsWidgets,
      );
      expect(tester.takeException(), isNull);
      _expectNoTextCutOff(tester, 'the steps with arrows', page: StepsPage);
    });
  }

  for (final (platform, window, scale) in [
    for (final platform in [TargetPlatform.android, TargetPlatform.iOS])
      for (final window in _windows)
        for (final scale in _scales) (platform, window, scale),
  ]) {
    testWidgets('a journey\'s details: nothing overflows or is cut off, at '
        '$scale× in ${window.width.round()}×${window.height.round()} on '
        '${platform.name}', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      ignoreOtherPages();
      // A journey of four rides, with other routes' departures to choose from,
      // after one with a moved stop, an alert and a stay-seated ride.
      for (final plan in [
        fakePlan(),
        TransitPlan.fromJson(connectionsPlanJson()),
      ]) {
        picker = Picker();
        picker.transit.immediately = plan;
        // Opened on transit, as the way last chosen.
        await picker.store.settings.set(lastTravelMode, TravelMode.transit);
        await picker.open(tester, size: window, platform: platform);
        await tester.tap(find.text('Trip details'));
        await tester.pumpAndSettle();
        expect(find.byType(TripDetailsPage), findsOneWidget);
        final sheet = SheetScaffold.controllerOf(
          tester.element(find.byType(TripDetailsPage)),
        );
        void check(String state) {
          expect(tester.takeException(), isNull, reason: state);
          _expectNoTextCutOff(tester, state, page: TripDetailsPage);
        }

        check('opened');
        sheet.animateTo(const SheetAnchor.full());
        await tester.pumpAndSettle();
        check('full');
        final scrollable = find
            .descendant(
              of: find.byType(TripDetailsPage),
              matching: find.byType(Scrollable),
            )
            .first;
        // Down through all of it, a window at a time.
        final position = tester.state<ScrollableState>(scrollable).position;
        for (
          var step = 0;
          step < 100 && position.pixels < position.maxScrollExtent - 0.5;
          step++
        ) {
          position.jumpTo(
            (position.pixels + window.height / 2).clamp(
              0,
              position.maxScrollExtent,
            ),
          );
          await tester.pump();
          check('scrolled to ${position.pixels.round()}');
        }
      }
    });
  }

  testWidgets('a tab\'s time is scaled no further than 1.3×', (tester) async {
    Future<Size> tabsAt(double scale) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      await tester.pumpAndSettle();
      return tester.getSize(find.byType(ModeTabs));
    }

    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await picker.open(tester);
    final normal = await tabsAt(1);
    final capped = await tabsAt(1.3);
    expect(capped.height, greaterThan(normal.height));
    expect(await tabsAt(2), capped);
    expect(await tabsAt(3.1), capped);
  });

  testWidgets('from 1.5× the arrival has a line of its own, and the name and '
      'caption two', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 1.5;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await picker.open(tester);
    await moveTo(tester, const SheetAnchor.full());
    Rect inRow(int index, String text) => tester.getRect(
      find.descendant(of: rows.at(index), matching: find.text(text)),
    );
    expect(
      inRow(0, 'Arrive 3:42 PM').top,
      greaterThanOrEqualTo(inRow(0, '22 min').bottom),
    );
    expect(inRow(0, 'Arrive 3:42 PM').left, inRow(0, '22 min').left);
    final caption = tester.widget<Text>(
      find.descendant(
        of: rows.at(2),
        matching: find.textContaining('No freeways'),
      ),
    );
    expect(caption.maxLines, 2);
    expect(
      tester
          .widget<Text>(
            find.descendant(
              of: rows.at(2),
              matching: find.text('via East Marginal Way South'),
            ),
          )
          .maxLines,
      2,
    );
  });

  /// Fails unless all of [finder]'s widget is in the window, above the buttons unless
  /// it's one of them, and a tap at its middle lands on it.
  void expectTappable(WidgetTester tester, Finder finder, String what) {
    final rect = tester.getRect(finder);
    final window = tester.view.physicalSize / tester.view.devicePixelRatio;
    final bottom =
        find.descendant(of: footer, matching: finder).evaluate().isEmpty
        ? footerTop(tester)
        : window.height;
    expect(rect.top, greaterThanOrEqualTo(-0.5), reason: 'the top of $what');
    expect(rect.left, greaterThanOrEqualTo(-0.5), reason: 'the start of $what');
    expect(
      rect.right,
      lessThanOrEqualTo(window.width + 0.5),
      reason: 'the end of $what',
    );
    expect(
      rect.bottom,
      lessThanOrEqualTo(bottom + 0.5),
      reason: 'the bottom of $what',
    );
    final box = tester.renderObject(finder);
    expect(
      tester
          .hitTestOnBinding(rect.center)
          .path
          .any((entry) => entry.target == box),
      isTrue,
      reason: 'a tap on $what',
    );
  }

  for (final (platform, window, scale) in [
    for (final platform in [TargetPlatform.android, TargetPlatform.iOS])
      for (final window in _windows)
        for (final scale in _scales) (platform, window, scale),
  ]) {
    final where =
        'at $scale× in ${window.width.round()}×${window.height.round()} on '
        '${platform.name}';

    testWidgets('the ends, every tab and the buttons can be tapped at the '
        'header anchor, and the selected row read in full, $where', (
      tester,
    ) async {
      ignoreOtherPages();
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await picker.open(tester, size: window, platform: platform);
      // Held, unless a held row and the buttons would leave the ends and tabs no
      // room in the small window: from 2× there.
      final holds = window.height > 600 || scale < 2;
      expect(heldSlot(tester), holds ? OptionList.slotId(0) : isNull);
      await moveTo(tester, headerAnchor);
      for (final end in ['From', 'To']) {
        expectTappable(
          tester,
          find
              .ancestor(
                of: find.text(end),
                matching: find.byWidgetPredicate(
                  (widget) => widget is InkWell || widget is CupertinoListTile,
                ),
              )
              .first,
          end,
        );
      }
      for (final mode in TravelMode.values.reversed) {
        final tab = find.descendant(of: tabs, matching: find.byIcon(mode.icon));
        expectTappable(tester, tab, 'the tab of $mode');
        await tester.tap(tab);
        await tester.pumpAndSettle();
        expect(tester.widget<ModeTabs>(tabs).selected, mode);
        expect(sheetOf(tester).restingAt, headerAnchor);
        final buttons = find.descendant(
          of: footer,
          matching: find.byWidgetPredicate(
            (widget) => widget is FilledButton || widget is CupertinoButton,
          ),
        );
        expect(buttons, findsWidgets);
        for (var i = 0; i < buttons.evaluate().length; i++) {
          expectTappable(tester, buttons.at(i), 'button $i of $mode');
        }
      }

      // Driving's selected row, scrolled to, as high as the sheet goes.
      await moveTo(tester, const SheetAnchor.full());
      final row = rows.first;
      expect(tester.widget<OptionRow>(row).selected, isTrue);
      await Scrollable.ensureVisible(tester.element(row));
      await tester.pumpAndSettle();
      final rect = tester.getRect(row);
      expect(rect.top, greaterThanOrEqualTo(-0.5));
      expect(rect.bottom, lessThanOrEqualTo(footerTop(tester) + 0.5));
    });

    testWidgets('the heights the page tells whether a row fits by are the '
        'heights laid out, $where', (tester) async {
      ignoreOtherPages();
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await picker.open(tester, size: window, platform: platform);
      final page = tester.element(find.byType(DirectionsPage));
      Matcher near(double height) => moreOrLessEquals(height, epsilon: 1e-6);
      expect(
        TripCard.heightOf(page),
        near(tester.getSize(find.byType(TripCard)).height),
      );
      expect(ModeTabs.heightOf(page), near(tester.getSize(tabs).height));
      expect(
        TripButtons.heightOf(
          page,
          bottomPadding: SheetScaffold.viewOf(page).geometry.padding.bottom,
        ),
        near(tester.getSize(footer).height),
      );
    });
  }

  // What the sheet rests at, the lowest anchor and the header anchor, as it did
  // before rows could go unheld: in a current phone's window, a row is held at every
  // text size, and nothing about it changed.
  for (final (platform, scale, rests) in [
    (TargetPlatform.android, 1.0, [243.0, 398.0]),
    (TargetPlatform.android, 1.3, [262.0, 428.0]),
    (TargetPlatform.android, 1.5, [353.0, 529.0]),
    (TargetPlatform.android, 2.0, [411.0, 611.0]),
    (TargetPlatform.iOS, 1.0, [249.0, 437.5]),
    (TargetPlatform.iOS, 1.3, [272.0, 465.5]),
    (TargetPlatform.iOS, 1.5, [373.0, 566.5]),
    (TargetPlatform.iOS, 2.0, [440.0, 633.5]),
  ]) {
    testWidgets('at $scale× in 390×844 on ${platform.name}, the first row is '
        'held under the tabs at the anchors it always had', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await picker.open(tester, size: const Size(390, 844), platform: platform);
      final sheet = sheetOf(tester);
      expect(heldSlot(tester), OptionList.slotId(0));
      expect(
        [extentOf(sheet, footerAnchor), extentOf(sheet, headerAnchor)],
        pairwiseCompare<double, double>(
          rests,
          (expected, actual) => (expected - actual).abs() < 0.05,
          'within a twentieth of a pixel of',
        ),
      );
      await moveTo(tester, headerAnchor);
      final header = find.byWidgetPredicate(
        (widget) => widget is SheetSection && widget.id == #directionsHeader,
      );
      expect(
        tester.getRect(rows.first).top,
        moreOrLessEquals(tester.getRect(header).bottom),
      );
      expect(
        tester.getRect(rows.first).bottom,
        moreOrLessEquals(footerTop(tester)),
      );
      await moveTo(tester, footerAnchor);
      expect(
        tester.getRect(rows.first).bottom,
        moreOrLessEquals(footerTop(tester)),
      );
    });
  }

  testWidgets('a change of text size holds a row, or none, in the frame it '
      'lays out at the new size', (tester) async {
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    ignoreOtherPages();
    await picker.open(tester, size: const Size(320, 568));
    expect(heldSlot(tester), OptionList.slotId(0));
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    await tester.pump();
    expect(heldSlot(tester), isNull);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(sheetOf(tester).restingAt, headerAnchor);
    await showMode(tester, TravelMode.walking);
    expect(tester.widget<ModeTabs>(tabs).selected, TravelMode.walking);
    tester.platformDispatcher.textScaleFactorTestValue = 1;
    await tester.pump();
    expect(heldSlot(tester), OptionList.slotId(0));
    await tester.pumpAndSettle();
    expect(sheetOf(tester).restingAt, headerAnchor);
    expect(
      tester.getRect(rows.first).bottom,
      moreOrLessEquals(footerTop(tester)),
    );
  });
}
