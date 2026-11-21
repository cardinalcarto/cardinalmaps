import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/departure_legs.dart';
import 'package:cardinal/directions/transit_details.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/transit_sheet.dart' show liveColor;
import 'package:cardinal/transit/nearby_page.dart' show CountdownNumber;
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
    this.now,
    this.newer,
  });

  final Journey journey;

  /// A newer plan's journeys, none of them [journey], whose departures the chain
  /// takes up ([ConnectionChain.refresh]) when they're given or change.
  final List<Journey>? newer;

  /// The time the first ride's departures are made as, if they're made by the clock.
  final DateTime? now;
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
  late var _chain = ConnectionChain(widget.journey, now: widget.now);

  @override
  void initState() {
    super.initState();
    widget.legs?.addListener(_arrived);
    _follow();
  }

  @override
  void didUpdateWidget(_Chained oldWidget) {
    super.didUpdateWidget(oldWidget);
    // The clock moved on: the choices as it has them.
    final now = widget.now;
    if (now != null && now != oldWidget.now) _chain = _chain.at(now);
    final newer = widget.newer;
    if (newer != null && !identical(newer, oldWidget.newer)) {
      _chain = _chain.refresh(newer);
    }
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
      expect(find.text('Live · 4 min late'), findsOneWidget);
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
      expect(find.text('Live · 4 min late'), findsOneWidget);
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
      BorderSide border(String time) {
        // The border is drawn over the tile.
        final box = tester.renderObject<RenderDecoratedBox>(
          find
              .descendant(of: tile(time), matching: find.byType(DecoratedBox))
              .first,
        );
        expect(box.position, DecorationPosition.foreground);
        return ((box.decoration as BoxDecoration).border! as Border).top;
      }

      final scheme = Theme.of(tester.element(tile('5:40 PM'))).colorScheme;
      // Mid-way, the border is on its way from one to the other: from the outline to
      // the primary color's thicker one, and back.
      expect(border('5:52 PM').width, inExclusiveRange(1, 2));
      expect(border('5:40 PM').width, inExclusiveRange(1, 2));
      await tester.pumpAndSettle();
      expect(border('5:52 PM'), BorderSide(color: scheme.primary, width: 2));
      expect(border('5:40 PM'), BorderSide(color: scheme.outline));
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
      expect(find.text('Live · 4 min late'), findsOneWidget);
    });

    testWidgets('the day\'s last is marked, and says so when chosen', (
      tester,
    ) async {
      await range(tester, 0);
      expect(find.text('Last'), findsOneWidget);
      await tester.ensureVisible(tile('6:04 PM'));
      await tester.pumpAndSettle();
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
      DateTime? now,
      List<Journey>? newer,
    }) => tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: _Chained(
            journey: journey,
            legs: legs,
            plan: plan,
            now: now,
            newer: newer,
            build: (chain, select, fetched) => CustomScrollView(
              slivers: [
                if (header)
                  SliverToBoxAdapter(
                    child: JourneyHeader(
                      chain: chain,
                      now: at(0),
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
      final departures = tester.getRect(
        find.byKey(const ValueKey(('departuresRow', 1))).last,
      );
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
      // How many, and how long they take.
      await tester.tap(find.text('1 stop · 20 min'));
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

    group('a wait row with nothing to wait for', () {
      /// The wait rows, by the index of the ride's leg.
      Finder waitRow(int leg) => find.byKey(ValueKey(('wait', leg))).last;

      /// The row of the departures of the ride that's leg [leg].
      Finder departuresRow(int leg) =>
          find.byKey(ValueKey(('departures', leg))).last;

      /// Every text in [row] that says nothing is kept from screen readers.
      void saysNothingAloud(Finder row) {
        final empty = find.descendant(
          of: row,
          matching: find.byWidgetPredicate((w) => w is Text && w.data == ''),
        );
        for (final text in empty.evaluate()) {
          expect(
            find.ancestor(
              of: find.byWidget(text.widget),
              matching: find.byWidgetPredicate(
                (w) => w is ExcludeSemantics && w.excluding,
              ),
            ),
            findsWidgets,
          );
        }
      }

      for (final scale in [1.0, 2.5]) {
        testWidgets('at ${scale}x, goes blank as the first ride\'s departures '
            'go, cross-fading, the same height, and never comes or goes', (
          tester,
        ) async {
          tester.platformDispatcher.textScaleFactorTestValue = scale;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          await tester.binding.setSurfaceSize(const Size(400, 4000));
          addTearDown(() => tester.binding.setSurfaceSize(null));
          await itinerary(tester, journey, now: at(0));
          expect(find.text('Wait 5 min'), findsOneWidget);
          expect(find.text('Wait 1 min'), findsOneWidget);
          final rows = {
            for (final leg in [1, 3]) leg: tester.getRect(waitRow(leg)),
          };
          final tiles = tester.getRect(departuresRow(1));
          final stops = {
            for (final key in [('boarding', 1), ('alighting', 1)])
              key: tester.getSize(find.byKey(ValueKey(key)).last),
          };

          // A departure missed by the clock: the rows stay as they are.
          await itinerary(tester, journey, now: at(301));
          await tester.pumpAndSettle();
          expect(waitRow(1), findsOneWidget);
          expect(waitRow(3), findsOneWidget);

          // Every departure gone: the first ride has no connection, and Link's is
          // unknown. Each row fades to nothing, where it was.
          await itinerary(tester, journey, now: at(1741));
          await tester.pump();
          await tester.pump(const Duration(milliseconds: 80));
          final fading = tester
              .widgetList<FadeTransition>(
                find.ancestor(
                  of: find.text('Wait 5 min'),
                  matching: find.byType(FadeTransition),
                ),
              )
              .fold(1.0, (shown, fade) => shown * fade.opacity.value);
          expect(fading, inExclusiveRange(0, 1));
          await tester.pumpAndSettle();
          for (final leg in [1, 3]) {
            // Where the rows below the first ride's tiles are moves with what's
            // said under them, but none is shorter or taller.
            expect(
              tester.getSize(waitRow(leg)),
              rows[leg]!.size,
              reason: 'leg $leg',
            );
            saysNothingAloud(waitRow(leg));
          }
          expect(tester.getRect(waitRow(1)), rows[1]);
          // The first ride's stops, with no times, are as tall as with them, so its
          // tiles are where they were.
          for (final key in stops.keys) {
            expect(
              tester.getSize(find.byKey(ValueKey(key)).last),
              stops[key],
              reason: '$key',
            );
          }
          expect(tester.getRect(departuresRow(1)).topLeft, tiles.topLeft);
          expect(find.textContaining(RegExp(r'^Wait ')), findsNothing);
          expect(find.text('No wait'), findsNothing);
          expect(find.byIcon(Icons.hourglass_empty), findsNothing);
          expect(tester.takeException(), isNull);
        });
      }

      testWidgets('a change still says where, but not the wait', (
        tester,
      ) async {
        await tester.binding.setSurfaceSize(const Size(400, 2400));
        addTearDown(() => tester.binding.setSurfaceSize(null));
        final json = fakePlanJson();
        final legs =
            ((json['journeys']! as List).single as Map)['legs']! as List;
        legs.removeAt(2);
        final linkStops = (legs[2] as Map)['stops']! as List;
        (linkStops.first as Map)['name'] = 'Westlake Station';
        final changing = TransitPlan.fromJson(json).journeys.single;
        await itinerary(tester, changing, now: at(0));
        final row = tester.getRect(waitRow(2));
        expect(find.text(', wait 1 min'), findsOneWidget);
        await itinerary(tester, changing, now: at(1741));
        await tester.pumpAndSettle();
        expect(find.text('Change at Westlake Station'), findsOneWidget);
        expect(find.byIcon(Icons.sync_alt), findsOneWidget);
        expect(find.text(', wait 1 min'), findsNothing);
        expect(tester.getSize(waitRow(2)), row.size);
        saysNothingAloud(waitRow(2));
      });

      for (final scale in [1.0, 2.5]) {
        testWidgets(
          'at ${scale}x, a ride whose departures can\'t be made keeps '
          'its row, blank, and the rides after theirs, its tiles where they '
          'were',
          (tester) async {
            tester.platformDispatcher.textScaleFactorTestValue = scale;
            addTearDown(
              tester.platformDispatcher.clearTextScaleFactorTestValue,
            );
            await tester.binding.setSurfaceSize(const Size(400, 8000));
            addTearDown(() => tester.binding.setSurfaceSize(null));
            // The 31 at 18:00 reaches Stop B too late for any of Link's.
            await itinerary(tester, connectionsJourney());
            final rows = {
              for (final leg in [1, 3, 5, 7]) leg: tester.getRect(waitRow(leg)),
            };
            final boarding = tester.getSize(
              find.byKey(const ValueKey(('boarding', 3))).last,
            );
            final tilesBelowWait =
                tester.getRect(departuresRow(3)).top -
                tester.getRect(waitRow(3)).top;
            expect(find.text('No wait'), findsNothing);
            final later = find.descendant(
              of: find.byKey(const ValueKey(('departures', 1))),
              matching: tile('6:00 PM'),
            );
            await tester.ensureVisible(later);
            await tester.pumpAndSettle();
            await tester.tap(later);
            await tester.pumpAndSettle();
            expect(find.byType(NoConnection), findsOneWidget);
            // Link's boarding stop, with no time, is as tall as with one, and its tiles
            // stay where they were, under rows that didn't change height.
            expect(
              tester.getSize(find.byKey(const ValueKey(('boarding', 3))).last),
              boarding,
            );
            expect(
              tester.getRect(departuresRow(3)).top -
                  tester.getRect(waitRow(3)).top,
              tilesBelowWait,
            );
            for (final leg in [1, 3, 5, 7]) {
              expect(waitRow(leg), findsOneWidget, reason: 'leg $leg');
              expect(
                tester.getSize(waitRow(leg)),
                rows[leg]!.size,
                reason: 'leg $leg',
              );
            }
            // The 2's row: nothing to say.
            expect(
              find.descendant(of: waitRow(7), matching: find.byType(Icon)),
              findsNothing,
            );
          },
        );
      }
    });

    group('at large text, a ride\'s title', () {
      const long =
          'Rainier Beach by way of Martin Luther King Jr Way South and '
          'Henderson Street';

      /// [connectionsPlanJson]'s journey, its 7 at 17:51 headed for [long] if
      /// [longer].
      Journey journeyWith({required bool longer}) {
        final json = connectionsPlanJson();
        final legs =
            ((json['journeys']! as List).single as Map)['legs'] as List;
        for (final d
            in ((legs[1] as Map)['departures'] as List)
                .cast<Map<String, Object?>>()) {
          if (longer && d['trip_id'] == '7-2') d['headsign'] = long;
        }
        return TransitPlan.fromJson(json).journeys.single;
      }

      Future<void> large(WidgetTester tester) async {
        tester.platformDispatcher.textScaleFactorTestValue = 2.5;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await tester.binding.setSurfaceSize(const Size(400, 8000));
        addTearDown(() => tester.binding.setSurfaceSize(null));
      }

      Rect title(WidgetTester tester) =>
          tester.getRect(find.byKey(const ValueKey(('title', 1))).last);
      Rect tiles(WidgetTester tester) =>
          tester.getRect(find.byKey(const ValueKey(('departures', 1))).last);

      testWidgets('keeps the room of a headsign a newer list drops', (
        tester,
      ) async {
        await large(tester);
        final journey = journeyWith(longer: true);
        await itinerary(tester, journey);
        final (height, top) = (title(tester).height, tiles(tester).top);
        await itinerary(tester, journey, newer: [journeyWith(longer: false)]);
        await tester.pumpAndSettle();
        // The list has it no longer, but its room is kept.
        expect(
          tester
              .widget<ConnectionTile>(find.byType(ConnectionTile).at(4))
              .departure
              .headsign,
          isNot(long),
        );
        expect(title(tester).height, height);
        expect(tiles(tester).top, top);
      });

      testWidgets('makes room for a longer one a newer list brings, as it\'s '
          'shown, so choosing it moves nothing', (tester) async {
        await large(tester);
        final journey = journeyWith(longer: false);
        await itinerary(tester, journey);
        final height = title(tester).height;
        await itinerary(tester, journey, newer: [journeyWith(longer: true)]);
        await tester.pumpAndSettle();
        expect(title(tester).height, greaterThan(height));
        final top = tiles(tester).top;
        final grown = title(tester).height;
        final seven = find.descendant(
          of: find.byKey(const ValueKey(('departures', 1))),
          matching: tile('5:51 PM'),
        );
        await tester.ensureVisible(seven);
        await tester.pumpAndSettle();
        final at = tester.getTopLeft(seven);
        await tester.tap(seven);
        await tester.pumpAndSettle();
        expect(find.text(long), findsOneWidget);
        expect(title(tester).height, grown);
        // The tiles may scroll sideways to center it, but nothing moves them down.
        expect(tester.getTopLeft(seven).dy, at.dy);
        expect(tiles(tester).top, top);
      });
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

    group('every departure of the first ride gone', () {
      Future<void> gone(
        WidgetTester tester,
        Journey journey,
        int seconds,
      ) async {
        await tester.binding.setSurfaceSize(const Size(400, 2400));
        addTearDown(() => tester.binding.setSurfaceSize(null));
        await itinerary(tester, journey, header: true, now: at(seconds));
      }

      void saysNoTimes(WidgetTester tester) {
        expect(find.textContaining('Go in'), findsNothing);
        expect(find.textContaining('Go now'), findsNothing);
        // But for the room kept for the widest times, at 10:00.
        expect(
          find.textContaining(RegExp(r'^(Leave|Arrive) (?!10:00)')),
          findsNothing,
        );
        expect(find.text('No connection'), findsNothing);
        // The first ride's tiles are each missed, or don't run.
        final tiles = tester.widgetList<ConnectionTile>(
          find.descendant(
            of: find.byKey(const ValueKey(('departures', 1))),
            matching: find.byType(ConnectionTile),
          ),
        );
        expect(tiles, isNotEmpty);
        expect(tiles.map((t) => t.catchable), everyElement(isFalse));
        expect(tiles.map((t) => t.selected), everyElement(isFalse));
        // The rides after have no departures, and no times.
        expect(find.byKey(const ValueKey(('departuresRow', 3))), findsNothing);
        expect(tester.takeException(), isNull);
      }

      testWidgets('after the day\'s last, says there are no more today', (
        tester,
      ) async {
        await gone(tester, journey, 1741);
        expect(find.text('No departures left'), findsOneWidget);
        expect(
          find.textContaining('Last left 6:04 PM · ', findRichText: true),
          findsOneWidget,
        );
        expect(find.text('No more departures today'), findsOneWidget);
        expect(
          find.text(
            'The last one between these stops left at 6:04 PM. Go back to '
            'see other trips.',
          ),
          findsOneWidget,
        );
        expect(find.text('Missed'), findsNWidgets(5));
        expect(find.text('Canceled'), findsOneWidget);
        saysNoTimes(tester);
      });

      testWidgets('with no more listed, says none are left', (tester) async {
        final json = fakePlanJson();
        final legs =
            ((json['journeys']! as List).single as Map)['legs']! as List;
        // The last listed isn't the day's last.
        (((legs[1] as Map)['departures']! as List).last as Map).remove(
          'last_of_day',
        );
        await gone(tester, TransitPlan.fromJson(json).journeys.single, 1741);
        expect(find.text('No departures left'), findsNWidgets(2));
        expect(
          find.textContaining('Last left 6:04 PM · ', findRichText: true),
          findsOneWidget,
        );
        expect(
          find.text(
            "You can't make any of these departures. Go back to see other "
            'trips.',
          ),
          findsOneWidget,
        );
        expect(find.text('No more departures today'), findsNothing);
        saysNoTimes(tester);
      });

      testWidgets('until then, a departure the clock passes is missed', (
        tester,
      ) async {
        await gone(tester, journey, 301);
        expect(
          tester.widget<ConnectionTile>(tile('5:40 PM')).catchable,
          isFalse,
        );
        expect(tester.widget<ConnectionTile>(tile('5:52 PM')).selected, isTrue);
        expect(find.text('No departures left'), findsNothing);
        expect(find.text('Leave 5:42 PM'), findsOneWidget);
      });
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
      expect(find.textContaining('5:30 PM – 6:32 PM · '), findsOneWidget);
      expect(find.text('Leave 5:30 PM'), findsOneWidget);
      expect(find.text('Arrive 6:32 PM'), findsOneWidget);
      expect(selected(tester, 3, '5:55 PM'), isTrue);

      await choose(tester, 1, '5:55 PM');
      // Leaving as much later, so there's as long to get to the stop.
      expect(find.textContaining('5:45 PM – 6:47 PM · '), findsOneWidget);
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
        expect(find.textContaining('1 stop · '), findsNWidgets(4));
        await tester.tap(find.textContaining('1 stop · ').first);
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
        expect(find.textContaining('1 stop · '), findsNWidgets(3));
        // Back on the suggested one, they show again, still open.
        await choose(tester, 1, '5:40 PM');
        expect(find.textContaining('1 stop · '), findsNWidgets(4));
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
        Rect title(tester) =>
            tester.getRect(find.byKey(ValueKey(('title', leg))).first);
        Rect boarding() =>
            tester.getRect(find.byKey(ValueKey(('boarding', leg))).first);
        await tester.ensureVisible(row);
        await tester.pumpAndSettle();
        final at = (tester.getTopLeft(row), title(tester), boarding());
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
        expect((tester.getTopLeft(row), title(tester), boarding()), at);
        await tester.pump(const Duration(milliseconds: 120));
        expect((tester.getTopLeft(row), title(tester), boarding()), at);
        await tester.pumpAndSettle();
        expect((tester.getTopLeft(row), title(tester), boarding()), at);
        expect(
          find.descendant(
            of: find.byKey(ValueKey(('stops', leg, 0))).first,
            matching: find.textContaining('3 stops · '),
          ),
          findsOneWidget,
        );
      }

      // The 9's leg, then the 7's, above it.
      await arrives('9-1', 5);
      await arrives('7-1', 1);
    });

    testWidgets('a tile missed with a choice upstream fades, its row as tall', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(400, 4000));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await itinerary(tester, connectionsJourney());
      // Link's 18:03, counting down, with no label.
      final link = find.byKey(const ValueKey(('departures', 3)));
      Finder row() =>
          find.descendant(of: link, matching: find.byType(IntrinsicHeight));
      Finder inTile(Finder f) =>
          find.descendant(of: rideTile(3, '6:03 PM'), matching: f);
      final height = tester.getSize(row()).height;
      expect(inTile(find.text('33')), findsOneWidget);
      expect(inTile(find.text('Missed')), findsNothing);

      /// How far down the tile [f] is.
      double down(Finder f) =>
          tester.getTopLeft(f).dy -
          tester.getTopLeft(rideTile(3, '6:03 PM')).dy;
      final labelTop = down(inTile(find.byKey(const ValueKey('label'))).last);

      await tester.ensureVisible(rideTile(1, '5:55 PM'));
      await tester.pumpAndSettle();
      await tester.tap(rideTile(1, '5:55 PM'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      // Mid-way, the label fades in where the line kept for it is, the countdown
      // fades to the dimmed time, and the row's as tall as it was.
      double opacity(Finder f) => tester
          .widget<FadeTransition>(
            find.ancestor(of: f, matching: find.byType(FadeTransition)).first,
          )
          .opacity
          .value;
      final missed = inTile(find.text('Missed'));
      expect(opacity(missed), inExclusiveRange(0, 1));
      final countdown = inTile(find.byType(CountdownNumber));
      expect(opacity(countdown), inExclusiveRange(0, 1));
      expect(down(missed), labelTop);
      expect(tester.getSize(row()).height, height);
      await tester.pumpAndSettle();
      expect(opacity(missed), 1);
      expect(countdown, findsNothing);
      expect(down(missed), labelTop);
      expect(tester.getSize(row()).height, height);
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
        expect(find.textContaining('1 stop · '), findsNWidgets(3));
        expect(find.textContaining('2 stops · '), findsNothing);

        asked('7-1').answer.complete(seven());
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 100));
        // Growing in.
        final toggle = find.byKey(const ValueKey(('stops', 1, 0))).first;
        final height = tester.getSize(toggle).height;
        await tester.pumpAndSettle();
        expect(height, lessThan(tester.getSize(toggle).height));
        expect(find.textContaining('2 stops · '), findsOneWidget);
        // From when it leaves to when it arrives, as its tile has it.
        expect(
          find.descendant(of: toggle, matching: find.text('2 stops · 9 min')),
          findsOneWidget,
        );
        await tester.tap(find.textContaining('2 stops · '));
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
        expect(find.textContaining('1 stop · '), findsNWidgets(4));
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
        await tester.tap(find.textContaining('1 stop · ').first);
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
        expect(find.textContaining('2 stops · '), findsNothing);
        expect(find.byKey(const ValueKey(('stops', 1, 0))), findsNothing);
      });

      testWidgets('one that fails leaves the ride\'s ends', (tester) async {
        await fetching(tester);
        await choose(tester, 1, '5:43 PM');
        asked('7-1').answer.completeError(Exception('offline'));
        await tester.pumpAndSettle();
        expect(find.textContaining('1 stop · '), findsNWidgets(3));
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
      expect(find.textContaining('5:45 PM – no connection · '), findsOneWidget);
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

    group('redesigned', () {
      /// The fake plan's JSON, with its street legs by [mode] and its rides changed by
      /// [change], each ride's leg JSON by its index among the legs.
      TransitPlan planWith({
        String mode = 'walking',
        void Function(List<Map<String, Object?>> legs)? change,
      }) {
        final json = fakePlanJson();
        final legs =
            (((json['journeys']! as List).single as Map)['legs']! as List)
                .cast<Map<String, Object?>>();
        for (final leg in legs) {
          if (leg['type'] != 'transit') leg['mode'] = mode;
        }
        change?.call(legs);
        return TransitPlan.fromJson(json);
      }

      for (final (mode, access, words) in [
        ('walking', TransitAccess.walking, '10 min walking'),
        ('cycling', TransitAccess.cycling, '10 min cycling'),
        ('wheelchair', TransitAccess.wheelchair, '10 min by wheelchair'),
      ]) {
        testWidgets('the header says the time on the streets $words', (
          tester,
        ) async {
          await tester.binding.setSurfaceSize(const Size(400, 2400));
          addTearDown(() => tester.binding.setSurfaceSize(null));
          await itinerary(
            tester,
            planWith(mode: mode).journeys.single,
            access: access,
            header: true,
          );
          expect(
            find.text('5:30 PM – 6:15 PM · 1 transfer · $words'),
            findsOneWidget,
          );
          // No title, and no distance.
          expect(find.text('Trip details'), findsNothing);
          expect(find.textContaining(' mi '), findsNothing);
        });
      }

      testWidgets('a live stop time off its schedule shows the schedule too', (
        tester,
      ) async {
        await tester.binding.setSurfaceSize(const Size(400, 2400));
        addTearDown(() => tester.binding.setSurfaceSize(null));
        await itinerary(tester, fakePlan().journeys.single);
        final boarding = find.byKey(const ValueKey(('boarding', 1))).last;
        // Scheduled at 5:36, leaving at 5:40: the schedule small and struck through,
        // before the live time in the live color.
        final scheduled = tester.widget<Text>(
          find.descendant(of: boarding, matching: find.text('5:36')),
        );
        expect(scheduled.style?.decoration, TextDecoration.lineThrough);
        final live = tester.widget<Text>(
          find.descendant(of: boarding, matching: find.text('5:40 PM')),
        );
        expect(
          live.style?.color,
          liveColor(Theme.of(tester.element(boarding))),
        );
        expect(
          tester.getTopLeft(find.text('5:36')).dx,
          lessThan(tester.getTopLeft(find.text('5:40 PM').first).dx),
        );
        // The time ends at the row's end.
        expect(
          tester
              .getTopRight(
                find.descendant(of: boarding, matching: find.text('5:40 PM')),
              )
              .dx,
          moreOrLessEquals(400 - 16, epsilon: 0.5),
        );
        // A scheduled stop time shows alone.
        final link = find.byKey(const ValueKey(('boarding', 3))).last;
        expect(
          find.descendant(
            of: link,
            matching: find.byWidgetPredicate(
              (w) =>
                  w is Text &&
                  w.style?.decoration == TextDecoration.lineThrough &&
                  (w.data ?? '').isNotEmpty,
            ),
          ),
          findsNothing,
        );
      });

      testWidgets('a tile says how late or early its departure runs', (
        tester,
      ) async {
        final plan = planWith(
          change: (legs) {
            final departures = (legs[1]['departures']! as List)
                .cast<Map<String, Object?>>();
            // The 40c, a minute early.
            departures[3]
              ..['status'] = 'live'
              ..['realtime'] = true
              ..['delay_seconds'] = -60;
          },
        );
        await range(tester, 0, of: plan.journeys.single);
        Finder inTile(String time, String text) =>
            find.descendant(of: tile(time), matching: find.text(text));
        final late = tester.widget<Text>(inTile('5:40 PM', ' · 4 min late'));
        final scheme = Theme.of(tester.element(tile('5:40 PM'))).colorScheme;
        expect(late.style?.color, scheme.error);
        final early = tester.widget<Text>(inTile('5:52 PM', ' · 1 min early'));
        expect(
          early.style?.color,
          liveColor(Theme.of(tester.element(tile('5:52 PM')))),
        );
        // One the rider misses says nothing of it, though it ran late.
        expect(
          find.descendant(
            of: tile('5:35 PM'),
            matching: find.textContaining('late'),
          ),
          findsNothing,
        );
        // No caption over the tiles.
        expect(find.text('Departures'), findsNothing);
      });

      testWidgets('chips say how it runs, never its vehicle\'s number', (
        tester,
      ) async {
        final plan = planWith(
          mode: 'cycling',
          change: (legs) {
            legs[1]['vehicle'] = {
              'id': '4321',
              'label': '4321',
              'occupancy': 'few_seats_available',
            };
            (legs[1]['trip']! as Map)['bikes_allowed'] = 'yes';
          },
        );
        await range(tester, 0, of: plan.journeys.single);
        expect(find.text('Live · 4 min late'), findsOneWidget);
        expect(find.text('Wheelchair accessible'), findsOneWidget);
        expect(find.text('Few seats'), findsOneWidget);
        expect(find.textContaining('4321'), findsNothing);
        expect(find.textContaining('Vehicle'), findsNothing);
        // Bikes only for a rider cycling to stops.
        expect(find.text('Bikes allowed'), findsNothing);
        await range(
          tester,
          0,
          of: plan.journeys.single,
          access: TransitAccess.cycling,
        );
        await tester.pumpAndSettle();
        expect(find.text('Bikes allowed'), findsOneWidget);
        // The alert is a row of its own, tall enough to tap.
        final alert = find.ancestor(
          of: find.text('Route 40 detoured'),
          matching: find.byType(ConstrainedBox),
        );
        expect(tester.getSize(alert.first).height, greaterThanOrEqualTo(44));
        // Another departure: scheduled, and its occupancy isn't known.
        await tester.ensureVisible(tile('5:52 PM'));
        await tester.pumpAndSettle();
        await tester.tap(tile('5:52 PM'));
        await tester.pumpAndSettle();
        expect(find.text('Scheduled'), findsOneWidget);
        expect(find.text('Few seats'), findsNothing);
        expect(find.text('Not wheelchair accessible'), findsOneWidget);
      });

      testWidgets('a tile tapped changes the height of nothing above it', (
        tester,
      ) async {
        await tester.binding.setSurfaceSize(const Size(320, 2400));
        addTearDown(() => tester.binding.setSurfaceSize(null));
        await itinerary(tester, fakePlan().journeys.single, header: true);
        Rect rect(Finder f) => tester.getRect(f.last);
        final above = [
          find.byType(JourneyHeader),
          find.byKey(const ValueKey(('wait', 1))),
          find.byKey(const ValueKey(('title', 1))),
          find.byKey(const ValueKey(('boarding', 1))),
        ];
        // The tiles stay put; what's under them, the chips, may change.
        final tiles = find.byKey(const ValueKey(('departures', 1)));
        Rect state() =>
            tester.getTopLeft(tiles) &
            tester.getSize(find.byType(JourneyHeader));
        final at = [for (final f in above) rect(f)];
        // From the late live one, whose stop shows its schedule, to a scheduled one.
        final later = rideTile(1, '5:52 PM');
        await tester.ensureVisible(later);
        await tester.pumpAndSettle();
        final scrolled = [for (final f in above) rect(f)];
        final top = state();
        await tester.tap(later);
        await tester.pump();
        expect([for (final f in above) rect(f)], scrolled);
        expect(state(), top);
        await tester.pump(const Duration(milliseconds: 100));
        expect([for (final f in above) rect(f)], scrolled);
        expect(state(), top);
        await tester.pumpAndSettle();
        expect([for (final f in above) rect(f)], scrolled);
        expect(state(), top);
        expect(selected(tester, 1, '5:52 PM'), isTrue);
        // Every height as it was before scrolling to it.
        expect(
          [for (final r in scrolled) r.height],
          [for (final r in at) r.height],
        );
      });
    });
  });
}
