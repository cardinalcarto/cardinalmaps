import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/settings/accessibility.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/transit/nearby_transit.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_location.dart';
import '../fakes/fake_nearby.dart';
import '../fakes/fake_place_store.dart';
import '../fakes/fake_saved_store.dart';
import '../fakes/fake_transit.dart';

/// Fremont, Seattle.
const _here = Fix(lng: -122.35, lat: 47.65, accuracy: 5);

const _feed = 'us-wa-Metro-Transit';

/// A route's JSON with [departures] (from [nearbyDepartureJson]) on one page.
Map<String, Object?> _route(
  String id, {
  List<Map<String, Object?>>? departures,
  List<String> headsigns = const ['Downtown Seattle'],
  int walkSeconds = 0,
}) {
  final json = nearbyRouteJson(id, walkSeconds: walkSeconds);
  final page = (json['pages']! as List).single as Map<String, Object?>
    ..['headsigns'] = headsigns;
  if (departures != null) page['departures'] = departures;
  return json;
}

Finder _row(String id) =>
    find.byKey(ValueKey<RouteKey>((feed: _feed, routeId: id)));

Finder _in(String id, Finder finder) =>
    find.descendant(of: _row(id), matching: finder);

/// The text of [id]'s following departures, as spans.
List<TextSpan> _following(WidgetTester tester, String id) {
  final text = tester.widget<Text>(
    _in(id, find.byKey(const ValueKey('following'))),
  );
  final spans = <TextSpan>[];
  text.textSpan!.visitChildren((span) {
    if (span is TextSpan && span.text != null) spans.add(span);
    return true;
  });
  return spans;
}

/// The big countdown's number in [id]'s row, with its style.
Text _next(WidgetTester tester, String id) => tester.widget<Text>(
  find
      .descendant(
        of: _in(id, find.byKey(const ValueKey('next'))),
        matching: find.byType(Text),
      )
      .first,
);

