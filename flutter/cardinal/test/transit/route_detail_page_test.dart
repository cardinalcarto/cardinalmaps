import 'dart:async';

import 'package:cardinal/map/location.dart';
import 'package:cardinal/directions/transit_sheet.dart' show liveColor;
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/transit/nearby_transit.dart';
import 'package:cardinal/transit/route_detail.dart';
import 'package:cardinal/transit/route_detail_page.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_location.dart';
import '../fakes/fake_map.dart';
import '../fakes/fake_nearby.dart';
import '../fakes/fake_place_store.dart';
import '../fakes/fake_saved_store.dart';
import '../fakes/fake_transit.dart';

/// Fremont, Seattle.
const _here = Fix(lng: -122.35, lat: 47.65, accuracy: 5);

const _feed = 'us-wa-Metro-Transit';
const _ballard = 'Stop A';
const _uw = 'Stop B';

/// Route 44 to Ballard from Stop A, and to UW Medical Center from Stop B, after 15 other
/// routes that leave sooner.
List<Map<String, Object?>> _routes() => [
  for (var i = 0; i < 15; i++) nearbyRouteJson('${100 + i}'),
  nearbyRoutePagesJson('44', [
    (headsigns: ['Ballard'], minutes: [30, 40], stop: _ballard),
    (headsigns: ['UW Medical Center'], minutes: [35], stop: _uw),
  ]),
];

Map<String, Object?> _stop(String name, {int i = 0}) => {
  'accessibility': 'unknown',
  'feed': _feed,
  'stop_id': name,
  'name': name,
  'latitude': 47.65 + i * 0.01,
  'longitude': -122.35,
};

/// A ride of [trip], leaving [minutes] after [fakeStart] from the first of [stops], two
/// minutes between each, its times live if [live].
Map<String, Object?> _ride(
  String trip,
  int minutes,
  List<String> stops, {
  bool live = false,
}) {
  final t = fakeStart + minutes * 60;
  return {
    'accessibility': 'unknown',
    'departure': t,
    'arrival': t + (stops.length - 1) * 120,
    'duration_seconds': (stops.length - 1) * 120,
    'realtime': live,
    'route': {'route_id': '44', 'short_name': '44', 'route_type': 3},
    'trip': {'trip_id': trip, 'start_date': '20261109', 'headsign': 'Ballard'},
    'stops': [
      for (final (i, name) in stops.indexed)
        {
          ..._stop(name, i: i),
          'arrival': t + i * 120,
          'departure': t + i * 120,
          if (live) 'predicted_departure': t + i * 120,
        },
    ],
  };
}

/// A route detail of 44 from [stop] under [headsigns].
RouteDetail _detail(
  List<Map<String, Object?>> departures, {
  List<String> headsigns = const ['Ballard'],
  String stop = _ballard,
  Map<String, Object?>? ride,
  String? shapeToStop,
  List<Map<String, Object?>> vehicles = const [],
  int walkSeconds = 240,
}) => RouteDetail.fromJson({
  'generated_at': fakeStart,
  'realtime': {'level': 'schedule', 'built_at': fakeStart, 'feeds': <Object>[]},
  'feed': _feed,
  'route': {
    'route_id': '44',
    'short_name': '44',
    'route_type': 3,
    'color': 'FDB71A',
  },
  'agency': {'name': 'Metro Transit'},
  'headsigns': headsigns,
  'stop': _stop(stop),
  'walk_seconds': walkSeconds,
  'departures': departures,
  'ride': ?ride,
  'shape_to_stop': ?shapeToStop,
  'vehicles': vehicles,
});

TripRun _run(String trip) => (tripId: trip, startDate: '20261109');

Finder _tile(String trip) => find.byKey(ValueKey(_run(trip)));

Finder _row(String id) =>
    find.byKey(ValueKey<RouteKey>((feed: _feed, routeId: id)));

/// [finder] on the page a paged row or header shows: it also lays out its first, unseen,
/// for its height.
Finder _shown(Finder of, Finder finder) {
  final pages = find.descendant(of: of, matching: find.byType(PageView));
  return find.descendant(
    of: pages.evaluate().isEmpty ? of : pages,
    matching: finder,
  );
}

