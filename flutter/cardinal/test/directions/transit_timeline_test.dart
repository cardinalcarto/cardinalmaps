import 'dart:math' as math;

import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/journey_list.dart';
import 'package:cardinal/directions/transit_timeline.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_transit.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

TimelinePiece piece(TimelineKind kind, int start, int end) =>
    TimelinePiece(kind: kind, start: at(start), end: at(end));

void main() {
  timelineEases();
  group('pieces', () {
    test('follow the legs, with waits filling the time between', () {
      final journey = fakePlan().journeys.single;
      final pieces = timelinePieces(journey);
      expect(pieces.map((p) => p.kind), [
        TimelineKind.street, // to the stop
        TimelineKind.wait, // for the late 40
        TimelineKind.ride, // 40
        TimelineKind.ride, // stays on as the 7
        TimelineKind.street, // to Link
        TimelineKind.wait,
        TimelineKind.ride, // 1 Line
        TimelineKind.street, // to the destination
      ]);
      // Back to back, from leaving to arriving.
      expect(pieces.first.start, journey.departure);
      expect(pieces.last.end, journey.arrival);
      for (var i = 1; i < pieces.length; i++) {
        expect(pieces[i].start, pieces[i - 1].end);
      }
    });

    test('a stay-seated ride is one piece per trip, joined', () {
      final rides = timelinePieces(fakePlan().journeys.single)
          .where((p) => p.kind == TimelineKind.ride)
          .toList();
      expect(rides.map((p) => p.route?.name), ['40', '7', '1 Line']);
      expect(rides.map((p) => p.continues), [true, false, false]);
      // The 7 has no color of its own, so it takes the ride's.
      expect(rides[1].route?.color, 'FDB71A');
      expect(
        timelineLabel(timelinePieces(fakePlan().journeys.single)),
        '40, then 7, then 1 Line',
      );
    });
  });

  group('placing', () {
    final axis = TimelineAxis(at(0), at(3600));

    test('puts pieces at their times', () {
      final placed = placePieces(
        [
          piece(TimelineKind.street, 0, 600),
          piece(TimelineKind.ride, 600, 2400),
          piece(TimelineKind.street, 2400, 3600),
        ],
        axis,
        360,
      );
      expect(placed.map((p) => (p.left, p.right)), [
        (0, 60),
        (60, 240),
        (240, 360),
      ]);
    });

    test('keeps a short ride wide enough to read, pushing only the next', () {
      final placed = placePieces(
        [
          piece(TimelineKind.ride, 600, 660), // 6 px at its times
          piece(TimelineKind.wait, 660, 720),
          piece(TimelineKind.ride, 720, 2400),
        ],
        axis,
        360,
      );
      expect(placed[0].left, 60);
      expect(placed[0].width, timelineMinWidths[TimelineKind.ride]);
      // The wait is squeezed to its minimum; the long ride still ends on time.
      expect(placed[1].left, placed[0].right);
      expect(placed[1].width, timelineMinWidths[TimelineKind.wait]);
      expect(placed[2].left, placed[1].right);
      expect(placed[2].right, 240);
    });

    test('never passes the right edge, moving what comes before left', () {
      final placed = placePieces(
        [
          piece(TimelineKind.street, 3000, 3540),
          piece(TimelineKind.ride, 3540, 3600),
        ],
        axis,
        360,
      );
      expect(placed.last.right, 360);
      expect(placed.last.width, timelineMinWidths[TimelineKind.ride]);
      expect(placed.first.right, placed.last.left);
      expect(placed.first.left, 300);
    });

    test('shares out the width when the minimums alone are too wide', () {
      final placed = placePieces(
        [
          for (var i = 0; i < 10; i++)
            piece(TimelineKind.ride, i * 60, i * 60 + 60),
        ],
        axis,
        200,
      );
      expect(placed.first.left, 0);
      expect(placed.last.right, closeTo(200, 1e-9));
      for (var i = 1; i < placed.length; i++) {
        expect(placed[i].left, closeTo(placed[i - 1].right, 1e-9));
      }
    });
  });

  test('the axis spans the journeys, marked at round times', () {
    final plan = fakePlan(journeys: 3);
    final axis = TimelineAxis.of(plan.journeys);
    expect(axis.start, plan.journeys.first.departure);
    expect(axis.end, plan.journeys.last.arrival);
    // 65 minutes over 360 px: marks every 15 minutes, on the quarter hour.
    final ticks = axis.ticks(360, inset: 0);
    expect(ticks, hasLength(4));
    for (final (i, t) in ticks.indexed) {
      expect(t.minute % 15, 0);
      expect(t.isAfter(axis.start) && t.isBefore(axis.end), isTrue);
      if (i > 0) expect(t.difference(ticks[i - 1]).inMinutes, 15);
    }
    // 18:30 is within a label's half-width of the right end, so it isn't marked.
    expect(axis.ticks(360), ticks.take(3));
  });

  testWidgets('rows show when to go and how long, on the shared axis', (
    tester,
  ) async {
    final plan = fakePlan(journeys: 2);
    final axis = TimelineAxis.of(plan.journeys);
    var tapped = -1;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Column(
            children: [
              for (final (i, j) in plan.journeys.indexed)
                JourneyRow(
                  journey: j,
                  axis: axis,
                  // 12 minutes before the first journey leaves.
                  now: at(-12 * 60),
                  onTap: () => tapped = i,
                ),
            ],
          ),
        ),
      ),
    );
    expect(find.textContaining('Go in 12 min'), findsOneWidget);
    expect(find.textContaining('Go in 22 min'), findsOneWidget);
    expect(find.text('45 min'), findsNWidgets(2));
    expect(find.byType(JourneyTimeline), findsNWidgets(2));
    await tester.tap(find.textContaining('Go in 22 min'));
    expect(tapped, 1);
  });

  testWidgets('"Go now" once it is time to leave', (tester) async {
    final journey = fakePlan().journeys.single;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: GoIn(journey: journey, now: at(30)),
        ),
      ),
    );
    expect(find.text('Go now'), findsOneWidget);
  });

  group('other routes', () {
    test('a stay-seated ride\'s are said once, with its first trip', () {
      final journey = fakePlan().journeys.single;
      final pieces = timelinePieces(journey);
      expect(pieces.where((p) => p.ride != null).map((p) => p.ride), [0, 0, 1]);
      final route = connectionsJourney().rides.first.departures[3].route!;
      expect(
        timelineLabel(
          pieces,
          others: [
            [route],
            [],
          ],
        ),
        '40 or 7, then 7, then 1 Line',
      );
    });

    test('are said with the rides they can be taken for', () {
      final journey = connectionsJourney();
      expect(
        timelineLabel(timelinePieces(journey), others: otherRoutes(journey)),
        '40 or 7, then 1 Line, then 8 or 48 or 9, then 2',
      );
    });

    test('sit over their rides, as near as they fit', () {
      // Each at its anchor when there's room.
      expect(
        placeOrLabels(
          [0, 100],
          [
            [40, 70],
            [40],
          ],
          300,
        ),
        [(left: 0.0, count: 2), (left: 100.0, count: 1)],
      );
      // Pushed right past the one before, and then fewer badges to fit.
      expect(
        placeOrLabels(
          [0, 50],
          [
            [40, 70],
            [40, 70],
          ],
          140,
        ),
        [(left: 0.0, count: 2), (left: 78.0, count: 1)],
      );
      // Pulled back from the right edge; dropped when there's no room at all.
      expect(
        placeOrLabels(
          [0, 280, 290],
          [
            [40],
            [40],
            [40],
          ],
          300,
        ),
        [(left: 0.0, count: 1), (left: 260.0, count: 1), (left: 0.0, count: 0)],
      );
    });

    testWidgets('an overview row keeps room for them above its timeline', (
      tester,
    ) async {
      Future<double> height(Journey journey) async {
        await tester.pumpWidget(
          MaterialApp(
            home: Scaffold(
              body: JourneyRow(
                journey: journey,
                axis: TimelineAxis.of([journey]),
                now: at(0),
                onTap: () {},
              ),
            ),
          ),
        );
        return tester.getSize(find.byType(JourneyTimeline)).height;
      }

      // The same, with or without any: every row keeps the line's room.
      expect(
        await height(fakePlan().journeys.single),
        26 + JourneyTimeline.orHeight + JourneyTimeline.orGap,
      );
      expect(
        await height(connectionsJourney()),
        26 + JourneyTimeline.orHeight + JourneyTimeline.orGap,
      );
      expect(
        tester.getSemantics(find.byType(JourneyTimeline)).label,
        contains('40 or 7, then 1 Line, then 8 or 48 or 9, then 2'),
      );
    });
  });

  test('pieces follow the departures chosen', () {
    final journey = connectionsJourney();
    var chain = ConnectionChain(journey);
    chain = chain.select(0, 5);
    final pieces = timelinePieces(journey, chain: chain);
    final rides = pieces.where((p) => p.kind == TimelineKind.ride).toList();
    expect(rides.map((p) => p.start), [at(1500), at(2460), at(3180), at(3960)]);
    expect(pieces.first.start, at(900));
    expect(pieces.last.end, at(4620));
    for (var i = 1; i < pieces.length; i++) {
      expect(pieces[i].start, pieces[i - 1].end);
    }
    // Another route's, as one piece.
    chain = ConnectionChain(journey).select(0, 3);
    expect(
      timelinePieces(
        journey,
        chain: chain,
      ).firstWhere((p) => p.ride == 0).route!.name,
      '7',
    );
  });
}

