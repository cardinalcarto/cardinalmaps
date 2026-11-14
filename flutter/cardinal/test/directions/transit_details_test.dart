import 'package:cardinal/directions/transit_details.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_transit.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

void main() {
  final journey = fakePlan().journeys.single;
  final bus = journey.rides.first;
  final link = journey.rides.last;

  Future<void> pump(WidgetTester tester, Widget child) => tester.pumpWidget(
    MaterialApp(
      home: Scaffold(body: SingleChildScrollView(child: child)),
    ),
  );

  Future<void> range(
    WidgetTester tester,
    TransitLeg ride, {
    TransitAccess access = TransitAccess.walking,
  }) => pump(tester, DepartureRange(ride: ride, access: access, now: at(0)));

  /// The chip for the departure at [time].
  Finder chip(String time) =>
      find.ancestor(of: find.text(time), matching: find.byType(DepartureChip));

  group('departure range', () {
    testWidgets('the journey\'s own is emphasized and described', (
      tester,
    ) async {
      await range(tester, bus);
      expect(find.byType(DepartureChip), findsNWidgets(6));
      final own = tester.widget<DepartureChip>(chip('5:40 PM'));
      expect(own.departure.suggested, isTrue);
      expect(own.chosen, isTrue);
      expect(find.text('Planned'), findsOneWidget);
      // Its color is the route's.
      final material = tester.widget<Material>(
        find.descendant(of: chip('5:40 PM'), matching: find.byType(Material)),
      );
      expect(material.color, const Color(0xffFDB71A));
      // How it's running, in one place.
      expect(find.text('Live: 4 min late'), findsOneWidget);
      expect(find.text('Wheelchair accessible'), findsOneWidget);
      expect(find.text('Route 40 detoured'), findsOneWidget);
    });

    testWidgets('departures the rider misses are muted and can\'t be chosen', (
      tester,
    ) async {
      await range(tester, bus);
      expect(find.text('Missed'), findsNWidgets(2));
      final missed = tester.widget<Material>(
        find.descendant(of: chip('5:30 PM'), matching: find.byType(Material)),
      );
      expect(missed.color, Colors.transparent);
      await tester.tap(chip('5:30 PM'));
      await tester.pump();
      // Still describing the journey's own.
      expect(find.text('Live: 4 min late'), findsOneWidget);
      expect(find.textContaining('Leaves'), findsNothing);
    });

    testWidgets('live and scheduled departures say which they are', (
      tester,
    ) async {
      await range(tester, bus);
      // A live departure has the live mark on its chip; a scheduled one doesn't.
      expect(
        find.descendant(
          of: chip('5:35 PM'),
          matching: find.byIcon(Icons.sensors),
        ),
        findsOneWidget,
      );
      expect(
        find.descendant(
          of: chip('5:52 PM'),
          matching: find.byIcon(Icons.sensors),
        ),
        findsNothing,
      );

      // A later one, chosen: when it leaves and arrives, and that it's scheduled, not
      // accessible, and has an alert the journey's own doesn't.
      expect(find.text('+12 min'), findsOneWidget);
      await tester.tap(chip('5:52 PM'));
      await tester.pump();
      expect(find.text('Leaves 5:52 PM, arrives 6:12 PM'), findsOneWidget);
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
      await range(tester, bus);
      expect(find.text('Canceled'), findsOneWidget);
      final time = tester.widget<Text>(find.text('6:00 PM'));
      expect(time.style?.decoration, TextDecoration.lineThrough);
      await tester.tap(chip('6:00 PM'));
      await tester.pump();
      expect(find.text('Live: 4 min late'), findsOneWidget);
    });

    testWidgets('the day\'s last is marked, and says so when chosen', (
      tester,
    ) async {
      await range(tester, bus);
      expect(find.text('Last'), findsOneWidget);
      await tester.tap(chip('6:04 PM'));
      await tester.pump();
      expect(
        find.text('The last 40 of the day from this stop'),
        findsOneWidget,
      );
    });

    testWidgets('a connection that is the night\'s last says so at once', (
      tester,
    ) async {
      await range(tester, link);
      // Its own chip is the last; the earlier one is missed.
      expect(find.text('Last'), findsOneWidget);
      expect(find.text('Missed'), findsOneWidget);
      expect(
        find.text('The last 1 Line of the day from this stop'),
        findsOneWidget,
      );
      expect(find.text('Scheduled'), findsOneWidget);
      expect(find.text('Wheelchair access unknown'), findsOneWidget);
    });
  });

  group('itinerary', () {
    Future<void> itinerary(
      WidgetTester tester,
      Journey journey, {
      TransitAccess access = TransitAccess.walking,
    }) => tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: CustomScrollView(
            slivers: [
              JourneyItinerary(
                journey: journey,
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
  });
}
