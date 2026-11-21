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
    chain.rides[ride].departures.indexWhere((d) => d.tripId == trip);

ConnectionChain select(ConnectionChain chain, int ride, String trip) =>
    chain.select(ride, indexOf(chain, ride, trip));

/// [fakePlanJson]'s journey, with [tag], as a plan would have it on its 40 twelve
/// minutes later (`t40c`), each ride on one trip, and the same Link.
Map<String, Object?> laterJourneyJson(String tag) {
  final journey =
      (fakePlanJson(tags: [tag])['journeys']! as List).single
          as Map<String, Object?>;
  final bus = (journey['legs']! as List)[1] as Map<String, Object?>;
  (bus['trip']! as Map)['trip_id'] = 't40c$tag';
  bus['segments'] = <Object>[];
  for (final d in (bus['departures']! as List).cast<Map<String, Object?>>()) {
    if (d['trip_id'] == 't40c$tag') {
      d['suggested'] = true;
    } else {
      d.remove('suggested');
    }
  }
  return journey;
}

/// A plan of [journeys], as [fakePlanJson] has them.
Journey journeyOf(Map<String, Object?> journey) => TransitPlan.fromJson({
  ...fakePlanJson(),
  'journeys': [journey],
}).journeys.single;

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

    test('an earlier ride makes the next ride the one arriving first', () {
      // The 62 at 17:32, were it catchable, has the rider at C at 17:44: Link's 17:50
      // arrives before the suggested 17:55, so the rider doesn't wait for the plan's.
      final json = connectionsPlanJson();
      final legs =
          ((json['journeys']! as List).single as Map<String, Object?>)['legs']!
              as List;
      final first = legs.cast<Map<String, Object?>>().firstWhere(
        (l) => l['type'] == 'transit',
      );
      for (final d
          in (first['departures']! as List).cast<Map<String, Object?>>()) {
        if (d['trip_id'] == '62-0') d.remove('missed');
      }
      final journey = TransitPlan.fromJson(json).journeys.single;
      final chain = select(ConnectionChain(journey), 0, '62-0');
      expect(trips(chain), ['62-0', 'link-0', '8-1', '2-1']);
      expect(chain.rides[1].chosen, isFalse);
      // Back on the suggested first ride, the suggested ones follow again.
      expect(trips(select(chain, 0, '40-1')), ['40-1', 'link-1', '8-1', '2-1']);
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

  group('by the clock', () {
    // The walk to Stop A takes 5 minutes: the 40 at 17:40 can be made until 17:35.
    test('the first ride\'s departures are made from when the rider can be at '
        'its stop, setting out now, and the plan\'s misses stand', () {
      final chain = ConnectionChain(connectionsJourney(), now: at(0));
      expect(trips(chain), ['40-1', 'link-1', '8-1', '2-1']);
      expect(chain.rides.first.catchable, [
        false,
        false,
        true,
        true,
        true,
        true,
        true,
        false,
      ]);
      // As without the time: the first ride's wait is the plan's.
      expect(chain.rides.first.ready, isNull);
      expect(chain.rides.first.waitSeconds, 300);
      expect(chain.missesAt, at(300).add(const Duration(microseconds: 1)));
      expect(ConnectionChain(connectionsJourney()).missesAt, isNull);
    });

    test('the suggested one turns missed, and the journey re-times onto the '
        'one that arrives first', () {
      final chain = ConnectionChain(connectionsJourney(), now: at(0));
      expect(trips(chain.at(at(300))), ['40-1', 'link-1', '8-1', '2-1']);
      final later = chain.at(at(301));
      expect(later.rides.first.catchable[2], isFalse);
      expect(trips(later), ['7-1', 'link-1', '8-1', '2-1']);
      expect(later.rides.first.chosen, isFalse);
      expect(later.suggested, isFalse);
      expect(later.departure, at(180));
      expect(later.rides[1].ready, at(1320 + 180));
      expect(later.missesAt, at(480).add(const Duration(microseconds: 1)));
      expect(later.now, at(301));
    });

    test('a later departure chosen stays as the suggested one leaves', () {
      final chain = select(
        ConnectionChain(connectionsJourney(), now: at(0)),
        0,
        '40-2',
      );
      final later = chain.at(at(301));
      expect(trips(later), trips(chain));
      expect(later.rides.first.chosen, isTrue);
    });

    test('a departure chosen that the clock passes gives way, as one that '
        'can\'t be made does', () {
      final chain = select(
        ConnectionChain(connectionsJourney(), now: at(0)),
        0,
        '7-1',
      );
      final later = chain.at(at(481));
      expect(later.rides.first.catchable[3], isFalse);
      expect(trips(later).first, '7-2');
      expect(later.rides.first.chosen, isFalse);
      // And the choice isn't made again as time goes on.
      expect(later.at(at(482)).rides.first.chosen, isFalse);
    });

    test('with every departure gone, the first ride has no connection and the '
        'rest is unknown', () {
      final chain = ConnectionChain(connectionsJourney(), now: at(0));
      expect(chain.at(at(1500)).gone, isFalse);
      final gone = chain.at(at(1501));
      expect(gone.gone, isTrue);
      expect(gone.rides.first.catchable, everyElement(isFalse));
      expect(gone.rides.map((r) => r.connection), [
        RideConnection.none,
        RideConnection.unknown,
        RideConnection.unknown,
        RideConnection.unknown,
      ]);
      expect(gone.arrival, isNull);
      expect(gone.missesAt, isNull);
      // The last that runs left at 18:00, the canceled 40 after it not counting.
      expect(gone.rides.first.lastRunning?.tripId, '31-1');
    });

    test('a ride without listed departures is missed when it leaves', () {
      final json = connectionsPlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      ((legs[1] as Map)['departures']! as List).clear();
      final chain = ConnectionChain(
        TransitPlan.fromJson(json).journeys.single,
        now: at(0),
      );
      expect(chain.missesAt, at(300).add(const Duration(microseconds: 1)));
      expect(chain.at(at(300)).gone, isFalse);
      expect(chain.at(at(301)).gone, isTrue);
    });

    test('a choice and a newer plan keep the time', () {
      final chain = ConnectionChain(connectionsJourney(), now: at(301));
      expect(select(chain, 0, '40-2').now, at(301));
      expect(trips(chain.rebase(connectionsJourney())).first, '7-1');
    });

    test('compares as choosing the same while the clock changes nothing', () {
      final chain = ConnectionChain(connectionsJourney(), now: at(0));
      expect(chain.at(at(200)).choosesAs(chain), isTrue);
      expect(chain.at(at(301)).choosesAs(chain), isFalse);
    });
  });

  group('a newer plan without the journey', () {
    /// A plan of one journey whose first ride is the 62 between the same two stops
    /// as [connectionsJourney]'s 40, whose departures list the 40 at 17:40, its own
    /// 62 at 17:43, and the 40 at 17:55 a minute late.
    List<Journey> another() {
      final json = connectionsPlanJson();
      final legs = ((json['journeys']! as List).single as Map)['legs']! as List;
      final ride = legs[1] as Map<String, Object?>;
      ride['route'] = fakeRoutes['62'];
      ride['trip'] = {
        'trip_id': '62-9',
        'start_date': '20261109',
        'headsign': 'Georgetown',
      };
      final departures = (ride['departures']! as List)
          .cast<Map<String, Object?>>();
      final forty = departures[2]
        ..remove('suggested')
        ..['route'] = fakeRoutes['40']
        ..['headsign'] = 'Downtown Seattle';
      final own = {
        ...departures[3],
        'trip_id': '62-9',
        'suggested': true,
        'route': null,
        'headsign': null,
      }..removeWhere((_, v) => v == null);
      final late = departures[5]
        ..['route'] = fakeRoutes['40']
        ..['departure'] = fakeStart + 1560
        ..['arrival'] = fakeStart + 2160
        ..['delay_seconds'] = 60;
      ride['departures'] = [forty, own, late];
      // Its other rides are between other stops.
      for (final i in const [3, 5, 7]) {
        final stops = ((legs[i] as Map)['stops']! as List)
            .cast<Map<String, Object?>>();
        stops.first['stop_id'] = 'elsewhere';
      }
      return TransitPlan.fromJson(json).journeys;
    }

    test('takes each ride\'s departures from a ride of it between the same '
        'stops, as this ride\'s list would have them', () {
      final chain = ConnectionChain(connectionsJourney());
      final refreshed = chain.refresh(another());
      expect(identical(refreshed.journey, chain.journey), isTrue);
      final first = refreshed.rides.first.departures;
      expect(first.map((d) => d.tripId), ['40-1', '62-9', '40-2']);
      // The 40 is this ride's own route and run; the 62 another's.
      expect(first[0].route, isNull);
      expect(first[0].suggested, isTrue);
      expect(first[0].headsign, isNull);
      expect(first[1].route?.shortName, '62');
      expect(first[1].suggested, isFalse);
      expect(first[1].headsign, 'Georgetown');
      expect(first[1].startDate, '20261109');
      expect(first.every((d) => !d.missed), isTrue);
      expect(trips(refreshed).first, '40-1');
      expect(refreshed.rides.first.suggested, isTrue);
      // Where no ride of it is between the same stops, as they were.
      for (final i in const [1, 2, 3]) {
        expect(
          identical(refreshed.rides[i].departures, chain.rides[i].departures),
          isTrue,
        );
      }
    });

    test('keeps a departure chosen that it still lists, at its new times', () {
      final chain = select(ConnectionChain(connectionsJourney()), 0, '40-2');
      final refreshed = chain.refresh(another());
      expect(refreshed.rides.first.departure?.tripId, '40-2');
      expect(refreshed.rides.first.chosen, isTrue);
      expect(refreshed.rides.first.departureTime, at(1560));
      // And one it doesn't list gives way.
      final gone = select(ConnectionChain(connectionsJourney()), 0, '31-1');
      expect(gone.refresh(another()).rides.first.chosen, isFalse);
    });

    test('a later plan that has the journey again has its own departures', () {
      final chain = ConnectionChain(connectionsJourney()).refresh(another());
      final back = connectionsJourney();
      final rebased = chain.rebase(back);
      expect(
        identical(rebased.rides.first.departures, back.rides.first.departures),
        isTrue,
      );
    });
  });

  group('departures looked up between a ride\'s stops', () {
    // The first ride of [connectionsJourney] is a 40 from Stop A to Stop B, its own
    // run `40-1` at 17:40.
    List<RideDeparture> looked({bool own = true}) => [
      ...fakeDepartures([
        if (own) ('40-1', 600),
        ('40-2', 1560),
        ('40-9', 2000),
      ], ride: 600),
      ...fakeDepartures(
        [('7-1', 780)],
        ride: 540,
        route: fakeRoutes['7'],
        headsign: 'Rainier Beach',
      ),
    ]..sort((a, b) => a.departure.compareTo(b.departure));

    test('are the ride\'s, read as its own list would say them', () {
      final chain = ConnectionChain(connectionsJourney());
      final relisted = chain.relist({0: looked()});
      expect(identical(relisted.journey, chain.journey), isTrue);
      final first = relisted.rides.first;
      expect(first.departures.map((d) => d.tripId), [
        '40-1',
        '7-1',
        '40-2',
        '40-9',
      ]);
      // The ride's own run is the suggested one, and says nothing the ride does.
      expect(first.departures.map((d) => d.suggested), [
        true,
        false,
        false,
        false,
      ]);
      expect(first.departures.map((d) => d.route?.shortName), [
        null,
        '7',
        null,
        null,
      ]);
      expect(first.departures.map((d) => d.headsign), [
        null,
        'Rainier Beach',
        null,
        null,
      ]);
      expect(first.departures.every((d) => !d.missed), isTrue);
      expect(first.suggested, isTrue);
      // The other rides' are as they were.
      for (var i = 1; i < chain.rides.length; i++) {
        expect(
          identical(relisted.rides[i].departures, chain.rides[i].departures),
          isTrue,
        );
      }
    });

    test('keep a choice they list, and move one they don\'t', () {
      final chain = select(ConnectionChain(connectionsJourney()), 0, '40-2');
      final relisted = chain.relist({0: looked()});
      expect(relisted.rides.first.departure?.tripId, '40-2');
      expect(relisted.rides.first.chosen, isTrue);
      expect(relisted.rides.first.departureTime, at(1560));
      final seven = select(ConnectionChain(connectionsJourney()), 0, '7-2');
      final moved = seven.relist({0: looked()});
      expect(moved.rides.first.chosen, isFalse);
      expect(moved.rides.first.made, isTrue);
    });

    test('bring a first ride whose departures were all gone back, on the one '
        'that can be made and arrives first', () {
      // Five minutes to the stop, and the last listed leaves at 18:00.
      final gone = ConnectionChain(connectionsJourney(), now: at(2000));
      expect(gone.gone, isTrue);
      expect(gone.rides[1].connection, RideConnection.unknown);
      final back = gone.relist({
        0: fakeDepartures([
          ('40-8', 1900),
          ('40-9', 2400),
          ('40-10', 3000),
        ], ride: 600),
      });
      expect(back.gone, isFalse);
      expect(back.rides.first.catchable, [false, true, true]);
      expect(back.rides.first.departure?.tripId, '40-9');
      expect(back.rides.first.chosen, isFalse);
      // As long before it as the plan leaves before its own.
      expect(back.departure, at(1800));
      // The rides after are known again, as far as their own lists reach.
      expect(back.rides[1].connection, isNot(RideConnection.unknown));
    });

    test('stay, under a newer plan without the journey, where a ride of that '
        'plan between the same stops would lend its own', () {
      final chain = ConnectionChain(connectionsJourney());
      final relisted = chain.relist({0: looked(own: false)});
      final other = [connectionsJourney(lateLastRide: false)];
      final kept = relisted.refresh(other, keep: {0});
      expect(
        identical(kept.rides[0].departures, relisted.rides[0].departures),
        isTrue,
      );
      // A ride not looked up takes what the plan lends.
      expect(kept.rides[3].departures.map((d) => d.tripId), ['2-1']);
      // Without it, the plan's list would take its place.
      final lent = relisted.refresh(other);
      expect(lent.rides[0].departures.first.tripId, '40-0');
    });
  });

  group('the journey of the departures taken', () {
    final own = fakePlan(tags: ['-x']).journeys.single;
    final later = journeyOf(laterJourneyJson('-x'));

    test('is the journey itself, as the plan has it', () {
      expect(ConnectionChain(own).takes(own), isTrue);
      expect(ConnectionChain(own).takes(later), isFalse);
    });

    test('is another of the same rides on the trips the rider takes', () {
      final chain = select(ConnectionChain(own), 0, 't40c-x');
      expect(trips(chain), ['t40c-x', 'link-x']);
      expect(chain.takes(later), isTrue);
      expect(chain.takes(own), isFalse);
      // The clock taking it does as well as a choice.
      expect(ConnectionChain(own, now: at(301)).takes(later), isTrue);
    });

    test('is on the same service day', () {
      final json = laterJourneyJson('-x');
      final bus = (json['legs']! as List)[1] as Map<String, Object?>;
      (bus['trip']! as Map)['start_date'] = '20261110';
      for (final d
          in (bus['departures']! as List).cast<Map<String, Object?>>()) {
        d['start_date'] = '20261110';
      }
      final chain = select(ConnectionChain(own), 0, 't40c-x');
      expect(chain.takes(journeyOf(json)), isFalse);
    });

    test('is between the same stops', () {
      final json = laterJourneyJson('-x');
      final stops = ((json['legs']! as List)[3] as Map)['stops']! as List;
      (stops.last as Map)['stop_id'] = 'elsewhere';
      final chain = select(ConnectionChain(own), 0, 't40c-x');
      expect(chain.takes(journeyOf(json)), isFalse);
    });

    test('is none while a ride has no connection', () {
      expect(ConnectionChain(own, now: at(1741)).takes(own), isFalse);
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
