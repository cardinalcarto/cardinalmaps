import 'dart:async';

import 'package:cardinal/directions/directions_page.dart';
import 'package:cardinal/directions/mode_tabs.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/trip_buttons.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_location.dart';
import '../fakes/fake_map.dart';
import '../fakes/fake_navigation.dart';
import '../fakes/fake_routes.dart';
import '../fakes/fake_transit.dart';

/// Where the trips of these tests go: made-up coordinates.
final destination = Place(
  id: 'poi:1',
  kind: 'poi',
  name: 'Museum of Flight',
  center: const LngLat(1, 4),
);

const here = Fix(lng: 0.5, lat: 2, accuracy: 3);

const _line = [
  [0.5, 2.0],
  [1.0, 4.0],
];

/// Four ways to drive there, the fastest first: by freeway; by a tolled freeway;
/// by streets, the shortest; and by a ferry, with no roads to name it for.
final viaFreeway = fakeRoute(
  _line,
  distance: 22850,
  duration: 1320,
  roadNames: const ['I-5 South'],
  motorway: true,
  runs: [fakeRun(3, 8), fakeRun(15, 27, motorway: true), fakeRun(4, 12)],
  steps: const [
    DirectionsStep(instruction: 'Drive south.', distance: 640, duration: 90),
    DirectionsStep(instruction: 'Take I-5.', distance: 22210, duration: 1230),
  ],
);
final viaToll = fakeRoute(
  _line,
  distance: 22200,
  duration: 1500,
  roadNames: const ['SR 99', 'SR 509'],
  toll: true,
  motorway: true,
  runs: [
    fakeRun(5, 10),
    fakeRun(10, 24, motorway: true, toll: true),
    fakeRun(10, 11),
  ],
  steps: const [
    DirectionsStep(instruction: 'Take SR 99.', distance: 22200, duration: 1500),
  ],
);
final viaStreets = fakeRoute(
  _line,
  distance: 21080,
  duration: 2040,
  roadNames: const ['East Marginal Way South'],
  runs: [fakeRun(34, 10.3)],
  steps: const [
    DirectionsStep(
      instruction:
          'Keep left at the fork to stay on East Marginal Way South, then '
          'continue past the freight yards toward the museum entrance.',
      distance: 21080,
      duration: 2040,
    ),
    DirectionsStep(
      instruction: 'Arrive at Museum of Flight.',
      distance: 0,
      duration: 0,
    ),
  ],
);
final viaFerry = fakeRoute(_line, distance: 30000, duration: 2700, ferry: true);
final drivingRoutes = [viaFreeway, viaToll, viaStreets, viaFerry];

final walkingRoutes = [
  fakeRoute(
    _line,
    distance: 20100,
    duration: 16800,
    roadNames: const ['Airport Way South'],
  ),
  fakeRoute(
    _line,
    distance: 20900,
    duration: 17400,
    roadNames: const ['4th Avenue South', 'East Marginal Way South'],
  ),
];

final cyclingRoutes = [
  fakeRoute(
    _line,
    distance: 25100,
    duration: 4680,
    roadNames: const ['Elliott Bay Trail', 'East Marginal Way South'],
  ),
  fakeRoute(
    _line,
    distance: 26200,
    duration: 5040,
    roadNames: const ['Green River Trail'],
  ),
  fakeRoute(
    _line,
    distance: 24300,
    duration: 5460,
    roadNames: const ['Beacon Avenue South'],
  ),
];

/// The directions page over fakes, as the tests of the picker open it.
class Picker {
  Picker()
    : routes = FakeRoutes(
        by: {
          TravelMode.driving: drivingRoutes,
          TravelMode.walking: walkingRoutes,
          TravelMode.cycling: cyclingRoutes,
        },
      ),
      transit = FakeTransit(immediately: fakePlan(journeys: 2)),
      map = FakeMap()..hasGround = true,
      sessions = FakeSessions(),
      store = fakeStore() {
    navigation = fakeNavigationController(sessions: sessions);
  }

  final FakeRoutes routes;
  final FakeTransit transit;
  final FakeMap map;
  final FakeSessions sessions;
  final Store store;
  late final NavigationController navigation;

