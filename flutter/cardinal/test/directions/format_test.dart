import 'package:cardinal/directions/format.dart';
import 'package:cardinal/l10n/lookup.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_routes.dart';

DirectionsRoute _route(
  double distance,
  double duration, {
  List<String> names = const [],
  bool toll = false,
  bool ferry = false,
  bool motorway = false,
}) => fakeRoute(
  const [
    [0, 0],
    [1, 1],
  ],
  distance: distance,
  duration: duration,
  roadNames: names,
  toll: toll,
  ferry: ferry,
  motorway: motorway,
);

void main() {
  final words = localizationsFor('en')!;

  test('miles from a tenth of a mile, feet below that', () {
    expect(formatDistance(32.3, units: UnitSystem.imperial), '106 ft');
    expect(formatDistance(161, units: UnitSystem.imperial), '0.1 mi');
    expect(formatDistance(10460, units: UnitSystem.imperial), '6.5 mi');
    expect(formatDistance(32187, units: UnitSystem.imperial), '20 mi');
  });

  test('kilometers from one, meters below that', () {
    expect(formatDistance(420.4, units: UnitSystem.metric), '420 m');
    expect(formatDistance(1500, units: UnitSystem.metric), '1.5 km');
    expect(formatDistance(23400, units: UnitSystem.metric), '23 km');
  });

  test('durations round to minutes and read in hours past an hour', () {
    expect(formatDuration(0), '<1 min');
    expect(formatDuration(20), '<1 min');
    expect(formatDuration(672), '11 min');
    expect(formatDuration(7380), '2 hr 3 min');
    expect(formatDuration(7200), '2 hr');
  });

  test('a list is joined as the language writes one', () {
    expect(formatList(words, []), '');
    expect(formatList(words, ['SR 99']), 'SR 99');
    expect(formatList(words, ['SR 99', 'SR 509']), 'SR 99 and SR 509');
    expect(
      formatList(words, ['SR 99', 'SR 509', 'Ambaum Boulevard']),
      'SR 99, SR 509, and Ambaum Boulevard',
    );
    expect(formatList(words, ['A', 'B', 'C', 'D', 'E']), 'A, B, C, D, and E');
  });

  test('a route is named for its roads, in the order it takes them', () {
    expect(
      routeName(words, _route(1, 1, names: ['I-5 South'])),
      'via I-5 South',
    );
    expect(
      routeName(words, _route(1, 1, names: ['SR 99', 'SR 509'])),
      'via SR 99 and SR 509',
    );
    expect(routeName(words, _route(1, 1)), isNull, reason: 'no named roads');
  });

  group('what a route\'s row says of it', () {
    final freeway = _route(22850, 1320, motorway: true);
    final tolled = _route(22200, 1500, motorway: true, toll: true);
    final streets = _route(21080, 2040);
    final ferry = _route(30000, 2700, ferry: true, toll: true);
    final routes = [freeway, tolled, streets, ferry];
    List<String> facts(
      List<DirectionsRoute> routes,
      int index, {
      bool driving = true,
    }) =>
        routeFacts(routes, index, driving: driving, units: UnitSystem.imperial);

    test('the first is the fastest', () {
      expect(facts(routes, 0), ['14 mi', 'Fastest']);
    });

    test('the others are slower by the minutes their times differ', () {
      expect(facts(routes, 1), ['14 mi', '3 min slower', 'Tolls']);
      expect(facts(routes, 2).sublist(0, 2), ['13 mi', '12 min slower']);
      // 22 min and 1 hr 26 min.
      expect(facts([freeway, _route(90000, 5160)], 1)[1], '1 hr 4 min slower');
      // Both show 22 min.
      expect(facts([freeway, _route(23000, 1340)], 1)[1], '<1 min slower');
    });

    test(
      'the quickest is the fastest, though the router put another first',
      () {
        final routes = [freeway, _route(30000, 1200, motorway: true)];
        expect(facts(routes, 1), ['19 mi', 'Fastest']);
        expect(facts(routes, 0)[1], endsWith('slower'));
      },
    );

    test('the shortest says so, unless it\'s the only one', () {
      expect(facts(routes, 2), contains('Shortest'));
      expect(facts(routes, 0), isNot(contains('Shortest')));
      expect(facts([streets], 0), ['13 mi', 'Fastest']);
      expect(facts([streets, ferry], 0), ['13 mi', 'Fastest', 'Shortest']);
      // Of two as short, the first.
      expect(facts([freeway, freeway], 0), contains('Shortest'));
      expect(facts([freeway, freeway], 1), isNot(contains('Shortest')));
    });

    test('a drive says its ferry, that it has no freeways when another '
        'has, and its tolls, in that order', () {
      expect(facts(routes, 2), [
        '13 mi',
        '12 min slower',
        'Shortest',
        'No freeways',
      ]);
      expect(facts(routes, 3), [
        '19 mi',
        '23 min slower',
        'Ferry',
        'No freeways',
        'Tolls',
      ]);
      // With no freeway on any, there's nothing to tell apart.
      expect(facts([streets, ferry], 0), ['13 mi', 'Fastest', 'Shortest']);
    });

    test('a walk or a ride says none of that', () {
      expect(facts(routes, 3, driving: false), ['19 mi', '23 min slower']);
      expect(facts(routes, 2, driving: false), [
        '13 mi',
        '12 min slower',
        'Shortest',
      ]);
    });

    test('in one line, and in the units asked for', () {
      expect(
        routeCaption(routes, 2, driving: true, units: UnitSystem.metric),
        '21 km · 12 min slower · Shortest · No freeways',
      );
    });

    test('a drive whose profile has "Toll" on it doesn\'t say it again', () {
      expect(
        routeFacts(
          routes,
          1,
          driving: true,
          units: UnitSystem.imperial,
          tollOnProfile: true,
        ),
        ['14 mi', '3 min slower'],
      );
      expect(
        routeCaption(
          routes,
          3,
          driving: true,
          units: UnitSystem.imperial,
          tollOnProfile: true,
        ),
        '19 mi · 23 min slower · Ferry · No freeways',
      );
    });
  });

  test('a climb rounds to 10 ft or 5 m, and is nothing if that\'s none', () {
    expect(formatClimb(164.6, units: UnitSystem.imperial), '540 ft');
    expect(formatClimb(1.4, units: UnitSystem.imperial), isNull);
    expect(formatClimb(1.6, units: UnitSystem.imperial), '10 ft');
    expect(formatClimb(1.4, units: UnitSystem.metric), isNull);
    expect(formatClimb(2.6, units: UnitSystem.metric), '5 m');
    expect(formatClimb(163, units: UnitSystem.metric), '165 m');
    expect(formatClimb(0, units: UnitSystem.metric), isNull);
  });

  group('what a ride\'s or a walk\'s row says of its ground', () {
    DirectionsRoute ride(
      double distance,
      double duration, {
      double? climb,
      double trails = 0,
      DirectionsRunKind kind = DirectionsRunKind.trail,
      bool stairs = false,
    }) => fakeRoute(
      const [
        [0, 0],
        [1, 1],
      ],
      distance: distance,
      duration: duration,
      runs: [fakeRun(duration / 60, distance / duration, kind: kind)],
      elevation: climb == null ? null : fakeElevation([0, 1], climb: climb),
      trailShare: trails,
      stairs: stairs,
    );
    List<String> facts(List<DirectionsRoute> routes, int index) =>
        routeFacts(routes, index, driving: false, units: UnitSystem.imperial);

    test('how far it climbs, after how it compares', () {
      expect(facts([ride(25100, 4680, climb: 164.6)], 0), [
        '16 mi',
        'Fastest',
        '540 ft climb',
      ]);
      // In meters where the units are.
      expect(
        routeFacts(
          [ride(25100, 4680, climb: 164.6)],
          0,
          driving: false,
          units: UnitSystem.metric,
        ),
        ['25 km', 'Fastest', '165 m climb'],
      );
    });

    test('nothing of it with no elevation, or none to speak of', () {
      expect(facts([ride(25100, 4680)], 0), ['16 mi', 'Fastest']);
      expect(facts([ride(25100, 4680, climb: 1)], 0), ['16 mi', 'Fastest']);
    });

    test('"Least climbing" with its climb on the one that climbs clearly '
        'least', () {
      final routes = [
        ride(25100, 4680, climb: 164.6),
        ride(26200, 5040, climb: 115.8),
        ride(24300, 5460, climb: 277),
      ];
      expect(facts(routes, 0), ['16 mi', 'Fastest', '540 ft climb']);
      expect(facts(routes, 1), [
        '16 mi',
        '6 min slower',
        'Least climbing, 380 ft',
      ]);
      expect(facts(routes, 2), [
        '15 mi',
        '13 min slower',
        'Shortest',
        '910 ft climb',
      ]);
      // And with none to speak of, without the number.
      expect(
        facts([ride(1000, 600, climb: 30), ride(1000, 700, climb: 0)], 1),
        ['0.6 mi', '2 min slower', 'Least climbing'],
      );
    });

    test('not when it climbs less by under 15 m, or under a fifth', () {
      expect(
        leastClimbing([ride(1, 1, climb: 30), ride(1, 2, climb: 44)]),
        isNull,
      );
      expect(leastClimbing([ride(1, 1, climb: 30), ride(1, 2, climb: 45)]), 0);
      expect(
        leastClimbing([ride(1, 1, climb: 300), ride(1, 2, climb: 370)]),
        isNull,
      );
      expect(
        leastClimbing([ride(1, 1, climb: 300), ride(1, 2, climb: 375)]),
        0,
      );
      // Against every other, not just the next.
      expect(
        leastClimbing([
          ride(1, 1, climb: 300),
          ride(1, 2, climb: 400),
          ride(1, 3, climb: 310),
        ]),
        isNull,
      );
    });

    test('not when it\'s the only one, or another doesn\'t say', () {
      expect(leastClimbing([ride(1, 1, climb: 30)]), isNull);
      expect(leastClimbing([ride(1, 1, climb: 30), ride(1, 2)]), isNull);
    });

    test('"Mostly trails" for a ride over half on them', () {
      expect(facts([ride(25100, 4680, trails: 0.64)], 0), [
        '16 mi',
        'Fastest',
        'Mostly trails',
      ]);
      expect(facts([ride(25100, 4680, trails: 0.5)], 0), ['16 mi', 'Fastest']);
    });

    test('a walk: "Stairs" if it takes any, and nothing of its paths', () {
      final walk = ride(
        2100,
        1680,
        climb: 25,
        trails: 0.9,
        kind: DirectionsRunKind.path,
        stairs: true,
      );
      expect(facts([walk], 0), ['1.3 mi', 'Fastest', '80 ft climb', 'Stairs']);
    });

    test('a drive says none of it', () {
      final drive = ride(
        2100,
        1680,
        climb: 25,
        trails: 0.9,
        kind: DirectionsRunKind.street,
      );
      expect(
        routeFacts([drive], 0, driving: true, units: UnitSystem.imperial),
        ['1.3 mi', 'Fastest'],
      );
    });
  });
}
