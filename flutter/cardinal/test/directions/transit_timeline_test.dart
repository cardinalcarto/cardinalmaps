import 'package:cardinal/directions/transit_timeline.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_transit.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

TimelinePiece piece(TimelineKind kind, int start, int end) =>
    TimelinePiece(kind: kind, start: at(start), end: at(end));

void main() {
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
}
