import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_transit.dart';

DateTime at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

/// The trips taken, ride by ride, or null where a ride isn't made.
List<String?> trips(ConnectionChain chain) => [
  for (final r in chain.rides) r.made ? r.departure?.tripId : null,
];

/// The index of [trip] among ride [ride]'s departures.
int indexOf(ConnectionChain chain, int ride, String trip) =>
    chain.rides[ride].ride.departures.indexWhere((d) => d.tripId == trip);

ConnectionChain select(ConnectionChain chain, int ride, String trip) =>
    chain.select(ride, indexOf(chain, ride, trip));

void main() {
  test('reads departures of other routes, and the walk between rides', () {
    final journey = connectionsJourney();
    final rides = journey.rides.toList();
    expect(rides.first.transferSeconds, isNull);
    expect(rides.skip(1).map((r) => r.transferSeconds), [180, 60, 120]);
    final departures = rides.first.departures;
    // Its own route's departures don't name it; another's does.
    expect(departures[2].route, isNull);
    expect(departures[3].route!.shortName, '7');
    expect(departures[3].route!.color, '8E3A9C');
    expect(departures[3].headsign, 'Rainier Beach');
    expect(departures[3].startDate, '20261109');
  });

  group('the chain', () {
    test('starts with each ride\'s suggested departure', () {
      final chain = ConnectionChain(connectionsJourney());
      expect(trips(chain), ['40-1', 'link-1', '8-1', '2-1']);
      expect(chain.suggested, isTrue);
      expect(chain.rides.any((r) => r.chosen), isFalse);
      expect(chain.departure, at(0));
      expect(chain.arrival, at(3720));
      expect(chain.duration, 3720);
      expect(chain.brokenAt, isNull);
      // When the rider can be at each stop: the arrival before, plus the walk.
      expect(chain.rides.map((r) => r.ready), [
        null,
        at(1200 + 180),
        at(2100 + 60),
        at(2880 + 120),
      ]);
      expect(chain.rides.map((r) => r.waitSeconds), [300, 120, 120, 60]);
    });

    test('the first ride\'s catchable departures are the plan\'s', () {
      final first = ConnectionChain(connectionsJourney()).rides.first;
      // Missed, missed, own, 7, 40, 7, 31, canceled.
      expect(first.catchable, [
        false,
        false,
        true,
        true,
        true,
        true,
        true,
        false,
      ]);
    });

    test('a later ride\'s are those that leave once the rider is there', () {
      final chain = ConnectionChain(connectionsJourney());
      // Link at 17:50 leaves before the rider is there at 17:53.
      expect(chain.rides[1].catchable, [false, true, true, true]);
    });

    test('a later first ride makes the next connections later', () {
      final chain = select(ConnectionChain(connectionsJourney()), 0, '40-2');
      expect(trips(chain), ['40-2', 'link-3', '8-2', '2-2']);
      expect(chain.rides.map((r) => r.chosen), [true, false, false, false]);
      expect(chain.rides[1].ready, at(2100 + 180));
      expect(chain.rides[1].catchable, [false, false, false, true]);
      expect(chain.rides[1].waitSeconds, 2460 - 2280);
      // Leaving as much later as the first ride does.
      expect(chain.departure, at(900));
      expect(chain.arrival, at(4440 + 180));
      expect(chain.duration, 4620 - 900);
      expect(chain.suggested, isFalse);
    });

    test('a ride after keeps its suggested departure while it can be made', () {
      // The 7 at 17:43 reaches B at 17:52: the rider is at C at 17:55, as Link leaves.
      final chain = select(ConnectionChain(connectionsJourney()), 0, '7-1');
      expect(chain.rides[1].ready, at(1500));
      expect(trips(chain), ['7-1', 'link-1', '8-1', '2-1']);
      expect(chain.rides[1].waitSeconds, 0);
      expect(chain.rides.first.route!.shortName, '7');
      expect(chain.rides.first.headsign, 'Rainier Beach');
      expect(chain.arrival, at(3720));
    });

    test('a ride chosen by the rider stays while it can be made', () {
      var chain = ConnectionChain(connectionsJourney());
      chain = select(chain, 2, '9-1');
      expect(trips(chain), ['40-1', 'link-1', '9-1', '2-2']);
      // A later first ride that still makes the 9 keeps it.
      chain = select(chain, 0, '7-1');
      expect(trips(chain), ['7-1', 'link-1', '9-1', '2-2']);
      expect(chain.rides[2].chosen, isTrue);
      // One that can't drops it for the first that can be made.
      chain = select(chain, 0, '40-2');
      expect(trips(chain), ['40-2', 'link-3', '8-2', '2-2']);
      expect(chain.rides[2].chosen, isFalse);
      // And back again, the suggested ones follow.
      chain = select(chain, 0, '40-1');
      expect(trips(chain), ['40-1', 'link-1', '8-1', '2-1']);
    });

    test('the first ride that can be made is any route\'s', () {
      // From Link's 18:03, at D Bay 2 at 18:14: past the 8 and the 48, the 9 at 18:15,
      // before the next 8.
      var chain = select(ConnectionChain(connectionsJourney()), 1, 'link-2');
      expect(trips(chain).sublist(2), ['9-1', '2-2']);
      // From Link's 18:11, at 18:22: the 8 at 18:23.
      chain = select(ConnectionChain(connectionsJourney()), 1, 'link-3');
      expect(trips(chain).sublist(2), ['8-2', '2-2']);
    });

    test('a departure that can\'t be made can\'t be chosen', () {
      final chain = ConnectionChain(connectionsJourney());
      // Missed, canceled, and on a later ride, leaving before the rider is there.
      expect(identical(select(chain, 0, '40-0'), chain), isTrue);
      expect(identical(select(chain, 0, '40-3'), chain), isTrue);
      expect(identical(select(chain, 1, 'link-0'), chain), isTrue);
    });

    test('an earlier departure can be chosen if it can be made', () {
      final chain = select(
        select(ConnectionChain(connectionsJourney()), 0, '40-2'),
        1,
        'link-3',
      );
      // Back to the suggested first ride: Link at 17:55 can be made again.
      final back = select(chain, 0, '40-1');
      expect(trips(back), ['40-1', 'link-3', '8-2', '2-2']);
      final earlier = select(back, 1, 'link-1');
      expect(trips(earlier), ['40-1', 'link-1', '8-1', '2-1']);
    });

    test('a ride none of whose departures can be made has no connection', () {
      final chain = select(
        ConnectionChain(connectionsJourney(lateLastRide: false)),
        0,
        '40-2',
      );
      expect(chain.brokenAt, 3);
      expect(chain.rides.last.connection, RideConnection.none);
      expect(chain.rides.last.ready, at(3780 + 120));
      expect(chain.rides.last.catchable, [false]);
      expect(chain.rides.last.departureTime, isNull);
      expect(chain.arrival, isNull);
      expect(chain.duration, isNull);
    });

    test('the rides after one with no connection are unknown', () {
      // The 8 at 18:23 reaches E at 18:33: no 2 after it.
      var chain = ConnectionChain(connectionsJourney(lateLastRide: false));
      chain = select(chain, 2, '8-2');
      expect(chain.brokenAt, 3);
      // Two rides earlier: the 40 at 17:55 has no Link it can make at all.
      final json = connectionsPlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      ((legs[3] as Map)['departures']! as List).removeLast();
      chain = select(
        ConnectionChain(TransitPlan.fromJson(json).journeys.single),
        0,
        '40-2',
      );
      expect(chain.brokenAt, 1);
      expect(chain.rides.map((r) => r.connection), [
        RideConnection.made,
        RideConnection.none,
        RideConnection.unknown,
        RideConnection.unknown,
      ]);
      expect(chain.rides[2].ready, isNull);
      expect(chain.rides[2].catchable, everyElement(isFalse));
    });

    test('a newer plan keeps the choices it still lists', () {
      var chain = select(ConnectionChain(connectionsJourney()), 0, '40-2');
      chain = select(chain, 1, 'link-3');
      final rebased = chain.rebase(connectionsJourney());
      expect(trips(rebased), ['40-2', 'link-3', '8-2', '2-2']);
      expect(rebased.rides.take(2).map((r) => r.chosen), [true, true]);
      // A plan without the chosen 40: back to the suggested one.
      final json = connectionsPlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      ((legs[1] as Map)['departures']! as List).removeAt(5);
      final without = chain.rebase(TransitPlan.fromJson(json).journeys.single);
      expect(trips(without), ['40-1', 'link-3', '8-2', '2-2']);
      expect(without.rides.map((r) => r.chosen), [false, true, false, false]);
    });

    test('a ride without listed departures is made if it leaves in time', () {
      final json = connectionsPlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      ((legs[3] as Map)['departures']! as List).clear();
      var chain = ConnectionChain(TransitPlan.fromJson(json).journeys.single);
      expect(chain.rides[1].made, isTrue);
      expect(chain.rides[1].departure, isNull);
      expect(chain.rides[1].departureTime, at(1500));
      chain = select(chain, 0, '40-2');
      expect(chain.brokenAt, 1);
    });

    test('without the walk between rides, the plan\'s misses stand', () {
      final json = connectionsPlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      (legs[3] as Map).remove('transfer_seconds');
      final chain = ConnectionChain(TransitPlan.fromJson(json).journeys.single);
      expect(chain.rides[1].ready, isNull);
      expect(chain.rides[1].catchable, [false, true, true, true]);
    });
  });

  group('other routes', () {
    test('up to two that can be made, near the suggested one, each once', () {
      final others = otherRoutes(connectionsJourney());
      String names(List<TransitRoute> routes) =>
          routes.map((r) => r.shortName).join(' ');
      // The 62 is missed, the second 7 is a repeat, and the 31 leaves 20 minutes later.
      expect(others.map(names), ['7', '', '48 9', '']);
    });

    test('a third is left out, and one more than 15 minutes off', () {
      final json = connectionsPlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      final departures = (legs[5] as Map)['departures']! as List;
      // A 2 at 18:12, between the 48 and the 9.
      departures.insert(
        2,
        Map.of(departures[1] as Map<String, Object?>)
          ..['trip_id'] = '2-x'
          ..['departure'] = fakeStart + 2520
          ..['route'] = fakeRoutes['2'],
      );
      final others = otherRoutes(TransitPlan.fromJson(json).journeys.single);
      expect(others[2].map((r) => r.shortName), ['48', '2']);
      expect(
        otherRoutes(
          connectionsJourney(),
          within: const Duration(minutes: 5),
        )[2].map((r) => r.shortName),
        ['48'],
      );
    });
  });
}