void timelineEases() {
  testWidgets('a timeline following choices eases from one to the next', (
    tester,
  ) async {
    final journey = connectionsJourney();
    var chain = ConnectionChain(journey);
    late StateSetter update;
    TimelineAxis axisOf(ConnectionChain c) =>
        TimelineAxis(c.departure, c.arrival!);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              update = setState;
              return JourneyTimeline(
                journey: journey,
                chain: chain,
                axis: axisOf(chain),
              );
            },
          ),
        ),
      ),
    );
    final state = tester.state<JourneyTimelineState>(
      find.byType(JourneyTimeline),
    );
    DrawnPiece ride(int r) =>
        state.drawn.lastWhere((p) => p.piece.ride == r && p.opacity > 0);
    final before = ride(0);
    // A 7 in place of the 40: later, another color, another label.
    update(() => chain = chain.select(0, 3));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    final mid = ride(0);
    await tester.pumpAndSettle();
    final after = ride(0);
    expect(after.piece.route!.name, '7');
    expect(after.left, isNot(moreOrLessEquals(before.left)));
    expect(
      mid.left,
      inExclusiveRange(
        math.min(before.left, after.left),
        math.max(before.left, after.left),
      ),
    );
    expect(mid.color, isNot(before.color));
    expect(mid.color, isNot(after.color));
    // The 40's label fading out under the 7's.
    expect(mid.fromRoute?.name, '40');
    expect(mid.labelT, inExclusiveRange(0, 1));
    expect(after.fromRoute, isNull);
  });
}
