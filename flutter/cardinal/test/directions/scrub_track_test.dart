import 'package:cardinal/directions/scrub_track.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/transit_timeline.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_routes.dart';
import '../fakes/fake_transit.dart';

/// Made-up places on a made-up line, a degree of latitude apart.
const _north = [
  [0.0, 0.0],
  [0.0, 1.0],
  [0.0, 2.0],
];

void main() {
  group('a line', () {
    final line = LineTrack([
      for (final p in _north) DirectionsPoint(lat: p[1], lng: p[0]),
    ]);

    test('is measured along its points', () {
      expect(line.length, moreOrLessEquals(2 * 111195, epsilon: 1));
    });

    test('has a point at a distance along it, kept to its ends', () {
      expect(line.pointAt(111195 * 1.5)!.lat, moreOrLessEquals(1.5));
      expect(line.pointAt(-10)!.lat, 0);
      expect(line.pointAt(1e9)!.lat, 2);
      expect(line.pointAtShare(0.25)!.lat, moreOrLessEquals(0.5));
      expect(LineTrack(const []).pointAt(5), isNull);
    });

    test('snaps a point beside it onto it, no further back than asked', () {
      final beside = const DirectionsPoint(lat: 1.2, lng: 0.01);
      expect(line.snap(beside) / 111195, moreOrLessEquals(1.2, epsilon: 1e-3));
      // A line that comes back past the same place: the later pass, from there on.
      final there = LineTrack(const [
        DirectionsPoint(lat: 0, lng: 0),
        DirectionsPoint(lat: 1, lng: 0),
        DirectionsPoint(lat: 0, lng: 0.01),
      ]);
      const near = DirectionsPoint(lat: 0.5, lng: 0.001);
      final first = there.snap(near);
      expect(first / 111195, moreOrLessEquals(0.5, epsilon: 1e-2));
      expect(
        there.snap(near, from: 111195) / 111195,
        moreOrLessEquals(1.5, epsilon: 1e-2),
      );
    });
  });

  group('a street route', () {
    // 22 minutes of runs: 3 at 8 m/s, 15 at 27 m/s, 4 at 12 m/s.
    final runs = [fakeRun(3, 8), fakeRun(15, 27), fakeRun(4, 12)];
    final runsDistance = runs.fold(0.0, (sum, r) => sum + r.distance);
    final route = fakeRoute(
      _north,
      distance: 28000,
      duration: 1320,
      runs: runs,
      steps: const [
        DirectionsStep(
          instruction: 'Drive north.',
          distance: 1400,
          duration: 180,
          roadName: 'Elm Street',
        ),
        DirectionsStep(
          instruction: 'Take the freeway.',
          distance: 24000,
          duration: 900,
          roadName: 'I-5 North',
        ),
        DirectionsStep(instruction: 'Keep on.', distance: 2600, duration: 240),
        DirectionsStep(instruction: 'Arrive.', distance: 0, duration: 0),
      ],
    );

    test('is along its runs by the time across its graph', () {
      expect(streetSpotAt(route, 0).meters, 0);
      // Halfway through the second run.
      final spot = streetSpotAt(route, 10.5 / 22);
      final gone = 3 * 60 * 8 + 7.5 * 60 * 27;
      expect(spot.share, moreOrLessEquals(gone / runsDistance));
      expect(spot.meters, moreOrLessEquals(gone / runsDistance * 28000));
      expect(spot.run, same(runs[1]));
      expect(streetSpotAt(route, 1).share, 1);
      expect(streetSpotAt(route, 1.5).share, 1, reason: 'kept to the end');
    });

    test(
      'names the road of the step it is in, and none for a step without',
      () {
        expect(streetSpotAt(route, 0.01).road, 'Elm Street');
        expect(streetSpotAt(route, 0.5).road, 'I-5 North');
        expect(streetSpotAt(route, 0.995).road, isNull);
      },
    );

    test('with no runs, is along its distance by the share of its graph', () {
      final plain = fakeRoute(_north, distance: 2000);
      final spot = streetSpotAt(plain, 0.25);
      expect(spot.share, 0.25);
      expect(spot.meters, 500);
      expect(spot.run, isNull);
    });

    test('is on its line by its share of the distance', () {
      final point = streetPointAt(route, 10.5 / 22)!;
      final gone = (3 * 60 * 8 + 7.5 * 60 * 27) / runsDistance;
      expect(point.lat, moreOrLessEquals(2 * gone, epsilon: 1e-6));
      expect(streetPointAt(fakeRoute(_north), 0.5)!.lat, moreOrLessEquals(1));
    });

    test('reads out for a drive: how far, the road and the speed', () {
      expect(
        streetReadout(
          route,
          10.5 / 22,
          driving: true,
          units: UnitSystem.imperial,
        ),
        ['Mile 8.3', 'I-5 North', '60 mph'],
      );
      expect(
        streetReadout(
          route,
          10.5 / 22,
          driving: true,
          units: UnitSystem.metric,
        ),
        ['km 13.3', 'I-5 North', '97 km/h'],
      );
      // No road there.
      expect(
        streetReadout(route, 0.995, driving: true, units: UnitSystem.metric),
        ['km 27.9', '43 km/h'],
      );
      // No runs: no speed.
      expect(
        streetReadout(
          fakeRoute(_north, distance: 2000),
          0.5,
          driving: true,
          units: UnitSystem.metric,
        ),
        ['km 1.0'],
      );
    });

    test(
      'reads out for a ride or a walk: how far, the height and the grade',
      () {
        // 30 m apart: level for 300 m, up 30 m over the next 300 m, then down.
        final heights = [
          for (var i = 0; i <= 10; i++) 100.0,
          for (var i = 1; i <= 10; i++) 100.0 + 3 * i,
          for (var i = 1; i <= 10; i++) 130.0 - 6 * i,
        ];
        final ride = fakeRoute(
          _north,
          distance: 900,
          duration: 300,
          elevation: fakeElevation(heights),
        );
        expect(
          streetReadout(ride, 0.1, driving: false, units: UnitSystem.imperial),
          ['Mile 0.1', '330 ft', 'level'],
        );
        expect(
          streetReadout(ride, 0.5, driving: false, units: UnitSystem.metric),
          ['km 0.5', '115 m', '10% uphill'],
        );
        expect(
          streetReadout(ride, 0.85, driving: false, units: UnitSystem.metric),
          ['km 0.8', '95 m', '20% downhill'],
        );
        // No elevation: how far alone.
        expect(
          streetReadout(
            fakeRoute(_north, distance: 900),
            0.5,
            driving: false,
            units: UnitSystem.imperial,
          ),
          ['Mile 0.3'],
        );
      },
    );
  });

  group('the readout\'s words', () {
    test('a milepost, in tenths and whole from 100', () {
      expect(formatMilepost(0, units: UnitSystem.imperial), 'Mile 0.0');
      expect(formatMilepost(9978, units: UnitSystem.imperial), 'Mile 6.2');
      expect(formatMilepost(180000, units: UnitSystem.imperial), 'Mile 112');
      expect(formatMilepost(4150, units: UnitSystem.metric), 'km 4.2');
    });

    test('a speed', () {
      expect(formatSpeed(26, units: UnitSystem.imperial), '58 mph');
      expect(formatSpeed(26, units: UnitSystem.metric), '94 km/h');
    });

    test('a height, as climbs are rounded', () {
      expect(formatHeight(100.6, units: UnitSystem.imperial), '330 ft');
      expect(formatHeight(101, units: UnitSystem.metric), '100 m');
    });

    test('a grade, level within a percent', () {
      expect(formatGrade(0.061), '6% uphill');
      expect(formatGrade(-0.03), '3% downhill');
      expect(formatGrade(0.009), 'level');
      expect(formatGrade(-0.004), 'level');
    });
  });

  group('a journey', () {
    final t = fakeStart;
    // A walk of 5 minutes to Elm Street, a wait of 3, a ride of 12 past Oak Street to
    // Market Square, standing a minute at Oak Street; and a walk of 4 minutes.
    final journey = _journey(t);
    final ride = journey.legs[1] as TransitLeg;
    DateTime at(int seconds) =>
        DateTime.fromMillisecondsSinceEpoch((t + seconds) * 1000);

    test('says what the rider is doing at a time', () {
      expect(journeyActivity(journey, at(60)), 'walking');
      expect(journeyActivity(journey, at(400)), 'waiting at Elm Street');
      expect(journeyActivity(journey, at(600)), 'on 9 Line');
      expect(journeyActivity(journey, at(1000)), 'on 9 Line');
      expect(journeyActivity(journey, at(1400)), 'walking');
    });

    test('a ride with no short name goes by its long one, and a ride on the '
        'streets by bike is cycling', () {
      final other = _journey(t, shortName: null, mode: 'cycling');
      expect(journeyActivity(other, at(60)), 'cycling');
      expect(journeyActivity(other, at(700)), 'on Harbor Line');
    });

    test('walks along its walk by time', () {
      // Halfway through the walk, which goes 0.1° north.
      expect(journeyPointAt(journey, at(150))!.lat, moreOrLessEquals(0.05));
    });

    test('waits at the stop it boards at', () {
      final point = journeyPointAt(journey, at(450))!;
      expect(point.lat, moreOrLessEquals(0.1));
      expect(point.lng, moreOrLessEquals(0));
    });

    test('rides between stops by their times, along the line', () {
      // Elm Street (0.1) to Oak Street (0.5) from 8 to 14 minutes: halfway at 11.
      final point = journeyPointAt(journey, at(660))!;
      expect(point.lat, moreOrLessEquals(0.3, epsilon: 1e-6));
      // The line bends east between them; the rider follows it.
      expect(point.lng, greaterThan(0));
      // Standing at Oak Street for its minute.
      expect(journeyPointAt(journey, at(870))!.lat, moreOrLessEquals(0.5));
      expect(ride.stops[1].stop.name, 'Oak Street');
    });

    test('a ride without a line goes straight from stop to stop', () {
      final straight = _journey(t, shape: false);
      final point = journeyPointAt(straight, at(660))!;
      expect(point.lat, moreOrLessEquals(0.3, epsilon: 1e-6));
      expect(point.lng, moreOrLessEquals(0, epsilon: 1e-9));
    });

    test('is at the end once it has arrived', () {
      expect(journeyPointAt(journey, at(1620))!.lat, moreOrLessEquals(1.0));
    });

    test('is at a time within the piece of its timeline at a point', () {
      final axis = TimelineAxis(at(0), at(1620));
      final placed = placePieces(timelinePieces(journey), axis, 324);
      // 324 px for 27 minutes: 12 px a minute where nothing is widened.
      expect(timelineTimeAt(placed, -5), at(0));
      expect(timelineTimeAt(placed, 400), at(1620));
      final walk = placed.first;
      expect(timelineTimeAt(placed, walk.left + walk.width / 2), at(150));
    });
  });

  group('panning to a point', () {
    const square = [
      (lng: 0.0, lat: 0.0),
      (lng: 1.0, lat: 0.0),
      (lng: 1.0, lat: 1.0),
      (lng: 0.0, lat: 1.0),
    ];

    test('inside, there is nothing to do', () {
      expect(panInto(square, 0.5, 0.5), isNull);
    });

    test('outside, moves only as far as the nearest edge', () {
      final shift = panInto(square, 1.2, 0.5)!;
      expect(shift.lng, moreOrLessEquals(0.2));
      expect(shift.lat, moreOrLessEquals(0));
      final corner = panInto(square, -0.1, 1.3)!;
      expect(corner.lng, moreOrLessEquals(-0.1));
      expect(corner.lat, moreOrLessEquals(0.3));
    });
  });
}

