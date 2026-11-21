import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/departure_legs.dart';
import 'package:cardinal/directions/transit_details.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/units.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_transit.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

/// Holds a chain of [journey]'s departures, chosen as [build]'s widget says, and with
/// [legs], of [plan], the legs fetched of the departures it takes but the plan's.
class _Chained extends StatefulWidget {
  const _Chained({
    required this.journey,
    required this.build,
    this.legs,
    this.plan,
  });

  final Journey journey;
  final DepartureLegs? legs;
  final TransitPlan? plan;
  final Widget Function(
    ConnectionChain chain,
    void Function(int ride, int departure) select,
    List<TransitLeg?> fetched,
  )
  build;

  @override
  State<_Chained> createState() => _ChainedState();
}

class _ChainedState extends State<_Chained> {
  late var _chain = ConnectionChain(widget.journey);

  @override
  void initState() {
    super.initState();
    widget.legs?.addListener(_arrived);
    _follow();
  }

  void _arrived() => setState(() {});

  void _follow() =>
      widget.legs?.follow(widget.plan!, _chain, TransitAccess.walking);

  @override
  void dispose() {
    widget.legs?.removeListener(_arrived);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.build(
    _chain,
    (ride, departure) {
      setState(() => _chain = _chain.select(ride, departure));
      _follow();
    },
    [
      for (final choice in _chain.rides)
        widget.legs?.legOf(choice, TransitAccess.walking),
    ],
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
      build: (chain, select, _) => DepartureRange(
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
      await tester.pumpAndSettle();
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

    testWidgets('choosing one keeps each tile, and eases the border over', (
      tester,
    ) async {
      await range(tester, 0);
      final before = tester.element(tile('5:52 PM'));
      final own = tester.element(tile('5:40 PM'));
      await tester.tap(tile('5:52 PM'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      // The same tiles, not rebuilt in each other's places.
      expect(tester.element(tile('5:52 PM')), same(before));
      expect(tester.element(tile('5:40 PM')), same(own));
      Color border(String time) {
        final box = tester.renderObject<RenderDecoratedBox>(
          find
              .descendant(of: tile(time), matching: find.byType(DecoratedBox))
              .first,
        );
        return ((box.decoration as BoxDecoration).border! as Border).top.color;
      }

      // Mid-way, the border is on its way from one to the other.
      expect(border('5:52 PM').a, inExclusiveRange(0, 1));
      expect(border('5:40 PM').a, inExclusiveRange(0, 1));
      await tester.pumpAndSettle();
      expect(border('5:52 PM').a, 1);
      expect(border('5:40 PM').a, 0);
    });

    testWidgets('the row opens on the chosen tile, from its first frame', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(160, 600));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await range(tester, 0);
      final row = tester.state<ScrollableState>(
        find.descendant(
          of: find.byType(DepartureRange),
          matching: find.byType(Scrollable),
        ),
      );
      final x = tester.getCenter(tile('5:40 PM')).dx;
      expect(row.position.pixels, greaterThan(0));
      expect(x, moreOrLessEquals(80, epsilon: 1));
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
          build: (chain, select, _) => DepartureRange(
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
      DepartureLegs? legs,
      TransitPlan? plan,
    }) => tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: _Chained(
            journey: journey,
            legs: legs,
            plan: plan,
            build: (chain, select, fetched) => CustomScrollView(
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
                  fetched: fetched,
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
      await tester.binding.setSurfaceSize(const Size(900, 2400));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await itinerary(tester, journey);
      expect(find.text('Stay on board'), findsOneWidget);
      expect(find.text('At 3rd Ave & Pike St it continues as'), findsOneWidget);
      expect(find.text('to Rainier Beach'), findsOneWidget);
      // Its row spans the list as the others do, so its rail lines up with theirs
      // however short its text: on a wide screen, its lines don't fill the row.
      Finder rowOf(Finder text) => find
          .ancestor(
            of: text,
            matching: find.byWidgetPredicate(
              (w) => w.runtimeType.toString() == '_RailRow',
            ),
          )
          .first;
      final seat = tester.getRect(rowOf(find.text('Stay on board')));
      final departures = tester.getRect(rowOf(find.text('Departures').first));
      expect(seat.left, departures.left);
      expect(seat.width, departures.width);
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
      expect(find.text('Change at Westlake Station'), findsOneWidget);
      expect(find.text(', wait 1 min'), findsOneWidget);
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

    /// Taps that tile, scrolled to, and lets the rows settle.
    Future<void> choose(WidgetTester tester, int leg, String time) async {
      await tester.ensureVisible(rideTile(leg, time));
      await tester.pumpAndSettle();
      await tester.tap(rideTile(leg, time));
      await tester.pumpAndSettle();
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
        // Back on the suggested one, they show again, still open.
        await choose(tester, 1, '5:40 PM');
        expect(find.text('1 stop before'), findsNWidgets(4));
        expect(find.text('Stop A to Stop B, between'), findsOneWidget);
      },
    );

    testWidgets('a choice moves nothing above the departures chosen in', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 800));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final transit = FakeTransit();
      final legs = DepartureLegs(transit);
      addTearDown(legs.dispose);
      final plan = TransitPlan.fromJson(connectionsPlanJson());
      await itinerary(
        tester,
        plan.journeys.single,
        header: true,
        legs: legs,
        plan: plan,
      );

      /// Taps [time] among leg [leg]'s departures, and checks that the tiles' row is
      /// exactly where it was, the moment it's tapped, mid-way and after.
      Future<void> stays(int leg, String time) async {
        final row = find.byKey(ValueKey(('departures', leg)));
        await tester.ensureVisible(rideTile(leg, time));
        await tester.pumpAndSettle();
        final at = tester.getTopLeft(row);
        await tester.tap(rideTile(leg, time));
        await tester.pump();
        expect(tester.getTopLeft(row), at);
        await tester.pump(const Duration(milliseconds: 120));
        expect(tester.getTopLeft(row), at);
        await tester.pumpAndSettle();
        expect(selected(tester, leg, time), isTrue);
        expect(tester.getTopLeft(row), at);
      }

      // A 7 that reaches Link just as it leaves: no wait for it, said in a row that
      // stays for when there is one.
      await choose(tester, 1, '5:43 PM');
      expect(find.text('No wait'), findsOneWidget);
      await stays(3, '6:03 PM');
      expect(find.text('Wait 8 min'), findsOneWidget);
      await stays(3, '5:55 PM');
      expect(find.text('No wait'), findsOneWidget);

      // Another route, headed elsewhere, with a longer wait.
      expect(find.text('Wait 2 min'), findsOneWidget);
      await stays(5, '6:15 PM');
      expect(find.text('Wait 9 min'), findsOneWidget);
      expect(find.text('Rainier Beach'), findsWidgets);
      expect(
        find.descendant(
          of: find.byKey(const ValueKey(('title', 5))).first,
          matching: find.text('9'),
        ),
        findsOneWidget,
      );

      /// Answers the question for [trip]'s leg, of leg [leg], with a leg of three stops
      /// between, and checks that neither the departures' row nor the ride's rows
      /// above it move or change height as its stops grow in below.
      Future<void> arrives(String trip, int leg) async {
        final row = find.byKey(ValueKey(('departures', leg)));
        Rect title() =>
            tester.getRect(find.byKey(ValueKey(('title', leg))).first);
        Rect boarding() =>
            tester.getRect(find.byKey(ValueKey(('boarding', leg))).first);
        await tester.ensureVisible(row);
        await tester.pumpAndSettle();
        final at = (tester.getTopLeft(row), title(), boarding());
        final question = transit.rides.singleWhere(
          (q) => q.query.tripId == trip && !q.aborted,
        );
        question.answer.complete(
          fakeRideLeg(
            trip,
            const [
              [0, 0],
              [0, 0],
            ],
            departure: question.query.departure.millisecondsSinceEpoch ~/ 1000,
            between: const ['One', 'Two', 'Three'],
          ),
        );
        await tester.pump();
        expect((tester.getTopLeft(row), title(), boarding()), at);
        await tester.pump(const Duration(milliseconds: 120));
        expect((tester.getTopLeft(row), title(), boarding()), at);
        await tester.pumpAndSettle();
        expect((tester.getTopLeft(row), title(), boarding()), at);
        expect(
          find.descendant(
            of: find.byKey(ValueKey(('stops', leg, 0))).first,
            matching: find.text('3 stops before'),
          ),
          findsOneWidget,
        );
      }

      // The 9's leg, then the 7's, above it.
      await arrives('9-1', 5);
      await arrives('7-1', 1);
    });

    testWidgets('rows below the departures chosen in grow and fade in', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 4000));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await itinerary(tester, connectionsJourney());
      // The 7 shows only its ends, so the 40's stops toggle goes; back on the 40 it
      // grows back in.
      final toggle = find.byKey(const ValueKey(('stops', 1, 0)));
      final full = tester.getSize(toggle.first).height;
      await choose(tester, 1, '5:43 PM');
      expect(toggle, findsNothing);
      await tester.ensureVisible(rideTile(1, '5:40 PM'));
      await tester.pumpAndSettle();
      await tester.tap(rideTile(1, '5:40 PM'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(tester.getSize(toggle.first).height, inExclusiveRange(0, full));
      await tester.pumpAndSettle();
      expect(tester.getSize(toggle.first).height, full);
    });

    group('another departure\'s leg', () {
      late FakeTransit transit;
      late DepartureLegs legs;
      late TransitPlan plan;

      setUp(() {
        transit = FakeTransit();
        legs = DepartureLegs(transit);
        plan = TransitPlan.fromJson(connectionsPlanJson());
      });

      tearDown(() => legs.dispose());

      Future<void> fetching(WidgetTester tester) async {
        await tester.binding.setSurfaceSize(const Size(400, 4000));
        addTearDown(() => tester.binding.setSurfaceSize(null));
        await itinerary(
          tester,
          plan.journeys.single,
          legs: legs,
          plan: plan,
          header: true,
        );
      }

      /// The question still asked for [trip]'s leg.
      RideQuestion asked(String trip) => transit.rides.singleWhere(
        (q) => q.query.tripId == trip && !q.aborted,
      );

      /// The 7 at 17:43 from Stop A to Stop B, as the source has it: a minute later
      /// than the plan does, by two stops.
      TransitLeg seven({List<Map<String, Object?>> segments = const []}) =>
          fakeRideLeg(
            '7-1',
            const [
              [0, 0],
              [0, 0],
            ],
            departure: fakeStart + 840,
            ride: 540,
            route: fakeRoutes['7'],
            from: 'Stop A',
            to: 'Stop B',
            between: const ['Fremont Bridge', 'Dexter Ave'],
            skipped: const {'Dexter Ave'},
            segments: segments,
          );

      testWidgets('shows its stops once it arrives, timed as chosen', (
        tester,
      ) async {
        await fetching(tester);
        await choose(tester, 1, '5:43 PM');
        expect(find.text('1 stop before'), findsNWidgets(3));
        expect(find.text('2 stops before'), findsNothing);

        asked('7-1').answer.complete(seven());
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 100));
        // Growing in.
        final toggle = find.byKey(const ValueKey(('stops', 1, 0))).first;
        final height = tester.getSize(toggle).height;
        await tester.pumpAndSettle();
        expect(height, lessThan(tester.getSize(toggle).height));
        expect(find.text('2 stops before'), findsOneWidget);
        // From when it leaves to when it arrives, as its tile has it.
        expect(
          find.descendant(of: toggle, matching: find.text('9 min')),
          findsOneWidget,
        );
        await tester.tap(find.text('2 stops before'));
        await tester.pumpAndSettle();
        expect(find.text('Fremont Bridge'), findsOneWidget);
        // Its stops' own times between, and the choice's at its ends.
        Finder beside(String stop, String time) => find.descendant(
          of: find.ancestor(of: find.text(stop), matching: find.byType(Row)),
          matching: find.text(time),
        );
        expect(beside('Fremont Bridge', '5:47 PM'), findsOneWidget);
        expect(beside('Dexter Ave', '5:50 PM'), findsOneWidget);
        expect(beside('Stop A', '5:43 PM'), findsOneWidget);
        expect(beside('Stop B', '5:52 PM'), findsOneWidget);
        final skipped = tester.widget<Text>(find.text('Dexter Ave'));
        expect(skipped.style?.decoration, TextDecoration.lineThrough);
      });

      testWidgets('a stay-seated one stays on board, in each trip\'s colors', (
        tester,
      ) async {
        await fetching(tester);
        await choose(tester, 1, '5:43 PM');
        expect(find.text('Stay on board'), findsNothing);
        asked('7-1').answer.complete(
          seven(
            segments: [
              {
                'trip_id': '7-1',
                'route': fakeRoutes['7'],
                'headsign': 'Rainier Beach',
                'first_stop_index': 0,
                'last_stop_index': 1,
              },
              {
                'trip_id': '9-7',
                'route': fakeRoutes['9'],
                'headsign': 'Othello',
                'first_stop_index': 1,
                'last_stop_index': 3,
              },
            ],
          ),
        );
        await tester.pumpAndSettle();
        expect(find.text('Stay on board'), findsOneWidget);
        expect(find.text('At Fremont Bridge it continues as'), findsOneWidget);
        expect(find.text('to Othello'), findsOneWidget);
        // Each trip's stops between: none on the 7's, one on the 9's.
        expect(find.byKey(const ValueKey(('stops', 1, 0))), findsNothing);
        expect(find.text('1 stop before'), findsNWidgets(4));
        final seat = tester.widget(
          find.byKey(const ValueKey(('seat', 1, 0))).last,
        );
        // ignore: avoid_dynamic_calls
        final rail = (seat as dynamic).rail;
        // ignore: avoid_dynamic_calls
        expect(rail.aboveColor, const Color(0xFF8E3A9C));
        // ignore: avoid_dynamic_calls
        expect(rail.belowColor, const Color(0xFFF15A22));
      });

      testWidgets('an open toggle stays open across choices, on its ride', (
        tester,
      ) async {
        await fetching(tester);
        await tester.tap(find.text('1 stop before').first);
        await tester.pumpAndSettle();
        expect(find.text('Stop A to Stop B, between'), findsOneWidget);

        await choose(tester, 1, '5:43 PM');
        asked('7-1').answer.complete(seven());
        await tester.pumpAndSettle();
        // Open, on the 7's stops.
        expect(find.text('Stop A to Stop B, between'), findsNothing);
        expect(find.text('Fremont Bridge'), findsOneWidget);
        // The other rides' stay closed.
        expect(find.text('Stop C to Stop D, between'), findsNothing);
        expect(find.text('Stop D Bay 2 to Stop E, between'), findsNothing);

        await choose(tester, 1, '5:40 PM');
        expect(find.text('Fremont Bridge'), findsNothing);
        expect(find.text('Stop A to Stop B, between'), findsOneWidget);
        expect(find.text('Stop C to Stop D, between'), findsNothing);
      });

      testWidgets('one no longer taken when it arrives is ignored', (
        tester,
      ) async {
        await fetching(tester);
        await choose(tester, 1, '5:43 PM');
        final first = asked('7-1');
        await choose(tester, 1, '5:51 PM');
        expect(first.aborted, isTrue);
        first.answer.complete(seven());
        await tester.pumpAndSettle();
        expect(find.text('2 stops before'), findsNothing);
        expect(find.byKey(const ValueKey(('stops', 1, 0))), findsNothing);
      });

      testWidgets('one that fails leaves the ride\'s ends', (tester) async {
        await fetching(tester);
        await choose(tester, 1, '5:43 PM');
        asked('7-1').answer.completeError(Exception('offline'));
        await tester.pumpAndSettle();
        expect(find.text('1 stop before'), findsNWidgets(3));
        expect(find.text('Stop A'), findsOneWidget);
        expect(find.text('Stop B'), findsOneWidget);
        expect(find.text('5:43 PM'), findsNWidgets(2));
        expect(find.text('5:52 PM'), findsOneWidget);
      });
    });

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
