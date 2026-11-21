import 'package:cardinal/directions/journey_list.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/scrub.dart';
import 'package:cardinal/directions/speed_profile.dart' show routeGraphHeight;
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/transit_sheet.dart' show liveColor;
import 'package:cardinal/directions/transit_timeline.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_transit.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

/// The fake plan's journey, its JSON changed by [change]: a walk to a stop that has
/// moved, the 40 staying on as the 7 with live times, a walk within Westlake, the
/// 1 Line, and a walk from it.
Journey journeyWith(void Function(List<Map<String, Object?>> legs) change) {
  final json = fakePlanJson();
  final journey = (json['journeys']! as List).single as Map<String, Object?>;
  final legs = (journey['legs']! as List).cast<Map<String, Object?>>();
  change(legs);
  journey['legs'] = legs;
  return TransitPlan.fromJson(json).journeys.single;
}

/// The legs of [legs] that are on the streets.
Iterable<Map<String, Object?>> streets(List<Map<String, Object?>> legs) =>
    legs.where((leg) => leg['type'] != 'transit');

const _platforms = [TargetPlatform.android, TargetPlatform.iOS];

void main() {
  final row = find.byType(JourneyRow);
  final timeline = find.byType(JourneyTimeline);
  final times = find.textContaining(' – ');

  /// A row of [journey], [width] wide, 12 minutes before the fake plan's leaves.
  Future<void> pumpRow(
    WidgetTester tester,
    Journey journey, {
    double width = 800,
    TargetPlatform platform = TargetPlatform.android,
    bool selected = false,
    DateTime? now,
    ScrubSink? scrub,
  }) => tester.pumpWidget(
    MaterialApp(
      theme: appTheme(Brightness.light, platform: platform),
      home: Scaffold(
        body: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: width,
            child: JourneyRow(
              journey: journey,
              axis: TimelineAxis.of([journey]),
              now: now ?? at(-12 * 60),
              selected: selected,
              onTap: () {},
              scrub: scrub,
            ),
          ),
        ),
      ),
    ),
  );

  OptionRowMetrics metricsOf(WidgetTester tester) =>
      OptionRowMetrics.of(tester.element(timeline));

  /// Where the row's lines are: within its fill and its padding.
  Rect contentOf(WidgetTester tester) {
    final metrics = metricsOf(tester);
    final rect = tester.getRect(row);
    return Rect.fromLTRB(
      rect.left + metrics.margin + metrics.padding.left,
      rect.top + metrics.padding.top,
      rect.right - metrics.margin - metrics.padding.right,
      rect.bottom - metrics.spacing - metrics.padding.bottom,
    );
  }

  /// The style [finder]'s text is written in.
  TextStyle styleOf(WidgetTester tester, Finder finder) => tester
      .renderObject<RenderParagraph>(
        find.descendant(of: finder, matching: find.byType(RichText)),
      )
      .text
      .style!;

  /// The timeline's painting, of its rides and the other routes under them.
  Finder painting() =>
      find.descendant(of: timeline, matching: find.byType(CustomPaint));

  /// Where each ride of the timeline starts.
  Map<int, double> rideLefts(WidgetTester tester) => {
    for (final piece in tester.state<JourneyTimelineState>(timeline).drawn)
      if (piece.key case ('ride', final int ride, 0)) ride: piece.left,
  };

  Matcher startsAt(double x) => predicate<Offset>(
    (offset) => (offset.dx - x).abs() < 1e-6,
    'an offset $x across',
  );

  /// A sink that takes no notice.
  final sink = ScrubSink(
    onStart: (_, select) => select(),
    onMove: (_, _) {},
    onEnd: (_) {},
  );

  /// Scrubs the row's timeline, to the middle of it.
  Future<TestGesture> scrub(WidgetTester tester) async {
    final rect = tester.getRect(timeline);
    final start = Offset(rect.center.dx, rect.top + 20);
    final gesture = await tester.startGesture(start);
    await gesture.moveBy(const Offset(24, 0));
    await tester.pump();
    await gesture.moveTo(start);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    return gesture;
  }

  ReadoutPainter readoutOf(WidgetTester tester) => tester
      .widgetList<CustomPaint>(
        find.descendant(of: row, matching: find.byType(CustomPaint)),
      )
      .map((paint) => paint.painter)
      .whereType<ReadoutPainter>()
      .single;

  /// Where the readout is painted.
  Rect readoutRect(WidgetTester tester) => tester.getRect(
    find
        .ancestor(
          of: find.byWidgetPredicate(
            (w) => w is CustomPaint && w.painter is ReadoutPainter,
          ),
          matching: find.byType(Positioned),
        )
        .first,
  );

  group('what a journey\'s row says after its times', () {
    test('one transfer, a ride that stays on as another trip being one '
        'vehicle, and the minutes on the streets', () {
      final journey = fakePlan().journeys.single;
      // Three trips on two vehicles.
      expect(journey.rides, hasLength(2));
      expect(journey.rides.first.segments, hasLength(2));
      expect(journeyFacts(journey), ['1 transfer', '10 min walking']);
    });

    test('no transfers goes unsaid', () {
      // The 40 that stays on as the 7, alone.
      final journey = journeyWith((legs) => legs.removeRange(2, 4));
      expect(journey.rides.single.segments, hasLength(2));
      expect(journeyFacts(journey), ['7 min walking']);
    });

    test('several transfers', () {
      expect(journeyFacts(connectionsJourney()), [
        '3 transfers',
        '14 min walking',
      ]);
    });

    test('the streets by how stops are reached', () {
      Journey by(String mode) => journeyWith((legs) {
        for (final leg in streets(legs)) {
          leg['mode'] = mode;
        }
      });
      expect(journeyFacts(by('walking')).last, '10 min walking');
      expect(journeyFacts(by('cycling')).last, '10 min cycling');
      expect(journeyFacts(by('wheelchair')).last, '10 min by wheelchair');
    });

    test('the streets to the nearest minute, and unsaid under half of one', () {
      Journey taking(int seconds) => journeyWith((legs) {
        for (final leg in streets(legs)) {
          leg['duration_seconds'] = seconds;
        }
      });
      expect(journeyFacts(taking(1300)), ['1 transfer', '1 hr 5 min walking']);
      expect(journeyFacts(taking(60)), ['1 transfer', '3 min walking']);
      expect(journeyFacts(taking(10)), ['1 transfer', '1 min walking']);
      expect(journeyFacts(taking(9)), ['1 transfer']);
      expect(journeyFacts(taking(0)), ['1 transfer']);
    });
  });

  for (final platform in _platforms) {
    group('on ${platform.name}', () {
      testWidgets('a row is a street row\'s four lines: when to go and how '
          'long, its times and the rest, its timeline, the other routes', (
        tester,
      ) async {
        await pumpRow(tester, connectionsJourney(), platform: platform);
        final metrics = metricsOf(tester);
        final content = contentOf(tester);
        expect(tester.getSize(row).height, metrics.height);
        final goIn = find.textContaining('Go in');
        final took = find.text('1 hr 2 min');
        // When to go, as large as a street row's time; how long, as its arrival.
        expect(
          tester.widget<Text>(goIn).textSpan!.toPlainText(),
          'Go in 12 min',
        );
        expect(styleOf(tester, goIn).fontSize, metrics.headline.fontSize);
        expect(styleOf(tester, goIn).fontWeight, metrics.headline.fontWeight);
        expect(tester.widget<Text>(took).style, metrics.arrival);
        expect(tester.getRect(goIn).left, content.left);
        expect(tester.getRect(took).right, content.right);
        expect(
          tester.getRect(took).left,
          greaterThan(tester.getRect(goIn).right),
        );
        final headline = tester.getRect(goIn).bottom;
        expect(headline, content.top + metrics.headlineHeight);
        // Its times and the rest, as a street row's name.
        final second = tester.widget<Text>(times);
        expect(second.data, '5:30 PM – 6:32 PM · 3 transfers · 14 min walking');
        expect(second.style, metrics.name);
        expect(second.maxLines, 1);
        expect(
          tester.getRect(times),
          Rect.fromLTRB(
            content.left,
            headline + OptionRowMetrics.gap,
            content.right,
            headline + OptionRowMetrics.gap + metrics.nameHeight,
          ),
        );
        // Its timeline where a street row's graph is, and under that the line a
        // street row's caption has.
        expect(
          tester.getRect(timeline),
          Rect.fromLTRB(
            content.left,
            tester.getRect(times).bottom + OptionRowMetrics.gap,
            content.right,
            content.bottom,
          ),
        );
        expect(
          tester.getSize(timeline).height,
          routeGraphHeight + OptionRowMetrics.gap + metrics.captionHeight,
        );
        expect(tester.takeException(), isNull);
      });

      testWidgets('from 1.5× how long has a line of its own, and its times '
          'and the rest two, in the height every row has', (tester) async {
        for (final scale in [1.5, 2.0, 3.1]) {
          tester.platformDispatcher.textScaleFactorTestValue = scale;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          await pumpRow(
            tester,
            connectionsJourney(),
            width: 390,
            platform: platform,
          );
          final metrics = metricsOf(tester);
          final content = contentOf(tester);
          expect(metrics.roomy, isTrue);
          expect(tester.getSize(row).height, metrics.height);
          final goIn = tester.getRect(find.textContaining('Go in'));
          final took = tester.getRect(find.text('1 hr 2 min'));
          expect(goIn.top, greaterThanOrEqualTo(content.top));
          expect(took.top, greaterThanOrEqualTo(goIn.bottom));
          expect(took.left, goIn.left);
          expect(tester.widget<Text>(times).maxLines, 2);
          final second = tester.getRect(times);
          expect(
            second.top,
            moreOrLessEquals(
              content.top +
                  metrics.headlineHeight +
                  metrics.arrivalHeight +
                  OptionRowMetrics.gap,
            ),
          );
          expect(second.height, metrics.nameHeight);
          // The timeline and the other routes under it, whole, in what's left.
          final graph = tester.getRect(timeline);
          expect(
            graph.top,
            moreOrLessEquals(second.bottom + OptionRowMetrics.gap),
          );
          expect(graph.bottom, lessThanOrEqualTo(content.bottom + 1e-6));
          expect(
            graph.height,
            greaterThanOrEqualTo(
              JourneyTimeline.barHeightOf(tester.element(timeline)) +
                  OptionRowMetrics.gap +
                  metrics.captionHeight / 2 -
                  1e-6,
            ),
          );
          // As many of their badges as the line has room for.
          expect(painting(), paints..rrect());
          expect(tester.takeException(), isNull, reason: 'at $scale×');
        }
      });
    });
  }

  group('when to go', () {
    /// The parts of what the row says of it.
    List<TextSpan> goInSpans(WidgetTester tester) =>
        (tester.widget<Text>(find.textContaining('Go ')).textSpan! as TextSpan)
            .children!
            .cast<TextSpan>();

    testWidgets('has its minutes in the live color when the first ride\'s '
        'times are live', (tester) async {
      await pumpRow(tester, fakePlan().journeys.single);
      final theme = Theme.of(tester.element(row));
      final spans = goInSpans(tester);
      expect(spans.map((span) => span.text), ['Go in ', '12 min']);
      expect(spans.first.style, isNull);
      expect(spans.last.style!.color, liveColor(theme));
      expect(
        styleOf(tester, find.textContaining('Go in')).color,
        theme.colorScheme.onSurface,
      );
    });

    testWidgets('has none of it in the live color when they aren\'t', (
      tester,
    ) async {
      await pumpRow(tester, journeyWith((legs) => legs[1]['realtime'] = false));
      final spans = goInSpans(tester);
      expect(spans.map((span) => span.style), [null, null]);
    });

    testWidgets('is "Go now" once it is time', (tester) async {
      final journey = fakePlan().journeys.single;
      await pumpRow(tester, journey, now: at(30));
      final theme = Theme.of(tester.element(row));
      expect(find.text('Go now'), findsOneWidget);
      expect(goInSpans(tester).single.style!.color, liveColor(theme));
    });

    testWidgets('is in the primary color on the selected row, and nothing '
        'moves', (tester) async {
      final journey = journeyWith((legs) => legs[1]['realtime'] = false);
      await pumpRow(tester, journey);
      final goIn = find.textContaining('Go in');
      final scheme = Theme.of(tester.element(row)).colorScheme;
      Map<String, Rect> rects() => {
        for (final text in tester.widgetList<Text>(
          find.descendant(of: row, matching: find.byType(Text)),
        ))
          text.data ?? text.textSpan!.toPlainText(): tester.getRect(
            find.byWidget(text),
          ),
        'timeline': tester.getRect(timeline),
      };
      final before = rects();
      expect(styleOf(tester, goIn).color, scheme.onSurface);
      await pumpRow(tester, journey, selected: true);
      await tester.pumpAndSettle();
      expect(styleOf(tester, goIn).color, scheme.primary);
      // How long it takes stays as it was, as a street row's arrival does.
      expect(
        tester.widget<Text>(find.text('45 min')).style,
        metricsOf(tester).arrival,
      );
      expect(rects(), before);
    });
  });

  group('its times and the rest', () {
    RenderParagraph paragraph(WidgetTester tester) =>
        tester.renderObject<RenderParagraph>(
          find.descendant(of: times, matching: find.byType(RichText)),
        );

    testWidgets('are cut from the end where the line is short, and the mark '
        'of a stop that has moved stays', (tester) async {
      final journey = fakePlan().journeys.single;
      await pumpRow(tester, journey);
      expect(
        tester.widget<Text>(times).data,
        '5:30 PM – 6:15 PM · 1 transfer · 10 min walking',
      );
      expect(tester.widget<Text>(times).overflow, TextOverflow.ellipsis);
      expect(paragraph(tester).didExceedMaxLines, isFalse);
      final mark = find.byIcon(Icons.wrong_location);
      expect(tester.getRect(mark).right, contentOf(tester).right);

      await pumpRow(tester, journey, width: 320);
      expect(paragraph(tester).didExceedMaxLines, isTrue);
      // What's cut is the end of it: the times are first, and whole.
      final shown = paragraph(tester).getBoxesForSelection(
        const TextSelection(baseOffset: 0, extentOffset: 17),
      );
      expect(shown.last.right, lessThanOrEqualTo(tester.getSize(times).width));
      final content = contentOf(tester);
      expect(tester.getRect(mark).right, content.right);
      expect(
        tester.getRect(mark).left,
        greaterThan(tester.getRect(times).right),
      );
      expect(tester.getRect(mark).top, greaterThanOrEqualTo(content.top));
      expect(
        tester.getRect(mark).bottom,
        lessThanOrEqualTo(tester.getRect(times).bottom),
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('have no mark, and the whole line, with no stop moved', (
      tester,
    ) async {
      await pumpRow(tester, connectionsJourney());
      expect(find.byIcon(Icons.wrong_location), findsNothing);
      expect(tester.getRect(times).right, contentOf(tester).right);
    });

    testWidgets('change in place when a newer plan changes only the times', (
      tester,
    ) async {
      await pumpRow(tester, fakePlan().journeys.single);
      final where = tester.getRect(times);
      await pumpRow(tester, fakePlan(shifts: [120]).journeys.single);
      expect(
        tester.widget<Text>(times).data,
        '5:32 PM – 6:17 PM · 1 transfer · 10 min walking',
      );
      expect(tester.getRect(times), where);
      // Only the timeline is on its way anywhere.
      await tester.pump(const Duration(milliseconds: 50));
      expect(times, findsOneWidget);
      await tester.pumpAndSettle();
    });

    testWidgets('fade into what a newer plan makes of the rest, where they '
        'are', (tester) async {
      await pumpRow(tester, fakePlan().journeys.single);
      final where = tester.getRect(times);
      final height = tester.getSize(row).height;
      // Four minutes more from the last stop.
      await pumpRow(
        tester,
        journeyWith((legs) => legs.last['duration_seconds'] = 360),
      );
      await tester.pump(const Duration(milliseconds: 100));
      expect(times, findsNWidgets(2));
      double opacityOf(String text) => tester
          .widget<FadeTransition>(
            find
                .ancestor(
                  of: find.textContaining(text),
                  matching: find.byType(FadeTransition),
                )
                .first,
          )
          .opacity
          .value;
      expect(opacityOf('10 min walking'), inExclusiveRange(0, 1));
      expect(opacityOf('14 min walking'), inExclusiveRange(0, 1));
      expect(tester.getRect(times.first), where);
      expect(tester.getRect(times.last), where);
      expect(tester.getSize(row).height, height);
      await tester.pumpAndSettle();
      expect(
        tester.widget<Text>(times).data,
        '5:30 PM – 6:15 PM · 1 transfer · 14 min walking',
      );
      expect(tester.getRect(times), where);
    });
  });

  group('the other routes', () {
    testWidgets('are badged under the timeline, each from the left of its '
        'ride', (tester) async {
      await pumpRow(tester, connectionsJourney());
      final metrics = metricsOf(tester);
      final lefts = rideLefts(tester);
      expect(lefts.keys, [0, 1, 2, 3]);
      // The next thing painted that's a badge, which is as tall as the line.
      bool badge(Symbol method, List<dynamic> arguments) {
        if (method != #drawRRect) return false;
        final rrect = arguments.first as RRect;
        if (rrect.top != 0 || rrect.bottom != metrics.captionHeight) {
          throw 'a badge from ${rrect.top} to ${rrect.bottom} of the line';
        }
        return true;
      }

      expect(
        painting(),
        paints
          // The line under the timeline's place.
          ..translate(x: 0.0, y: routeGraphHeight + OptionRowMetrics.gap)
          // "or 7" under the 40.
          ..paragraph(offset: startsAt(lefts[0]! + 1))
          ..something(badge)
          ..paragraph()
          // "or 48, 9" under the 8.
          ..paragraph(offset: startsAt(lefts[2]! + 1))
          ..something(badge)
          ..paragraph()
          ..something(badge)
          ..paragraph()
          // The timeline, in the middle of its place.
          ..translate(x: 0.0, y: (routeGraphHeight - 26) / 2)
          ..path(),
      );
      // No more of them than those.
      expect(
        painting(),
        isNot(
          paints
            ..rrect()
            ..rrect()
            ..rrect()
            ..rrect(),
        ),
      );
    });

    testWidgets('leave their line blank, and as tall, when there are none', (
      tester,
    ) async {
      await pumpRow(tester, connectionsJourney());
      final withSome = tester.getRect(timeline);
      await pumpRow(tester, fakePlan().journeys.single);
      await tester.pumpAndSettle();
      expect(tester.getRect(timeline), withSome);
      expect(painting(), isNot(paints..rrect()));
      expect(
        painting(),
        paints
          ..translate(x: 0.0, y: routeGraphHeight + OptionRowMetrics.gap)
          ..translate(x: 0.0, y: (routeGraphHeight - 26) / 2)
          ..path(),
      );
    });
  });

  group('a scrub\'s readout', () {
    testWidgets('takes the place of its times and the rest, whole, and '
        'leaves when to go and how long', (tester) async {
      await pumpRow(tester, fakePlan().journeys.single, scrub: sink);
      final goIn = find.textContaining('Go in');
      final took = find.text('45 min');
      final where = tester.getRect(times);
      final rects = [tester.getRect(goIn), tester.getRect(took)];
      final height = tester.getSize(row).height;
      double fade(Finder text) => tester
          .widgetList<FadeTransition>(
            find.descendant(
              of: row,
              matching: find.ancestor(
                of: text,
                matching: find.byType(FadeTransition),
              ),
            ),
          )
          .fold(1, (shown, fade) => shown * fade.opacity.value);
      expect(fade(times), 1);
      final gesture = await scrub(tester);
      final readout = readoutOf(tester);
      expect(readout.scrub.shown.value, 1);
      expect(readout.scrub.readout, hasLength(2));
      expect(readout.painted, readout.scrub.readout.join(' · '));
      expect(readout.lines, 1);
      expect(readout.style.fontSize, metricsOf(tester).name.fontSize);
      expect(
        readout.style.color,
        Theme.of(tester.element(row)).colorScheme.primary,
      );
      expect(readoutRect(tester), where);
      expect(fade(times), 0);
      // The mark of a stop that has moved isn't part of what it replaces.
      expect(fade(find.byIcon(Icons.wrong_location)), 1);
      expect(fade(goIn), 1);
      expect(fade(took), 1);
      expect([tester.getRect(goIn), tester.getRect(took)], rects);
      expect(tester.getRect(times), where);
      expect(tester.getSize(row).height, height);
      await gesture.up();
      await tester.pumpAndSettle();
      expect(fade(times), 1);
      expect(readout.scrub.shown.value, 0);
    });

    testWidgets('at the roomier sizes takes both lines of them', (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await pumpRow(
        tester,
        fakePlan().journeys.single,
        width: 390,
        scrub: sink,
      );
      final metrics = metricsOf(tester);
      final where = tester.getRect(times);
      final took = tester.getRect(find.text('45 min'));
      expect(where.height, metrics.nameHeight);
      final gesture = await scrub(tester);
      final readout = readoutOf(tester);
      expect(readout.lines, 2);
      expect(readout.painted, readout.scrub.readout.join(' · '));
      expect(readoutRect(tester), where);
      expect(tester.getRect(find.text('45 min')), took);
      expect(tester.getRect(times), where);
      expect(tester.takeException(), isNull);
      await gesture.up();
      await tester.pumpAndSettle();
    });
  });

  group('to a screen reader', () {
    testWidgets('a row says all of it in order: when to go, how long, its '
        'times, transfers, the streets, its rides and the other routes', (
      tester,
    ) async {
      final handle = tester.ensureSemantics();
      await pumpRow(tester, connectionsJourney(), width: 320);
      expect(
        tester.getSemantics(row),
        isSemantics(
          isButton: true,
          hasTapAction: true,
          hasSelectedState: true,
          label:
              'Go in 12 min, 1 hr 2 min, 5:30 PM – 6:32 PM, 3 transfers, '
              '14 min walking, 40 or 7, then 1 Line, then 8 or 48 or 9, then 2',
        ),
      );
      await pumpRow(tester, fakePlan().journeys.single, width: 320);
      expect(
        tester.getSemantics(row).label,
        'Go in 12 min, 45 min, 5:30 PM – 6:15 PM, 1 transfer, '
        '10 min walking, 40, then 7, then 1 Line, A stop has moved',
      );
      handle.dispose();
    });

    testWidgets('a row that scrubs says the same, with its timeline a slider', (
      tester,
    ) async {
      final handle = tester.ensureSemantics();
      await pumpRow(tester, fakePlan().journeys.single, scrub: sink);
      expect(
        tester.getSemantics(row).label,
        'Go in 12 min, 45 min, 5:30 PM – 6:15 PM, 1 transfer, '
        '10 min walking, 40, then 7, then 1 Line, A stop has moved',
      );
      expect(
        tester.getSemantics(find.bySemanticsLabel('Along the journey')),
        isSemantics(isSlider: true, hasIncreaseAction: true),
      );
      handle.dispose();
    });
  });
}
