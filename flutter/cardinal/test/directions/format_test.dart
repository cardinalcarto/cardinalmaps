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

    test('one that isn\'t slower, though it isn\'t first, says neither', () {
      expect(facts([freeway, _route(30000, 1200, motorway: true)], 1), [
        '19 mi',
      ]);
    });

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
  });
}
