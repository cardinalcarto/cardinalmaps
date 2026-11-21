import 'package:cardinal/directions/mode_tabs.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:material_ui/material_ui.dart';

import 'picker_harness.dart';

/// The system text sizes the sheet is built at, from the default to the largest
/// accessibility size.
const _scales = [1.0, 1.3, 1.5, 2.0, 3.1];

/// A current phone's window, and a small one's.
const _windows = [Size(390, 844), Size(320, 568)];

/// Fails if any text of the directions sheet is cut off: taller than the box that
/// holds it, running past its lines where it has no ellipsis to say so, or wider than
/// its one line.
void _expectNoTextCutOff(WidgetTester tester, String state) {
  final content = find.ancestor(of: tabs, matching: find.byType(SheetContent));
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

void main() {
  late Picker picker;

  setUp(() => picker = Picker());

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
        final header = tester.getRect(tabs).size;
        final buttons = tester.getSize(footer);

        /// One state of the sheet: laid out without error, with every row as tall
        /// as a row is at this scale, and the sheet resting where it did.
        void check(String state, List<double> rests) {
          expect(tester.takeException(), isNull, reason: state);
          _expectNoTextCutOff(tester, state);
          for (final slot in slots.evaluate()) {
            expect(slot.size!.height, row, reason: 'a row, $state');
          }
          expect(tester.getRect(tabs).size, header, reason: 'the tabs, $state');
          expect(tester.getSize(footer), buttons, reason: 'buttons, $state');
          expect(anchors(), rests, reason: 'the anchors, $state');
        }

        final rests = anchors();
        check('loading', rests);
        expect(isRoomyText(TextScaler.linear(scale)), scale >= 1.5);

        picker.asked(TravelMode.driving).single.answer.complete(drivingRoutes);
        picker.asked(TravelMode.walking).single.answer.complete(walkingRoutes);
        picker.asked(TravelMode.cycling).single.answer.complete(cyclingRoutes);
        await tester.pumpAndSettle();
        for (final mode in [
          TravelMode.driving,
          TravelMode.walking,
          TravelMode.cycling,
        ]) {
          await showMode(tester, mode);
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
        await tester.pumpAndSettle();
        for (final mode in [
          TravelMode.driving,
          TravelMode.walking,
          TravelMode.cycling,
        ]) {
          await showMode(tester, mode);
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
              rests,
              reason: state,
            );
          }
        }
      });
    }
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
}