Map<String, Object?> _stop(
  String name,
  double lat,
  int arrival,
  int departure,
) => {
  'stop_id': name,
  'name': name,
  'latitude': lat,
  'longitude': 0.0,
  'arrival': arrival,
  'departure': departure,
};

/// A made-up journey from [t] (see the tests).
Journey _journey(
  int t, {
  String? shortName = '9 Line',
  String mode = 'walking',
  bool shape = true,
}) => TransitPlan.fromJson({
  'journeys': [
    {
      'departure': t,
      'arrival': t + 1620,
      'duration_seconds': 1620,
      'trips': 1,
      'legs': [
        {
          'type': 'access',
          'mode': mode,
          'duration_seconds': 300,
          'departure': t,
          'arrival': t + 300,
          'stop': _stop('Elm Street', 0.1, t + 300, t + 300),
          'walk': {
            'distance_meters': 11120.0,
            'shape': polyline6([
              [0, 0],
              [0, 0.1],
            ]),
          },
        },
        {
          'type': 'transit',
          'departure': t + 480,
          'arrival': t + 1200,
          'duration_seconds': 720,
          'route': {
            'route_id': 'r9',
            'short_name': ?shortName,
            'long_name': 'Harbor Line',
            'route_type': 0,
          },
          'stops': [
            _stop('Elm Street', 0.1, t + 480, t + 480),
            _stop('Oak Street', 0.5, t + 840, t + 900),
            _stop('Market Square', 0.8, t + 1200, t + 1200),
          ],
          if (shape)
            'shape': polyline6([
              [0, 0.1],
              [0.05, 0.3],
              [0, 0.5],
              [0, 0.8],
            ]),
        },
        {
          'type': 'egress',
          'mode': 'walking',
          'duration_seconds': 240,
          'departure': t + 1200,
          'arrival': t + 1440,
          'stop': _stop('Market Square', 0.8, t + 1200, t + 1200),
          'walk': {
            'distance_meters': 22240.0,
            'shape': polyline6([
              [0, 0.8],
              [0, 1.0],
            ]),
          },
        },
      ],
    },
  ],
  'realtime': {'level': 'schedule', 'feeds': <Object>[]},
}).journeys.single;
