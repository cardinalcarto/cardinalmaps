import 'dart:async';

import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/settings/accessibility.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/transit/nearby_transit.dart';
import 'package:cardinal/transit/nearby_page.dart';
import 'package:cardinal/transit/route_detail_page.dart';
import 'package:flutter/services.dart';
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

/// [finder] in [id]'s row, on the page it shows: a row with pages also lays out its
/// first, unseen, for its height.
Finder _in(String id, Finder finder) {
  final pages = find.descendant(of: _row(id), matching: find.byType(PageView));
  return find.descendant(
    of: pages.evaluate().isEmpty ? _row(id) : pages,
    matching: finder,
  );
}

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
  late HeadsignStore headsigns;

  setUp(() {
    now = DateTime.fromMillisecondsSinceEpoch(fakeStart * 1000);
    source = FakeNearbySource();
    settings = SettingsStore.inMemory();
    headsigns = HeadsignStore.inMemory();
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
        headsigns: headsigns,
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
      _in('62', find.text('Downtown Seattle / Sand Point')),
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

  group('pages', () {
    const ballard = 'Stop A';
    const uw = 'Stop B';
    const HeadsignStop atBallard = (
      feed: _feed,
      routeId: '44',
      stopId: ballard,
    );

    /// Route 44 to Ballard from Stop A and to UW Medical Center from Stop B.
    Map<String, Object?> u44({
      List<int> toBallard = const [3, 13, 23],
      List<int> toUw = const [9],
    }) => nearbyRoutePagesJson('44', [
      (headsigns: ['Ballard'], minutes: toBallard, stop: ballard),
      (headsigns: ['UW Medical Center'], minutes: toUw, stop: uw),
    ]);

    /// Whether [id]'s row shows [headsign] where a rider can see it.
    bool shows(WidgetTester tester, String id, String headsign) {
      final row = tester.getRect(_row(id));
      return _in(id, find.text(headsign)).evaluate().any((e) {
        final box = e.renderObject! as RenderBox;
        final center = box.localToGlobal(box.size.center(Offset.zero));
        return row.contains(center);
      });
    }

    Future<void> swipe(WidgetTester tester, String id) async {
      await tester.fling(_row(id), const Offset(-300, 0), 1000);
      await tester.pumpAndSettle();
    }

    late List<String> haptics;
    setUp(() => haptics = []);

    Future<void> listenForHaptics(WidgetTester tester) async {
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'HapticFeedback.vibrate') {
            haptics.add(call.arguments as String);
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
    }

    testWidgets('opens on the soonest page without a remembered one', (
      tester,
    ) async {
      await open(tester);
      await answer(tester, [
        u44(toBallard: [12], toUw: [4]),
      ]);
      expect(shows(tester, '44', 'UW Medical Center'), isTrue);
      expect(shows(tester, '44', 'Ballard'), isFalse);
      expect(
        find.descendant(
          of: _row('44'),
          matching: find.byKey(const ValueKey('dots')),
        ),
        findsOneWidget,
      );
    });

    testWidgets("a row's lines have room above and below them", (tester) async {
      await open(tester);
      await answer(tester, [_route('40')]);
      final row = tester.getRect(_row('40'));
      // The outermost row of the badge, stop, and times.
      final lines = tester.getRect(
        find
            .ancestor(
              of: find.text('Fremont Ave N & N 34th St'),
              matching: find.descendant(
                of: _row('40'),
                matching: find.byType(Row),
              ),
            )
            .last,
      );
      // 3 px more each way than a detail's header.
      expect(nearbyRowPadding, 11);
      expect(lines.top - row.top, closeTo(11, 0.5));
      expect(row.bottom - lines.bottom, closeTo(11, 0.5));
    });

    testWidgets('a route with one page has no dots', (tester) async {
      await open(tester);
      await answer(tester, [_route('40')]);
      expect(_in('40', find.byKey(const ValueKey('dots'))), findsNothing);
    });

    testWidgets('opens on the remembered page', (tester) async {
      await headsigns.remember(atBallard, 'Ballard', chosenAt: now);
      await open(tester);
      await answer(tester, [
        u44(toBallard: [12], toUw: [4]),
      ]);
      expect(shows(tester, '44', 'Ballard'), isTrue);
      expect(_next(tester, '44').data, '12');
    });

    testWidgets('a swipe shows the next page, with a light haptic, and '
        'remembers it', (tester) async {
      await listenForHaptics(tester);
      await open(tester);
      await answer(tester, [u44()]);
      expect(shows(tester, '44', 'Ballard'), isTrue);
      final height = tester.getSize(_row('44')).height;

      await swipe(tester, '44');
      expect(shows(tester, '44', 'UW Medical Center'), isTrue);
      expect(shows(tester, '44', 'Ballard'), isFalse);
      expect(haptics, ['HapticFeedbackType.lightImpact']);
      // One departure instead of three, and the row is as tall.
      expect(tester.getSize(_row('44')).height, height);
      expect(
        await headsigns.recall([
          (feed: _feed, routeId: '44', stopId: uw),
          atBallard,
        ]),
        {
          (feed: _feed, routeId: '44', stopId: uw): RememberedHeadsign(
            'UW Medical Center',
            now,
          ),
        },
      );

      // At the last page, a swipe settles where it was: nothing changes.
      await swipe(tester, '44');
      expect(haptics, hasLength(1));
    });

    group('a swipe moves the pages wherever on the row it starts:', () {
      /// Flings left from [at], and lets the row settle.
      Future<void> swipeFrom(WidgetTester tester, Offset at) async {
        await tester.flingFrom(at, const Offset(-300, 0), 1000);
        await tester.pumpAndSettle();
      }

      Offset centerOf(WidgetTester tester, Finder finder) {
        expect(finder, findsOneWidget);
        return tester.getCenter(finder);
      }

      final starts = <String, Offset Function(WidgetTester tester)>{
        'the dots': (tester) => centerOf(
          tester,
          find.descendant(
            of: _row('44'),
            matching: find.byKey(const ValueKey('dots')),
          ),
        ),
        // The dots' strip at the row's start, away from the dots themselves.
        'the dots strip': (tester) {
          final row = tester.getRect(_row('44'));
          final dots = tester.getRect(
            find.descendant(
              of: _row('44'),
              matching: find.byKey(const ValueKey('dots')),
            ),
          );
          return Offset(row.left + 8, dots.center.dy);
        },
        'the stop line': (tester) =>
            centerOf(tester, _in('44', find.text(ballard))),
        'the countdown': (tester) =>
            centerOf(tester, _in('44', find.byKey(const ValueKey('next')))),
        // The space above the lines, at the row's start.
        'the padding above the lines': (tester) => Offset(
          tester.getRect(_row('44')).left + 4,
          tester.getRect(_row('44')).top + nearbyRowPadding / 2,
        ),
        // Between the headsign line and the stop line, at the row's start.
        'the gap between lines': (tester) {
          final headsign = tester.getRect(_in('44', find.text('Ballard')));
          final stop = tester.getRect(_in('44', find.text(ballard)));
          return Offset(
            tester.getRect(_row('44')).left + 4,
            (headsign.bottom + stop.top) / 2,
          );
        },
      };
      for (final MapEntry(key: name, value: start) in starts.entries) {
        testWidgets(name, (tester) async {
          await open(tester);
          await answer(tester, [u44()]);
          expect(shows(tester, '44', 'Ballard'), isTrue);

          await swipeFrom(tester, start(tester));
          expect(shows(tester, '44', 'UW Medical Center'), isTrue);
          expect(shows(tester, '44', 'Ballard'), isFalse);
        });
      }
    });

    testWidgets('a tap on a row with pages still opens its detail, on the dots '
        'too', (tester) async {
      await open(tester);
      await answer(tester, [u44()]);
      await tester.tap(
        find.descendant(
          of: _row('44'),
          matching: find.byKey(const ValueKey('dots')),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(RouteDetailPage), findsOneWidget);
    });

    testWidgets("a page with nothing left that runs keeps the row's height", (
      tester,
    ) async {
      await open(tester);
      await answer(tester, [
        nearbyRoutePagesJson('44', [
          (headsigns: ['Ballard'], minutes: [3, 13, 23], stop: ballard),
          (headsigns: ['UW Medical Center'], minutes: [1], stop: uw),
        ]),
      ]);
      final height = tester.getSize(_row('44')).height;
      await swipe(tester, '44');
      await wait(tester, const Duration(minutes: 2));
      expect(shows(tester, '44', 'UW Medical Center'), isTrue);
      expect(tester.getSize(_row('44')).height, height);
    });

    testWidgets('a swipe does not move the row', (tester) async {
      await open(tester);
      await answer(tester, [
        u44(toBallard: [3], toUw: [20]),
        _route('62', departures: [nearbyDepartureJson('b', 6)]),
      ]);
      final y44 = tester.getTopLeft(_row('44')).dy;
      final y62 = tester.getTopLeft(_row('62')).dy;
      expect(y44, lessThan(y62));
      await swipe(tester, '44');
      expect(shows(tester, '44', 'UW Medical Center'), isTrue);
      expect(tester.getTopLeft(_row('44')).dy, y44);
      expect(tester.getTopLeft(_row('62')).dy, y62);

      // Nor does the next refresh from here.
      now = now.add(nearbyRefresh);
      await tester.pump(nearbyRefresh);
      await answer(tester, [
        u44(toBallard: [3], toUw: [20]),
        _route('62', departures: [nearbyDepartureJson('b', 6)]),
      ]);
      expect(tester.getTopLeft(_row('44')).dy, y44);
      expect(shows(tester, '44', 'UW Medical Center'), isTrue);
    });

    testWidgets('rows are ordered by the page each shows', (tester) async {
      await headsigns.remember(atBallard, 'Ballard', chosenAt: now);
      await open(tester);
      // 44's soonest is to UW, in 2 minutes; it shows Ballard's 12.
      await answer(tester, [
        u44(toBallard: [12], toUw: [2]),
        _route('62', departures: [nearbyDepartureJson('b', 5)]),
      ]);
      expect(
        tester.getTopLeft(_row('62')).dy,
        lessThan(tester.getTopLeft(_row('44')).dy),
      );
    });

    testWidgets('a row keeps its page across a refresh', (tester) async {
      await open(tester);
      await answer(tester, [u44()]);
      await swipe(tester, '44');
      final row = tester.element(_row('44'));

      // A page now comes first, and Ballard leaves sooner than ever.
      now = now.add(nearbyRefresh);
      await tester.pump(nearbyRefresh);
      await answer(tester, [
        nearbyRoutePagesJson('44', [
          (headsigns: ['Aurora Village'], minutes: [1], stop: ballard),
          (headsigns: ['Ballard'], minutes: [1], stop: ballard),
          (headsigns: ['UW Medical Center'], minutes: [9], stop: uw),
        ]),
      ]);
      expect(tester.element(_row('44')), same(row));
      expect(shows(tester, '44', 'UW Medical Center'), isTrue);
    });

    testWidgets('merged headsigns read with a slash', (tester) async {
      await open(tester);
      await answer(tester, [
        nearbyRoutePagesJson('44', [
          (
            headsigns: ['University District', 'UW Medical Center'],
            minutes: [4],
            stop: uw,
          ),
        ]),
      ]);
      expect(
        _in('44', find.text('University District / UW Medical Center')),
        findsOneWidget,
      );
    });
  });

  testWidgets('shows the schedule first, then real-time data in place', (
    tester,
  ) async {
    final landing = source.realtimeLanding = Completer<bool>();
    await open(tester);
    await answer(tester, [
      _route('40', departures: [nearbyDepartureJson('a', 3)]),
      _route('62', departures: [nearbyDepartureJson('b', 8)]),
    ]);
    expect(_next(tester, '40').data, '3');
    expect(source.asked, hasLength(1));
    final row40 = tester.element(_row('40'));
    final y40 = tester.getTopLeft(_row('40')).dy;
    final y62 = tester.getTopLeft(_row('62')).dy;

    landing.complete(true);
    await tester.pump();
    expect(source.asked, hasLength(2));
    // 40 is now late, and leaves after 62: it stays where it was.
    await answer(tester, [
      _route('62', departures: [nearbyDepartureJson('b', 8)]),
      _route('40', departures: [nearbyDepartureJson('a', 10, status: 'live')]),
    ]);
    expect(tester.element(_row('40')), same(row40));
    expect(tester.getTopLeft(_row('40')).dy, y40);
    expect(tester.getTopLeft(_row('62')).dy, y62);
    expect(_next(tester, '40').data, '10');
    expect(_in('40', find.byKey(const ValueKey('live'))), findsOneWidget);
  });

  testWidgets('coming back keeps the rows and refreshes them in place', (
    tester,
  ) async {
    await open(tester);
    await answer(tester, [_route('40'), _route('62')]);
    final row40 = tester.element(_row('40'));

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    await tester.pump();
    // Nothing is asked while it's away.
    now = now.add(const Duration(minutes: 2));
    await tester.pump(const Duration(minutes: 2));
    expect(source.asked, hasLength(1));
    expect(_row('40'), findsOneWidget);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpAndSettle();
    expect(source.asked, hasLength(2));
    expect(find.text('Finding departures near you…'), findsNothing);
    expect(find.text('Finding your location…'), findsNothing);
    expect(tester.element(_row('40')), same(row40));

    await answer(tester, [_route('62'), _route('40')]);
    expect(
      tester.getTopLeft(_row('40')).dy,
      lessThan(tester.getTopLeft(_row('62')).dy),
    );
  });

  testWidgets('shows the clock time past 60 minutes', (tester) async {
    await open(tester);
    await answer(tester, [
      _route(
        '40',
        departures: [
          nearbyDepartureJson('a', 61),
          nearbyDepartureJson('b', 107),
        ],
      ),
      _route(
        '62',
        departures: [
          nearbyDepartureJson('c', 60),
          nearbyDepartureJson('d', 90),
        ],
      ),
    ]);
    String at(int minutes) =>
        const DefaultMaterialLocalizations().formatTimeOfDay(
          TimeOfDay.fromDateTime(now.add(Duration(minutes: minutes))),
        );
    expect(_next(tester, '40').data, at(61));
    expect(_in('40', find.text(' min')), findsNothing);
    expect(_following(tester, '40').map((s) => s.text), [at(107)]);
    expect(_next(tester, '62').data, '60');
    expect(_following(tester, '62').map((s) => s.text), [at(90)]);
  });

  group('sheet', () {
    /// 40% of [appScreen]'s height: the lower anchor.
    const lower = 320.0;

    SheetController sheet(WidgetTester tester) =>
        SheetScaffold.controllerOf(tester.element(find.text('Nearby transit')));

    List<double> anchors(WidgetTester tester) => [
      for (final a in sheet(tester).anchors) a.extent,
    ];

    /// Drags the sheet by [dy] from its header, and lets it settle.
    Future<void> drag(WidgetTester tester, double dy) async {
      await tester.drag(find.text('Nearby transit'), Offset(0, dy));
      await tester.pumpAndSettle();
    }

    List<Map<String, Object?>> routes(int n) => [
      for (var i = 0; i < n; i++) _route('${100 + i}'),
    ];

    testWidgets('a long list opens at 40% and goes up to the full screen', (
      tester,
    ) async {
      await open(tester);
      await answer(tester, routes(20));
      expect(anchors(tester), [lower, appScreen.height]);
      expect(sheet(tester).extent, lower);

      await drag(tester, -600);
      expect(sheet(tester).extent, appScreen.height);
    });

    testWidgets('never rests below 40%', (tester) async {
      await open(tester);
      await answer(tester, routes(20));
      await drag(tester, 200);
      expect(sheet(tester).extent, lower);
    });

    testWidgets('with a few rows, tops out where the content ends', (
      tester,
    ) async {
      await open(tester);
      await answer(tester, routes(3));
      final [low, high] = anchors(tester);
      // Three rows end a little above 40%.
      expect(low, lower);
      expect(high, greaterThan(lower));
      expect(high, lessThan(lower + 80));
      expect(sheet(tester).extent, lower);

      await drag(tester, -600);
      expect(sheet(tester).extent, high);
    });

    testWidgets(
      'with only a message, the content ends below 40%, which is the top',
      (tester) async {
        await open(tester);
        expect(find.text('Finding departures near you…'), findsOneWidget);
        // The library can't take the lower of 40% and the content's end: the end
        // becomes the lower anchor, and 40%, past the content, the upper.
        final [end, top] = anchors(tester);
        expect(end, lessThan(lower));
        expect(top, lower);
        expect(sheet(tester).extent, lower);

        await drag(tester, -600);
        expect(sheet(tester).extent, lower);
        await drag(tester, 200);
        expect(sheet(tester).extent, end);
      },
    );

    testWidgets('rows arriving at 40% leave the sheet there', (tester) async {
      await open(tester);
      final extents = <double>[];
      final controller = sheet(tester);
      void heard() => extents.add(controller.extent);
      controller.addListener(heard);
      addTearDown(() => controller.removeListener(heard));

      Future<void> refresh() async {
        now = now.add(nearbyRefresh);
        await tester.pump(nearbyRefresh);
      }

      await answer(tester, routes(1));
      await refresh();
      await answer(tester, routes(20));
      await refresh();
      await answer(tester, routes(2));
      expect(source.asked, hasLength(3));
      expect(controller.extent, lower);
      expect(extents.where((e) => e != lower), isEmpty);
    });
  });
}