  /// What the page takes the time to be: 3:20 in the afternoon, wherever the test
  /// runs.
  DateTime now = DateTime(2026, 11, 9, 15, 20);

  /// Leaves every question for the test to answer.
  void answerNothing() {
    routes.immediatelyBy.clear();
    transit.immediately = null;
  }

  /// The questions asked for [mode], in order.
  List<RouteQuestion> asked(TravelMode mode) =>
      routes.asked.where((question) => question.mode == mode).toList();

  /// Opens directions to [destination] from [here], and waits for it to settle
  /// unless told not to.
  Future<GoRouter> open(
    WidgetTester tester, {
    bool settle = true,
    TargetPlatform? platform,
    Size size = appScreen,
    FakeLocation? location,
  }) async {
    await store.places.put(destination);
    final router = await pumpFakeApp(
      tester,
      platform: platform,
      size: size,
      navigation: navigation,
      store: store,
      routes: routes,
      transit: transit,
      map: map,
      clock: () => now,
      locationSource: location ?? FakeLocation(now: here),
      location: placeLocation(destination.id),
    );
    // As the place's Directions button does, which the largest text sizes leave
    // out of a small window's reach.
    unawaited(router.push(directionsLocation(destination.id)));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 500));
    if (settle) await tester.pumpAndSettle();
    return router;
  }
}

Finder get tabs => find.byType(ModeTabs);
Finder get rows => find.byType(OptionRow);
Finder get slots => find.byType(SheetSlot);

/// The directions page's buttons along the bottom of the window.
Finder get footer => find.descendant(
  of: find.byType(DirectionsPage),
  matching: find.byType(TripButtons),
);

/// The slot of the list the directions page holds in view, or null if it holds
/// none, as where the text is too large for a row to be held under the tabs.
Object? heldSlot(WidgetTester tester) => tester
    .widget<SheetContent>(
      find.descendant(
        of: find.byType(DirectionsPage),
        matching: find.byType(SheetContent),
      ),
    )
    .held;

SheetController sheetOf(WidgetTester tester) =>
    SheetScaffold.controllerOf(tester.element(tabs));

/// Where the top of the sheet's content is in the window: under the handle, or the
/// status bar once the sheet is as high as it goes.
double contentTop(WidgetTester tester) {
  final view = SheetScaffold.viewOf(
    tester.element(find.byType(DirectionsPage)),
  );
  final window = tester.view.physicalSize / tester.view.devicePixelRatio;
  return window.height - view.extent + view.geometry.contentOffset(view.extent);
}

/// The top of the buttons, which a held row stops above.
double footerTop(WidgetTester tester) => tester.getRect(footer).top;

/// Whether each row is the selected one, in the list's order.
List<bool> selectedRows(WidgetTester tester) => [
  for (final row in tester.widgetList<OptionRow>(rows)) row.selected,
];

/// Where each slot of the list is, whatever is painted of it there.
List<Rect> slotRects(WidgetTester tester) => [
  for (final element in slots.evaluate())
    (element.renderObject! as RenderBox).localToGlobal(Offset.zero) &
        (element.renderObject! as RenderBox).size,
];

const headerAnchor = DirectionsPage.headerAnchor;
const footerAnchor = SheetAnchor.footer();

/// Where [anchor] rests, in logical pixels of sheet.
double extentOf(SheetController sheet, SheetAnchor anchor) =>
    sheet.anchors.firstWhere((a) => a.anchor == anchor).extent;

Future<void> moveTo(WidgetTester tester, SheetAnchor anchor) async {
  sheetOf(tester).animateTo(anchor);
  await tester.pumpAndSettle();
}

/// Taps the tab of [mode].
Future<void> showMode(WidgetTester tester, TravelMode mode) async {
  await tester.tap(find.descendant(of: tabs, matching: find.byIcon(mode.icon)));
  await tester.pumpAndSettle();
}

/// What the tab of [mode] says under its icon, or null if its line is empty.
String? tabTime(WidgetTester tester, TravelMode mode) =>
    tester.widget<ModeTabs>(tabs).times[mode];