void main() {
  late DateTime now;
  late FakeNearbySource source;
  late SettingsStore settings;

  setUp(() {
    now = DateTime.fromMillisecondsSinceEpoch(fakeStart * 1000);
    source = FakeNearbySource();
    settings = SettingsStore.inMemory();
  });

  Future<void> open(WidgetTester tester) async {
    final places = FakePlaceStore();
    await pumpFakeApp(
      tester,
      location: '/transit',
      store: Store.withoutDatabase(
        places: places,
        saved: FakeSavedStore(places),
        settings: settings,
      ),
      nearby: source,
      clock: () => now,
      locationSource: FakeLocation(now: _here),
    );
  }

  Future<void> answer(
    WidgetTester tester,
    List<Map<String, Object?>> routes,
  ) async {
    source.asked.last.answer.complete(fakeNearby(routes));
    await tester.pumpAndSettle();
  }

  /// Moves the clock on by [by], and lets a second tick.
  Future<void> wait(WidgetTester tester, Duration by) async {
    now = now.add(by);
    await tester.pump(const Duration(seconds: 1));
    await tester.pumpAndSettle();
  }

  testWidgets('asks from where you are as it opens, walking', (tester) async {
    await open(tester);
    final question = source.asked.single;
    expect(question.location, const LngLat(-122.35, 47.65));
    expect(question.access, TransitAccess.walking);
    expect(find.text('Finding departures near you…'), findsOneWidget);
  });

  testWidgets('shows the rows in the order of the answer', (tester) async {
    await open(tester);
    await answer(tester, [
      _route(
        '62',
        headsigns: ['Downtown Seattle', 'Sand Point'],
        walkSeconds: 240,
      ),
      _route('40'),
      _route('E Line'),
    ]);
    final ys = [
      for (final id in ['62', '40', 'E Line']) tester.getTopLeft(_row(id)).dy,
    ];
    expect(ys, orderedEquals([...ys]..sort()));
    expect(_in('62', find.text('62')), findsOneWidget);
    expect(
      _in('62', find.text('Downtown Seattle • Sand Point')),
      findsOneWidget,
    );
    expect(_in('62', find.text('Fremont Ave N & N 34th St')), findsOneWidget);
    expect(_in('62', find.text('4 min')), findsOneWidget);
  });

  testWidgets('counts down by the minute, and says Now in the last', (
    tester,
  ) async {
    await open(tester);
    await answer(tester, [
      _route('40', departures: [nearbyDepartureJson('a', 3)]),
    ]);
    expect(_next(tester, '40').data, '3');
    expect(_in('40', find.text(' min')), findsOneWidget);

    await wait(tester, const Duration(seconds: 61));
    expect(_next(tester, '40').data, '1');

    await wait(tester, const Duration(seconds: 60));
    expect(_next(tester, '40').data, 'Now');
    expect(_in('40', find.text(' min')), findsNothing);
  });

  testWidgets('marks a live departure', (tester) async {
    await open(tester);
    await answer(tester, [
      _route('40', departures: [nearbyDepartureJson('a', 3, status: 'live')]),
      _route('62', departures: [nearbyDepartureJson('b', 4)]),
    ]);
    expect(_in('40', find.byKey(const ValueKey('live'))), findsOneWidget);
    expect(_in('62', find.byKey(const ValueKey('live'))), findsNothing);
  });

  testWidgets(
    'strikes through canceled and skipped ones, dims those out of reach, and '
    'hides those that left',
    (tester) async {
      await open(tester);
      await answer(tester, [
        _route(
          '40',
          walkSeconds: 300,
          departures: [
            nearbyDepartureJson('a', 2, status: 'canceled'),
            nearbyDepartureJson('b', 6),
            nearbyDepartureJson('c', 8, status: 'skipped'),
            nearbyDepartureJson('d', 12),
          ],
        ),
      ]);
      // A canceled one is never the countdown.
      expect(_next(tester, '40').data, '6');
      final spans = _following(tester, '40');
      final canceled = spans.firstWhere((s) => s.text == '2 min');
      expect(canceled.style?.decoration, TextDecoration.lineThrough);
      expect(spans.map((s) => s.text), contains(' Canceled'));
      final skipped = spans.firstWhere((s) => s.text == '8 min');
      expect(skipped.style?.decoration, TextDecoration.lineThrough);
      expect(spans.map((s) => s.text), contains(' Skipped'));
      expect(_next(tester, '40').style?.color?.a, 1);

      // 4.5 minutes to go, and a 5-minute walk.
      await wait(tester, const Duration(seconds: 89));
      final dimmed = _next(tester, '40');
      expect(dimmed.data, '4');
      expect(dimmed.style?.color?.a, closeTo(0.38, 0.01));

      // The canceled one's time has passed.
      await wait(tester, const Duration(seconds: 60));
      expect(
        _following(tester, '40').map((s) => s.text),
        isNot(contains(' Canceled')),
      );

      // So has the next one's: the one after counts down.
      await wait(tester, const Duration(minutes: 4));
      expect(_next(tester, '40').data, '5');
      expect(_next(tester, '40').style?.color?.a, 1);
      expect(_following(tester, '40').map((s) => s.text), [
        '1 min',
        ' Skipped',
      ]);
    },
  );

  testWidgets("keeps each route's row, and its order, across a refresh", (
    tester,
  ) async {
    await open(tester);
    await answer(tester, [_route('40'), _route('62')]);
    final row40 = tester.element(_row('40'));
    final row62 = tester.element(_row('62'));

    // It asks again every 30 seconds.
    now = now.add(const Duration(seconds: 30));
    await tester.pump(const Duration(seconds: 30));
    expect(source.asked, hasLength(2));
    await answer(tester, [_route('E Line'), _route('62'), _route('40')]);

    expect(tester.element(_row('40')), same(row40));
    expect(tester.element(_row('62')), same(row62));
    final ys = [
      for (final id in ['40', '62', 'E Line']) tester.getTopLeft(_row(id)).dy,
    ];
    expect(ys, orderedEquals([...ys]..sort()));
  });

  testWidgets('asks by wheelchair when that is the default, and again when it '
      'changes', (tester) async {
    await settings.set(wheelchairByDefault, true);
    await open(tester);
    expect(source.asked.single.access, TransitAccess.wheelchair);
    await answer(tester, [_route('40')]);

    await settings.set(wheelchairByDefault, false);
    await tester.pumpAndSettle();
    expect(source.asked, hasLength(2));
    expect(source.asked.last.access, TransitAccess.walking);
  });

  testWidgets('says so without your location, and asks nothing', (
    tester,
  ) async {
    final places = FakePlaceStore();
    await pumpFakeApp(
      tester,
      location: '/transit',
      store: Store.withoutDatabase(
        places: places,
        saved: FakeSavedStore(places),
        settings: settings,
      ),
      nearby: source,
      clock: () => now,
      locationSource: FakeLocation(enabled: false, now: _here),
    );
    expect(
      find.text('Turn on location to see transit near you.'),
      findsOneWidget,
    );
    expect(source.asked, isEmpty);
  });
}