final _header = find.byKey(const ValueKey('header'));

/// The tile's countdown number.
Text _countdown(WidgetTester tester, String trip) => tester.widget<Text>(
  find
      .descendant(
        of: find.descendant(
          of: _tile(trip),
          matching: find.byKey(const ValueKey('countdown')),
        ),
        matching: find.byType(Text),
      )
      .first,
);

bool _selected(WidgetTester tester, String trip) => tester
    .widget<DepartureTile>(
      find.descendant(of: _tile(trip), matching: find.byType(DepartureTile)),
    )
    .selected;

void main() {
  late DateTime now;
  late FakeNearbySource source;
  late SettingsStore settings;
  late HeadsignStore headsigns;
  late FakeMap map;
  late GoRouter router;

  setUp(() {
    now = DateTime.fromMillisecondsSinceEpoch(fakeStart * 1000);
    source = FakeNearbySource();
    settings = SettingsStore.inMemory();
    headsigns = HeadsignStore.inMemory();
    map = FakeMap();
  });

  /// Opens the list and answers it with [_routes].
  Future<void> openList(WidgetTester tester) async {
    final places = FakePlaceStore();
    router = await pumpFakeApp(
      tester,
      location: '/transit',
      store: Store.withoutDatabase(
        places: places,
        saved: FakeSavedStore(places),
        settings: settings,
        headsigns: headsigns,
      ),
      nearby: source,
      clock: () => now,
      locationSource: FakeLocation(now: _here),
      map: map,
    );
    source.asked.last.answer.complete(fakeNearby(_routes()));
    await tester.pumpAndSettle();
  }

  SheetController sheet(WidgetTester tester) =>
      SheetScaffold.controllerOf(tester.element(find.text('Nearby transit')));

  /// Raises the sheet to the top and scrolls the list until 44's row shows.
  Future<void> scrollTo44(WidgetTester tester) async {
    await tester.drag(find.text('Nearby transit'), const Offset(0, -700));
    await tester.pumpAndSettle();
    await tester.drag(_row('110'), const Offset(0, -500));
    await tester.pumpAndSettle();
    expect(tester.getCenter(_row('44')).dy, lessThan(appScreen.height - 100));
  }

  Future<void> swipe(WidgetTester tester, Finder finder) async {
    await tester.fling(finder, const Offset(-300, 0), 1000);
    await tester.pumpAndSettle();
  }

  /// Opens 44's detail from its row, on the page it shows.
  Future<void> open44(WidgetTester tester) async {
    await scrollTo44(tester);
    await tester.tap(_row('44'));
    await tester.pumpAndSettle();
  }

  Future<void> answerRoute(WidgetTester tester, RouteDetail detail) async {
    source.routesAsked.last.answer.complete(detail);
    await tester.pumpAndSettle();
  }

  Future<void> back(WidgetTester tester) async {
    router.pop();
    await tester.pumpAndSettle();
  }

  group('opening', () {
    testWidgets(
      'the sheet opens at 40%, and goes up to the end of its content',
      (tester) async {
        await openList(tester);
        await open44(tester);
        await answerRoute(
          tester,
          _detail(
            [nearbyDepartureJson('a', 10), nearbyDepartureJson('b', 20)],
            ride: _ride('a', 10, [
              for (var i = 0; i < 12; i++) i == 0 ? _ballard : 'Stop ${i + 1}',
            ]),
          ),
        );
        final controller = SheetScaffold.controllerOf(
          tester.element(find.byType(RouteDetailPage)),
        );
        final lower = appScreen.height * 0.4;
        expect(controller.anchors.first.extent, lower);
        expect(controller.anchors.last.extent, greaterThan(lower));
        expect(controller.extent, lower);
      },
    );

    testWidgets('a row opens on the page it shows, asking for its first '
        'catchable trip', (tester) async {
      await openList(tester);
      await scrollTo44(tester);
      await swipe(tester, _row('44'));
      await tester.tap(_row('44'));
      await tester.pumpAndSettle();

      expect(
        router.state.uri.toString(),
        '/transit/route/us-wa-Metro-Transit/44/Stop%20B'
        '?headsign=UW+Medical+Center',
      );
      expect(find.byType(RouteDetailPage), findsOneWidget);
      expect(_shown(_header, find.text('UW Medical Center')), findsOneWidget);
      expect(_shown(_header, find.text(_uw)), findsOneWidget);
      final query = source.routesAsked.single.query;
      expect(query.feed, _feed);
      expect(query.routeId, '44');
      expect(query.stopId, _uw);
      expect(query.headsigns, ['UW Medical Center']);
      expect(query.walkSeconds, 240);
      expect(query.tripId, '44-UW Medical Center-35');
      // The row's departures show at once, as tiles.
      expect(_tile('44-UW Medical Center-35'), findsOneWidget);
    });

    testWidgets('a location names the page by its identifiers', (tester) async {
      expect(
        routeDetailLocation(
          feed: 'a/b',
          routeId: 'R 1',
          stopId: 'S&1',
          headsigns: ['X', 'Y / Z'],
        ),
        '/transit/route/a%2Fb/R%201/S%261?headsign=X&headsign=Y+%2F+Z',
      );
    });

    testWidgets('back keeps the list: its rows, their pages, and its scroll', (
      tester,
    ) async {
      await openList(tester);
      await scrollTo44(tester);
      await swipe(tester, _row('44'));
      final row = tester.element(_row('44'));
      final at = tester.getTopLeft(_row('44'));
      final extent = sheet(tester).extent;

      await tester.tap(_row('44'));
      await tester.pumpAndSettle();
      expect(find.byType(RouteDetailPage), findsOneWidget);
      await answerRoute(
        tester,
        _detail(
          [nearbyDepartureJson('u1', 35)],
          headsigns: ['UW Medical Center'],
          stop: _uw,
        ),
      );
      await back(tester);

      expect(find.byType(RouteDetailPage), findsNothing);
      expect(tester.element(_row('44')), same(row));
      expect(tester.getTopLeft(_row('44')), at);
      expect(sheet(tester).extent, extent);
      expect(
        _shown(_row('44'), find.text('UW Medical Center')),
        findsOneWidget,
      );
      expect(find.text('Finding departures near you…'), findsNothing);
    });
  });

  testWidgets('opened without its row, as when restored, asks by the '
      'identifiers and shows the answer\'s page', (tester) async {
    final places = FakePlaceStore();
    await pumpFakeApp(
      tester,
      location: routeDetailLocation(
        feed: _feed,
        routeId: '44',
        stopId: _ballard,
        headsigns: ['Ballard'],
      ),
      store: Store.withoutDatabase(
        places: places,
        saved: FakeSavedStore(places),
        settings: settings,
        headsigns: headsigns,
      ),
      nearby: source,
      clock: () => now,
      locationSource: FakeLocation(now: _here),
      map: map,
    );
    final query = source.routesAsked.single.query;
    expect((query.feed, query.routeId, query.stopId), (_feed, '44', _ballard));
    expect(query.headsigns, ['Ballard']);
    expect(query.tripId, isNull);
    await answerRoute(
      tester,
      _detail([
        nearbyDepartureJson('a', 10),
      ], ride: _ride('a', 10, [_ballard, 'Leary Ave'])),
    );
    expect(_shown(_header, find.text('Ballard')), findsOneWidget);
    expect(_selected(tester, 'a'), isTrue);
    expect(find.text('Leary Ave'), findsOneWidget);
  });

  testWidgets("a swipe that starts on the header's dots moves its pages", (
    tester,
  ) async {
    await openList(tester);
    await open44(tester);
    await answerRoute(tester, _detail([nearbyDepartureJson('b1', 30)]));
    final dots = find.descendant(
      of: find.byType(RouteDetailPage),
      matching: find.byKey(const ValueKey('dots')),
    );
    expect(dots, findsOneWidget);

    await tester.flingFrom(tester.getCenter(dots), const Offset(-300, 0), 1000);
    await tester.pumpAndSettle();
    expect(_shown(_header, find.text('UW Medical Center')), findsOneWidget);
    expect(source.routesAsked.last.query.stopId, _uw);
  });

  testWidgets('swiping its pages remembers the headsign, as a row does, and '
      'shows that page', (tester) async {
    await openList(tester);
    await open44(tester);
    await answerRoute(tester, _detail([nearbyDepartureJson('b1', 30)]));
    expect(source.routesAsked.first.query.stopId, _ballard);

    await swipe(tester, _header);
    expect(_shown(_header, find.text('UW Medical Center')), findsOneWidget);
    expect(
      await headsigns.recall([(feed: _feed, routeId: '44', stopId: _uw)]),
      {
        (feed: _feed, routeId: '44', stopId: _uw): RememberedHeadsign(
          'UW Medical Center',
          now,
        ),
      },
    );
    final query = source.routesAsked.last.query;
    expect(query.stopId, _uw);
    expect(query.headsigns, ['UW Medical Center']);
    // The page's own departures show while its detail is asked for.
    expect(_tile('44-UW Medical Center-35'), findsOneWidget);
    expect(_tile('b1'), findsNothing);

    // The list stays as it was.
    await back(tester);
    expect(_shown(_row('44'), find.text('Ballard')), findsOneWidget);
  });

  group('tiles', () {
    testWidgets('count down as rows do, and say what each departure is', (
      tester,
    ) async {
      await openList(tester);
      await open44(tester);
      await answerRoute(
        tester,
        _detail([
          nearbyDepartureJson('left', -1),
          nearbyDepartureJson('now', 0, status: 'live'),
          // The walk is 4 minutes.
          nearbyDepartureJson('far', 3),
          nearbyDepartureJson('soon', 12),
          nearbyDepartureJson('off', 15, status: 'canceled'),
          nearbyDepartureJson('skip', 18, status: 'skipped'),
          {
            ...nearbyDepartureJson('on', 22),
            'continues_as': {
              'route_id': '43',
              'short_name': '43',
              'headsign': 'Capitol Hill',
            },
          },
          nearbyDepartureJson('later', 75),
        ]),
      );
      String at(int minutes) =>
          const DefaultMaterialLocalizations().formatTimeOfDay(
            TimeOfDay.fromDateTime(now.add(Duration(minutes: minutes))),
          );

      // Gone once it has left.
      expect(_tile('left'), findsNothing);

      expect(_countdown(tester, 'now').data, 'Now');
      expect(
        find.descendant(
          of: _tile('now'),
          matching: find.byKey(const ValueKey('live')),
        ),
        findsOneWidget,
      );
      expect(
        find.descendant(
          of: _tile('soon'),
          matching: find.byKey(const ValueKey('live')),
        ),
        findsNothing,
      );

      expect(_countdown(tester, 'soon').data, '12');
      expect(
        find.descendant(of: _tile('soon'), matching: find.text(at(12))),
        findsOneWidget,
      );
      expect(
        find.descendant(of: _tile('soon'), matching: find.text(' min')),
        findsOneWidget,
      );
      expect(_countdown(tester, 'soon').style!.color!.a, 1);

      // Too soon to walk to: dimmed.
      expect(_countdown(tester, 'far').style!.color!.a, closeTo(0.38, 0.01));

      for (final (trip, label) in [('off', 'Canceled'), ('skip', 'Skipped')]) {
        expect(
          _countdown(tester, trip).style!.decoration,
          TextDecoration.lineThrough,
        );
        expect(
          find.descendant(of: _tile(trip), matching: find.text(label)),
          findsOneWidget,
        );
      }

      expect(
        find.descendant(
          of: _tile('on'),
          matching: find.text('continues as 43'),
        ),
        findsOneWidget,
      );
      expect(find.textContaining('continues as'), findsOneWidget);

      // Past an hour, the clock time.
      expect(_countdown(tester, 'later').data, at(75));

      // One headsign: no tile says it.
      expect(find.byKey(const ValueKey('headsign')), findsNothing);

      // The first catchable is selected: the walk can't make "Now" or "far".
      expect(_countdown(tester, 'now').style!.color!.a, closeTo(0.38, 0.01));
      expect(_selected(tester, 'soon'), isTrue);
      expect(_selected(tester, 'now'), isFalse);
      expect(_selected(tester, 'far'), isFalse);

      // A minute on, "Now" has left.
      now = now.add(const Duration(minutes: 1));
      await tester.pump(const Duration(seconds: 1));
      await tester.pumpAndSettle();
      expect(_tile('now'), findsNothing);
      expect(_countdown(tester, 'soon').data, '11');
    });

    testWidgets('say their headsign when the page merges several', (
      tester,
    ) async {
      await openList(tester);
      await open44(tester);
      await answerRoute(
        tester,
        _detail(
          [
            nearbyDepartureJson('a', 5, headsign: 'Ballard'),
            nearbyDepartureJson('b', 9, headsign: 'Crown Hill'),
          ],
          headsigns: ['Ballard', 'Crown Hill'],
        ),
      );
      expect(
        find.descendant(of: _tile('a'), matching: find.text('Ballard')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: _tile('b'), matching: find.text('Crown Hill')),
        findsOneWidget,
      );
    });
  });

  group('selecting', () {
    Future<void> openWithRide(WidgetTester tester) async {
      await openList(tester);
      await open44(tester);
      await answerRoute(
        tester,
        _detail([
          nearbyDepartureJson('a', 10),
          nearbyDepartureJson('b', 20),
          nearbyDepartureJson('c', 30, status: 'canceled'),
          nearbyDepartureJson('d', 40),
        ], ride: _ride('a', 10, [_ballard, 'Leary Ave', 'Ballard End'])),
      );
      // The row's trip at 30 minutes isn't among them: the first catchable is
      // selected, and the answer's ride is its.
      expect(source.routesAsked, hasLength(1));
      expect(_selected(tester, 'a'), isTrue);
      expect(find.text('Leary Ave'), findsOneWidget);
    }

    testWidgets('a tile shows its trip, keeping the old one until it arrives', (
      tester,
    ) async {
      await openWithRide(tester);
      final asked = source.routesAsked.length;
      final top = tester.getTopLeft(find.text('Leary Ave')).dy;

      await tester.tap(_tile('b'));
      await tester.pumpAndSettle();
      expect(source.routesAsked, hasLength(asked + 1));
      final query = source.routesAsked.last.query;
      expect((query.tripId, query.startDate), ('b', '20261109'));
      expect(_selected(tester, 'b'), isTrue);
      expect(_selected(tester, 'a'), isFalse);
      // The old trip stays, where it was, until the new one comes.
      expect(find.text('Leary Ave'), findsOneWidget);
      expect(tester.getTopLeft(find.text('Leary Ave')).dy, top);

      source.routesAsked.last.answer.complete(
        _detail([
          nearbyDepartureJson('a', 10),
          nearbyDepartureJson('b', 20),
          nearbyDepartureJson('d', 40),
        ], ride: _ride('b', 20, [_ballard, 'Market St', 'Ballard End'])),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      // Fading from one to the other.
      expect(find.text('Leary Ave'), findsOneWidget);
      expect(find.text('Market St'), findsOneWidget);
      await tester.pumpAndSettle();
      expect(find.text('Leary Ave'), findsNothing);
      expect(find.text('Market St'), findsOneWidget);
    });

    testWidgets("the trip before the stop goes with its ride's", (
      tester,
    ) async {
      /// The line of the trip before the stop on the map, as latitudes.
      List<Object?>? before() {
        final data = map.geoJson[nearbyRouteSource]! as Map<String, Object?>;
        for (final f in (data['features']! as List).cast<Map>()) {
          if ((f['properties'] as Map)['role'] == 'before') {
            return [
              for (final c in (f['geometry'] as Map)['coordinates'] as List)
                (c as List)[1],
            ];
          }
        }
        return null;
      }

      final departures = [
        nearbyDepartureJson('a', 10),
        nearbyDepartureJson('b', 20),
      ];
      await openList(tester);
      await open44(tester);
      await answerRoute(
        tester,
        _detail(
          departures,
          ride: _ride('a', 10, [_ballard, 'Leary Ave']),
          shapeToStop: polyline6([
            [-122.35, 47.6],
            [-122.35, 47.65],
          ]),
        ),
      );
      expect(before(), [47.6, 47.65]);

      // Another trip selected: the old ride, and the line before it, stay until its own.
      await tester.tap(_tile('b'));
      await tester.pumpAndSettle();
      expect(before(), [47.6, 47.65]);
      await answerRoute(
        tester,
        _detail(
          departures,
          ride: _ride('b', 20, [_ballard, 'Market St']),
          shapeToStop: polyline6([
            [-122.35, 47.62],
            [-122.35, 47.65],
          ]),
        ),
      );
      expect(before(), [47.62, 47.65]);

      // A ride from its trip's first stop has nothing before it.
      now = now.add(routeDetailRefresh);
      await tester.pump(routeDetailRefresh);
      await answerRoute(
        tester,
        _detail(departures, ride: _ride('b', 20, [_ballard, 'Market St'])),
      );
      expect(before(), isNull);
    });

    testWidgets('a canceled tile is not selected', (tester) async {
      await openWithRide(tester);
      final asked = source.routesAsked.length;
      await tester.tap(_tile('c'));
      await tester.pumpAndSettle();
      expect(source.routesAsked, hasLength(asked));
      expect(_selected(tester, 'a'), isTrue);
    });

    testWidgets('marks live times in the timeline', (tester) async {
      await openList(tester);
      await open44(tester);
      await answerRoute(
        tester,
        _detail(
          [nearbyDepartureJson('44-Ballard-30', 30, status: 'live')],
          ride: _ride('44-Ballard-30', 30, [_ballard, 'Leary Ave'], live: true),
        ),
      );
      final time = const DefaultMaterialLocalizations().formatTimeOfDay(
        TimeOfDay.fromDateTime(now.add(const Duration(minutes: 32))),
      );
      final text = tester.widget<Text>(find.text(time));
      expect(
        text.style!.color,
        liveColor(Theme.of(tester.element(find.text(time)))),
      );
    });

    testWidgets('a selected trip that is gone after a refresh falls back to '
        'the first catchable', (tester) async {
      await openWithRide(tester);
      await tester.tap(_tile('b'));
      await tester.pumpAndSettle();
      await answerRoute(
        tester,
        _detail([
          nearbyDepartureJson('a', 10),
          nearbyDepartureJson('b', 20),
        ], ride: _ride('b', 20, [_ballard, 'Market St'])),
      );
      expect(_selected(tester, 'b'), isTrue);

      // A refresh asks for the selected trip, which has been dropped.
      now = now.add(routeDetailRefresh);
      await tester.pump(routeDetailRefresh);
      expect(source.routesAsked.last.query.tripId, 'b');
      final asked = source.routesAsked.length;
      await answerRoute(
        tester,
        _detail([
          nearbyDepartureJson('a', 10),
          nearbyDepartureJson('d', 40),
        ], ride: _ride('b', 20, [_ballard, 'Market St'])),
      );
      expect(_selected(tester, 'a'), isTrue);
      expect(_tile('b'), findsNothing);
      expect(source.routesAsked, hasLength(asked + 1));
      expect(source.routesAsked.last.query.tripId, 'a');
      // b's trip stays until a's comes.
      expect(find.text('Market St'), findsOneWidget);
      await answerRoute(
        tester,
        _detail([
          nearbyDepartureJson('a', 10),
          nearbyDepartureJson('d', 40),
        ], ride: _ride('a', 10, [_ballard, 'Leary Ave'])),
      );
      expect(find.text('Leary Ave'), findsOneWidget);
      expect(find.text('Market St'), findsNothing);
    });

    testWidgets('a selected trip that leaves falls back to the next', (
      tester,
    ) async {
      await openWithRide(tester);
      final asked = source.routesAsked.length;
      now = now.add(const Duration(minutes: 11));
      await tester.pump(const Duration(seconds: 1));
      await tester.pumpAndSettle();
      expect(_tile('a'), findsNothing);
      expect(_selected(tester, 'b'), isTrue);
      expect(source.routesAsked, hasLength(asked + 1));
      expect(source.routesAsked.last.query.tripId, 'b');
      // Until it comes, a's trip stays.
      expect(find.text('Leary Ave'), findsOneWidget);
    });
  });

  group('refreshing', () {
    testWidgets('every 30 s while it shows, on the schedule first', (
      tester,
    ) async {
      await openList(tester);
      final landing = source.realtimeLanding = Completer<bool>();
      await open44(tester);
      await answerRoute(
        tester,
        _detail([
          nearbyDepartureJson('44-Ballard-30', 30),
        ], ride: _ride('44-Ballard-30', 30, [_ballard, 'Leary Ave'])),
      );
      expect(source.routesAsked, hasLength(1));

      landing.complete(true);
      await tester.pump();
      expect(source.routesAsked, hasLength(2));
      await answerRoute(
        tester,
        _detail([
          nearbyDepartureJson('44-Ballard-30', 32, status: 'live'),
        ], ride: _ride('44-Ballard-30', 32, [_ballard, 'Leary Ave'])),
      );
      expect(_countdown(tester, '44-Ballard-30').data, '32');

      now = now.add(routeDetailRefresh);
      await tester.pump(routeDetailRefresh);
      expect(source.routesAsked, hasLength(3));
      expect(source.routesAsked.last.query.tripId, '44-Ballard-30');
    });

    testWidgets('asks nothing once it has closed', (tester) async {
      await openList(tester);
      await open44(tester);
      await answerRoute(tester, _detail([nearbyDepartureJson('a', 31)]));
      await back(tester);
      final asked = source.routesAsked.length;
      now = now.add(routeDetailRefresh * 3);
      await tester.pump(routeDetailRefresh * 3);
      expect(source.routesAsked, hasLength(asked));
    });
  });

  testWidgets('draws the trip, its stops, and vehicles on the map, and clears '
      'them on leaving', (tester) async {
    await openList(tester);
    await open44(tester);
    final subjects = map.subjectsSet;
    await answerRoute(
      tester,
      _detail(
        [nearbyDepartureJson('44-Ballard-30', 30)],
        ride: _ride('44-Ballard-30', 30, [
          _ballard,
          'Leary Ave',
          'Ballard End',
        ]),
        // From well south of Stop A, off the fitted camera.
        shapeToStop: polyline6([
          [-122.35, 47.55],
          [-122.35, 47.6],
          [-122.35, 47.65],
        ]),
        vehicles: [
          {
            'id': '7033',
            'trip_id': '44-Ballard-30',
            'latitude': 47.6,
            'longitude': -122.35,
            'bearing': 90.0,
          },
          {'id': '7034', 'latitude': 47.7, 'longitude': -122.35},
        ],
      ),
    );
    final data = map.geoJson[nearbyRouteSource]! as Map<String, Object?>;
    final features = (data['features']! as List)
        .cast<Map<String, Object?>>()
        .map((f) => f['properties']! as Map<String, Object?>)
        .toList();
    expect(features.map((f) => f['role']), [
      'before',
      'line',
      'stop',
      'stop',
      'boarding',
      'heading',
      'vehicle',
      'vehicle',
    ]);
    // The trip before the stop, in the route's colors, with no stops of its own.
    expect(features.first['color'], '#FDB71A');
    expect(features[1]['color'], '#FDB71A');
    final before =
        ((data['features']! as List).first as Map)['geometry'] as Map;
    expect(before['coordinates'], [
      [-122.35, 47.55],
      [-122.35, 47.6],
      [-122.35, 47.65],
    ]);
    expect(features[5]['bearing'], 90.0);
    expect(map.images.map((i) => i.name), [
      features.last['icon'],
      features[5]['icon'],
    ]);
    // The images came before the data that uses them.
    expect(
      map.calls.lastIndexOf('images'),
      lessThan(map.calls.lastIndexOf(nearbyRouteSource)),
    );
    // The camera fits the stop and the line: Stop A to Ballard End, not the trip before.
    expect(map.subjectsSet, subjects + 1);
    expect(map.subjectBounds!.south, closeTo(47.65, 1e-9));
    expect(map.subjectBounds!.north, closeTo(47.67, 1e-9));

    // A refresh with the same trip leaves the camera alone.
    now = now.add(routeDetailRefresh);
    await tester.pump(routeDetailRefresh);
    await answerRoute(
      tester,
      _detail(
        [nearbyDepartureJson('44-Ballard-30', 30)],
        ride: _ride('44-Ballard-30', 30, [
          _ballard,
          'Leary Ave',
          'Ballard End',
        ]),
      ),
    );
    expect(map.subjectsSet, subjects + 1);
    expect(map.images, hasLength(2));

    await back(tester);
    expect(map.geoJson[nearbyRouteSource], empty);
    expect(map.hasSubject, isFalse);
  });
}
