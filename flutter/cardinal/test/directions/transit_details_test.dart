import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/transit_details.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_transit.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

/// Holds a chain of [journey]'s departures, chosen as [build]'s widget says.
class _Chained extends StatefulWidget {
  const _Chained({required this.journey, required this.build});

  final Journey journey;
  final Widget Function(
    ConnectionChain chain,
    void Function(int ride, int departure) select,
  )
  build;

  @override
  State<_Chained> createState() => _ChainedState();
}

class _ChainedState extends State<_Chained> {
  late var _chain = ConnectionChain(widget.journey);

  @override
  Widget build(BuildContext context) => widget.build(
    _chain,
    (ride, departure) =>
        setState(() => _chain = _chain.select(ride, departure)),
  );
}

void main() {
  final journey = fakePlan().journeys.single;

  Future<void> pump(WidgetTester tester, Widget child) => tester.pumpWidget(
    MaterialApp(
      home: Scaffold(body: SingleChildScrollView(child: child)),
    ),
  );

  /// The departures of ride [ride] of [of], choosing as the rider taps.
  Future<void> range(
    WidgetTester tester,
    int ride, {
    Journey? of,
    TransitAccess access = TransitAccess.walking,
  }) => pump(
    tester,
    _Chained(
      journey: of ?? journey,
      build: (chain, select) => DepartureRange(
        choice: chain.rides[ride],
        access: access,
        now: at(0),
        onSelect: (departure) => select(ride, departure),
      ),
    ),
  );

  /// The tile for the departure at [time].
  Finder tile(String time) =>
      find.ancestor(of: find.text(time), matching: find.byType(ConnectionTile));

  group('departure tiles', () {
    testWidgets('the suggested one says so, and is described', (tester) async {
      await range(tester, 0);
      expect(find.byType(ConnectionTile), findsNWidgets(6));
      final own = tester.widget<ConnectionTile>(tile('5:40 PM'));
      expect(own.departure.suggested, isTrue);
      expect(own.selected, isTrue);
      expect(
        find.descendant(of: tile('5:40 PM'), matching: find.text('Suggested')),
        findsOneWidget,
      );
      expect(find.text('Planned'), findsNothing);
      // Counting down from 5:30, with the live mark for a live time.
      expect(
        find.descendant(of: tile('5:40 PM'), matching: find.text('10')),
        findsOneWidget,
      );
      expect(
        find.descendant(
          of: tile('5:40 PM'),
          matching: find.byIcon(Icons.sensors),
        ),
        findsOneWidget,
      );
      // All the 40's, so no badges.
      expect(find.byType(RouteBadge), findsNothing);
      // How it's running, in one place.
      expect(find.text('Live: 4 min late'), findsOneWidget);
      expect(find.text('Wheelchair accessible'), findsOneWidget);
      expect(find.text('Route 40 detoured'), findsOneWidget);
    });

    testWidgets('the countdown ticks each second', (tester) async {
      await range(tester, 0);
      expect(
        find.descendant(of: tile('5:52 PM'), matching: find.text('22')),
        findsOneWidget,
      );
      await tester.pump(const Duration(seconds: 1));
      await tester.pumpAndSettle();
      expect(
        find.descendant(of: tile('5:52 PM'), matching: find.text('21')),
        findsOneWidget,
      );
    });

    testWidgets('departures the rider misses are muted and can\'t be chosen', (
      tester,
    ) async {
      await range(tester, 0);
      expect(find.text('Missed'), findsNWidgets(2));
      await tester.tap(tile('5:35 PM'));
      await tester.pump();
      // Still describing the suggested one.
      expect(find.text('Live: 4 min late'), findsOneWidget);
      expect(tester.widget<ConnectionTile>(tile('5:40 PM')).selected, isTrue);
    });

    testWidgets('a later one, chosen, is described instead', (tester) async {
      await range(tester, 0);
      expect(
        find.descendant(
          of: tile('5:52 PM'),
          matching: find.byIcon(Icons.sensors),
        ),
        findsNothing,
      );
      await tester.tap(tile('5:52 PM'));
      await tester.pump();
      expect(tester.widget<ConnectionTile>(tile('5:52 PM')).selected, isTrue);
      // That it's scheduled, not accessible, and has an alert the suggested one doesn't.
      expect(find.text('Scheduled'), findsOneWidget);
      expect(find.text('Not wheelchair accessible'), findsOneWidget);
      expect(find.text('Trip 40c ends at Westlake'), findsOneWidget);
      expect(find.text('Route 40 detoured'), findsNothing);
      // Its alert's description is a tap away.
      expect(
        find.text('Riders continuing south should transfer.'),
        findsNothing,
      );
      await tester.tap(find.text('Trip 40c ends at Westlake'));
      await tester.pump();
      expect(
        find.text('Riders continuing south should transfer.'),
        findsOneWidget,
      );
    });

    testWidgets('a canceled departure is struck through and can\'t be chosen', (
      tester,
    ) async {
      await range(tester, 0);
      expect(find.text('Canceled'), findsOneWidget);
      final time = tester.widget<Text>(find.text('6:00 PM'));
      expect(time.style?.decoration, TextDecoration.lineThrough);
      await tester.tap(tile('6:00 PM'));
      await tester.pump();
      expect(find.text('Live: 4 min late'), findsOneWidget);
    });

    testWidgets('the day\'s last is marked, and says so when chosen', (
      tester,
    ) async {
      await range(tester, 0);
      expect(find.text('Last'), findsOneWidget);
      await tester.tap(tile('6:04 PM'));
      await tester.pump();
      expect(
        find.text('The last departure of the day between these stops'),
        findsOneWidget,
      );
    });

    testWidgets('a connection that is the night\'s last says so at once', (
      tester,
    ) async {
      await range(tester, 1);
      expect(find.text('Suggested · Last'), findsOneWidget);
      expect(find.text('Missed'), findsOneWidget);
      expect(
        find.text('The last departure of the day between these stops'),
        findsOneWidget,
      );
      expect(find.text('Scheduled'), findsOneWidget);
      expect(find.text('Wheelchair access unknown'), findsOneWidget);
    });

    testWidgets('departures of several routes each badge theirs', (
      tester,
    ) async {
      await range(tester, 0, of: connectionsJourney());
      final badges = tester.widgetList<RouteBadge>(find.byType(RouteBadge));
      expect(badges.map((b) => b.route!.shortName), [
        '40',
        '62',
        '40',
        '7',
        '7',
        '40',
        '31',
        '40',
      ]);
    });

    testWidgets('one leaving past the hour shows its time', (tester) async {
      await pump(
        tester,
        _Chained(
          journey: connectionsJourney(),
          build: (chain, select) => DepartureRange(
            choice: chain.rides[3],
            access: TransitAccess.walking,
            now: at(0),
            onSelect: (departure) => select(3, departure),
          ),
        ),
      );
      // The 2 at 18:21 is 51 minutes off; the one at 18:36, 66.
      expect(
        find.descendant(of: tile('6:21 PM'), matching: find.text('51')),
        findsOneWidget,
      );
      expect(
        tester.widget<Text>(
          find.descendant(
            of: find.byType(ConnectionTile).last,
            matching: find.byKey(const ValueKey('countdown')),
          ),
        ),
        isA<Text>().having((t) => t.data, 'text', '6:36 PM'),
      );
    });
  });

  group('itinerary', () {
    Future<void> itinerary(
      WidgetTester tester,
      Journey journey, {
      TransitAccess access = TransitAccess.walking,
      bool header = false,
    }) => tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: _Chained(
            journey: journey,
            build: (chain, select) => CustomScrollView(
              slivers: [
                if (header)
                  SliverToBoxAdapter(
                    child: JourneyHeader(
                      chain: chain,
                      now: at(0),
                      units: UnitSystem.imperial,
                      onBack: () {},
                    ),
                  ),
                JourneyItinerary(
                  chain: chain,
                  onSelect: select,
                  start: 'Waiting for the Interurban',
                  destination: 'Chu Minh',
                  units: UnitSystem.imperial,
                  access: access,
                  now: at(0),
                ),
              ],
            ),
          ),
        ),
      ),
    );

    testWidgets('runs from the start to the destination, ride by ride', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 2400));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await itinerary(tester, journey);
      expect(find.text('Waiting for the Interurban'), findsOneWidget);
      expect(find.text('Leave 5:30 PM'), findsOneWidget);
      expect(find.text('Chu Minh'), findsOneWidget);
      expect(find.text('Arrive 6:15 PM'), findsOneWidget);
      // Each ride, with its route, where it's headed, and where it's boarded and left.
      expect(find.byType(RouteBadge), findsNWidgets(3));
      expect(find.text('Downtown Seattle'), findsOneWidget);
      expect(find.text('Angle Lake'), findsOneWidget);
      expect(find.text('Westlake Station'), findsOneWidget);
      expect(find.text('Capitol Hill Station'), findsNWidgets(1));
      // The wait at the first stop.
      expect(find.text('Wait 5 min'), findsOneWidget);
      // The moved stop, beside the walk to it.
      expect(find.text('Fremont Ave N & N 34th St has moved'), findsOneWidget);
      expect(
        find.text('Board at the temporary stop 50 feet north.'),
        findsOneWidget,
      );
    });

    testWidgets('a stay-seated ride goes on as the next route, in one ride', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 2400));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await itinerary(tester, journey);
      expect(find.text('Stay on board'), findsOneWidget);
      expect(find.text('At 3rd Ave & Pike St it continues as'), findsOneWidget);
      expect(find.text('to Rainier Beach'), findsOneWidget);
      // Each trip's stops between are a tap away.
      expect(find.text('3rd Ave & Pike St'), findsNothing);
    });

    testWidgets('stops along the way open with a tap', (tester) async {
      await tester.binding.setSurfaceSize(const Size(400, 2400));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final json = fakePlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      // One trip, so the 40 has a stop between.
      ((legs[1] as Map)['segments'] as List).clear();
      await itinerary(tester, TransitPlan.fromJson(json).journeys.single);
      expect(find.text('3rd Ave & Pike St'), findsNothing);
      await tester.tap(find.text('1 stop before'));
      await tester.pump();
      expect(find.text('3rd Ave & Pike St'), findsOneWidget);
    });

    testWidgets('a change at the same stop has a row of its own', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 2400));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final json = fakePlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      // Link from the stop the bus leaves the rider at, with no walk between.
      legs.removeAt(2);
      final linkStops = (legs[2] as Map)['stops']! as List;
      (linkStops.first as Map)['name'] = 'Westlake Station';
      await itinerary(tester, TransitPlan.fromJson(json).journeys.single);
      expect(
        find.text('Change at Westlake Station, wait 1 min'),
        findsOneWidget,
      );
    });

    testWidgets('a wheelchair rider\'s street legs are named for it', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 2400));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final json = fakePlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      for (final leg in legs.cast<Map<String, Object?>>()) {
        if (leg['type'] != 'transit') leg['mode'] = 'wheelchair';
      }
      await itinerary(
        tester,
        TransitPlan.fromJson(json).journeys.single,
        access: TransitAccess.wheelchair,
      );
      expect(find.textContaining('Wheelchair 5 min'), findsOneWidget);
      expect(find.textContaining('Go '), findsNothing);
    });

    /// The tile at [time] among the departures of the ride that's leg [leg].
    Finder rideTile(int leg, String time) => find.descendant(
      of: find.byKey(ValueKey(('departures', leg))),
      matching: tile(time),
    );

    /// Taps that tile, scrolled to.
    Future<void> choose(WidgetTester tester, int leg, String time) async {
      await tester.ensureVisible(rideTile(leg, time));
      await tester.pumpAndSettle();
      await tester.tap(rideTile(leg, time));
      await tester.pump();
    }

    bool selected(WidgetTester tester, int leg, String time) =>
        tester.widget<ConnectionTile>(rideTile(leg, time)).selected;

    testWidgets('a later first ride re-times the rides after it', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 4000));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await itinerary(tester, connectionsJourney(), header: true);
      expect(find.text('5:30 PM – 6:32 PM'), findsOneWidget);
      expect(find.text('Leave 5:30 PM'), findsOneWidget);
      expect(find.text('Arrive 6:32 PM'), findsOneWidget);
      expect(selected(tester, 3, '5:55 PM'), isTrue);

      await choose(tester, 1, '5:55 PM');
      // Leaving as much later, so there's as long to get to the stop.
      expect(find.text('5:45 PM – 6:47 PM'), findsOneWidget);
      expect(find.text('Leave 5:45 PM'), findsOneWidget);
      expect(find.text('Arrive 6:47 PM'), findsOneWidget);
      // Each ride after on the first it can make.
      expect(selected(tester, 3, '6:11 PM'), isTrue);
      expect(selected(tester, 5, '6:23 PM'), isTrue);
      expect(selected(tester, 7, '6:36 PM'), isTrue);
      // Link's earlier ones can't be made now.
      expect(
        find.descendant(
          of: find.byKey(const ValueKey(('departures', 3))),
          matching: find.text('Missed'),
        ),
        findsNWidgets(3),
      );
      expect(find.text('Wait 3 min'), findsOneWidget);

      // And back.
      await choose(tester, 1, '5:40 PM');
      expect(find.text('Arrive 6:32 PM'), findsOneWidget);
      expect(selected(tester, 3, '5:55 PM'), isTrue);
    });

    testWidgets(
      'another route\'s departure shows its route, and only its ends',
      (tester) async {
        await tester.binding.setSurfaceSize(const Size(400, 4000));
        addTearDown(() => tester.binding.setSurfaceSize(null));
        await itinerary(tester, connectionsJourney());
        expect(find.text('1 stop before'), findsNWidgets(4));
        await tester.tap(find.text('1 stop before').first);
        await tester.pump();
        expect(find.text('Stop A to Stop B, between'), findsOneWidget);

        await choose(tester, 1, '5:43 PM');
        // Its route and headsign above, and when it leaves and arrives.
        final title = find.ancestor(
          of: find.text('Rainier Beach').first,
          matching: find.byType(Row),
        );
        expect(
          find.descendant(of: title.first, matching: find.byType(RouteBadge)),
          findsOneWidget,
        );
        expect(find.text('Downtown Seattle'), findsNothing);
        expect(find.text('5:43 PM'), findsNWidgets(2));
        expect(find.text('5:52 PM'), findsOneWidget);
        // Its stops between aren't known.
        expect(find.text('Stop A to Stop B, between'), findsNothing);
        expect(find.text('1 stop before'), findsNWidgets(3));
        // Back on the suggested one, they're a tap away again.
        await choose(tester, 1, '5:40 PM');
        expect(find.text('1 stop before'), findsNWidgets(4));
        expect(find.text('Stop A to Stop B, between'), findsNothing);
      },
    );

    testWidgets('a ride with no connection says so', (tester) async {
      await tester.binding.setSurfaceSize(const Size(400, 4000));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await itinerary(
        tester,
        connectionsJourney(lateLastRide: false),
        header: true,
      );
      await choose(tester, 1, '5:55 PM');
      expect(find.byType(NoConnection), findsOneWidget);
      expect(
        find.textContaining("You'd be at the stop at 6:35 PM"),
        findsOneWidget,
      );
      expect(find.text('5:45 PM – no connection'), findsOneWidget);
      // The destination's row says so too.
      expect(find.text('No connection'), findsNWidgets(2));
    });

    testWidgets('the rides after one with no connection are unknown', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 4000));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final json = connectionsPlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      ((legs[3] as Map)['departures']! as List).removeLast();
      await itinerary(tester, TransitPlan.fromJson(json).journeys.single);
      await choose(tester, 1, '5:55 PM');
      expect(find.byType(NoConnection), findsOneWidget);
      // No departures to choose from, and no times.
      expect(find.byKey(const ValueKey(('departures', 5))), findsNothing);
      expect(find.byKey(const ValueKey(('departures', 7))), findsNothing);
      expect(find.text('6:08 PM'), findsNothing);
      expect(find.text('Mount Baker'), findsOneWidget);
    });
  });
}
