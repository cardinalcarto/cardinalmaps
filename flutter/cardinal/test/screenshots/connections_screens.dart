// Screenshots of a transit journey's connections, for review: see `render.dart`. The
// overview's "or" badges, and a journey's departure tiles as planned, with a later first
// ride chosen, and with no connection.
import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/transit_details.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/transit_timeline.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_transit.dart';
import 'render.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

/// A phone's width, and room for all of a journey's details.
const _width = 392.0;

Future<void> _app(WidgetTester tester, Size size, Widget body) async {
  await tester.binding.setSurfaceSize(size);
  addTearDown(() => tester.binding.setSurfaceSize(null));
  await tester.pumpWidget(
    MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: appTheme(Brightness.light, platform: TargetPlatform.android),
      home: Scaffold(body: SafeArea(child: body)),
    ),
  );
}

/// [journey]'s details, choosing as the rider taps.
class _Details extends StatefulWidget {
  const _Details(this.journey);

  final Journey journey;

  @override
  State<_Details> createState() => _DetailsState();
}

class _DetailsState extends State<_Details> {
  late var _chain = ConnectionChain(widget.journey);

  @override
  Widget build(BuildContext context) => CustomScrollView(
    slivers: [
      SliverToBoxAdapter(
        child: JourneyHeader(
          chain: _chain,
          now: at(0),
          units: UnitSystem.imperial,
          onBack: () {},
        ),
      ),
      JourneyItinerary(
        chain: _chain,
        onSelect: (ride, departure) =>
            setState(() => _chain = _chain.select(ride, departure)),
        start: 'Home',
        destination: 'Chu Minh',
        units: UnitSystem.imperial,
        access: TransitAccess.walking,
        now: at(0),
      ),
    ],
  );
}

/// Taps the first ride's 5:55 PM.
Future<void> _laterFirstRide(WidgetTester tester) async {
  final tile = find.descendant(
    of: find.byKey(const ValueKey(('departures', 1))),
    matching: find.ancestor(
      of: find.text('5:55 PM'),
      matching: find.byType(ConnectionTile),
    ),
  );
  await tester.ensureVisible(tile);
  await tester.pumpAndSettle();
  await tester.tap(tile);
  await tester.pumpAndSettle();
  // Back to the top, for the whole of it.
  await tester.drag(find.byType(CustomScrollView), const Offset(0, 4000));
}

void main() {
  setUpAll(loadRenderFonts);
  setUp(
    () => FocusManager.instance.highlightStrategy =
        FocusHighlightStrategy.alwaysTouch,
  );

  testWidgets('overview', (tester) async {
    final journeys = [connectionsJourney(), ...fakePlan(journeys: 2).journeys];
    final axis = TimelineAxis.of(journeys);
    await _app(
      tester,
      const Size(_width, 420),
      Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
            child: TimelineTicks(axis: axis),
          ),
          for (final journey in journeys) ...[
            JourneyRow(
              journey: journey,
              axis: axis,
              now: at(-120),
              onTap: () {},
            ),
            const Divider(height: 1, indent: 16, endIndent: 16),
          ],
        ],
      ),
    );
    await screenshot(tester, 'connections-1-overview');
  });

  testWidgets('overview, crowded', (tester) async {
    // A 4-ride journey on an axis of two hours, as beside much later journeys.
    final journey = connectionsJourney();
    final axis = TimelineAxis(at(0), at(2 * 3600));
    await _app(
      tester,
      const Size(_width, 200),
      Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
            child: TimelineTicks(axis: axis),
          ),
          JourneyRow(journey: journey, axis: axis, now: at(-120), onTap: () {}),
        ],
      ),
    );
    await screenshot(tester, 'connections-1b-overview-crowded');
  });

  testWidgets('details, suggested', (tester) async {
    await _app(
      tester,
      const Size(_width, 2000),
      _Details(connectionsJourney()),
    );
    await screenshot(tester, 'connections-2-details-suggested');
  });

  testWidgets('details, a later first ride', (tester) async {
    await _app(
      tester,
      const Size(_width, 2000),
      _Details(connectionsJourney()),
    );
    await _laterFirstRide(tester);
    await screenshot(tester, 'connections-3-details-later');
  });

  testWidgets('details, no connection', (tester) async {
    await _app(
      tester,
      const Size(_width, 2000),
      _Details(connectionsJourney(lateLastRide: false)),
    );
    await _laterFirstRide(tester);
    await screenshot(tester, 'connections-4-details-no-connection');
  });
}
