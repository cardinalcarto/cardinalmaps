import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/departure_legs.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_transit.dart';

void main() {
  late FakeTransit transit;
  late DepartureLegs legs;
  late TransitPlan plan;
  late ConnectionChain chain;
  const walking = TransitAccess.walking;

  /// [chain] taking the bus's departure [trip].
  ConnectionChain taking(String trip) => chain.select(
    0,
    chain.rides[0].ride.departures.indexWhere((d) => d.tripId == trip),
  );

  setUp(() {
    transit = FakeTransit();
    legs = DepartureLegs(transit);
    plan = fakePlan();
    chain = ConnectionChain(plan.journeys.single);
  });

  tearDown(() => legs.dispose());

  test('asks only for departures other than the plan\'s own', () {
    legs.follow(plan, chain, walking);
    expect(transit.rides, isEmpty);
    expect(legs.legOf(chain.rides[0], walking), isNull);
    legs.follow(plan, null, walking);
    expect(transit.rides, isEmpty);
  });

  test('keeps what it fetched, and tells its listeners', () async {
    var told = 0;
    legs.addListener(() => told++);
    final later = taking('t40c');
    legs.follow(plan, later, walking);
    legs.follow(plan, later, walking);
    final question = transit.rides.single;
    expect(question.query.tripId, 't40c');
    expect(question.query.startDate, '20261109');
    expect(question.query.feed, 'us-wa-Metro-Transit');
    final leg = fakeRideLeg('t40c', const [
      [0.6, 2.1],
      [0.9, 3.2],
    ], departure: fakeStart + 1320);
    question.answer.complete(leg);
    await pumpEventQueue();
    expect(told, 1);
    expect(legs.legOf(later.rides[0], walking), same(leg));
    // Not the plan's own, nor another mode's.
    expect(legs.legOf(chain.rides[0], walking), isNull);
    expect(legs.legOf(later.rides[0], TransitAccess.wheelchair), isNull);

    // Followed again, it isn't asked again; a new plan forgets it and asks again.
    legs.follow(plan, later, walking);
    expect(transit.rides, hasLength(1));
    final newer = fakePlan();
    legs.follow(newer, ConnectionChain(newer.journeys.single), walking);
    expect(legs.legOf(later.rides[0], walking), isNull);
  });

  test(
    'a failure is forgotten, and asked again when it is chosen again',
    () async {
      final later = taking('t40c');
      legs.follow(plan, later, walking);
      transit.rides.single.answer.completeError(
        const TransitPlanError('unknown_trip', 'gone'),
      );
      await pumpEventQueue();
      expect(legs.legOf(later.rides[0], walking), isNull);
      legs.follow(plan, chain, walking);
      legs.follow(plan, later, walking);
      expect(transit.rides, hasLength(2));
    },
  );

  test('an answer for a departure no longer taken is dropped', () async {
    final later = taking('t40c');
    legs.follow(plan, later, walking);
    legs.follow(plan, chain, walking);
    final question = transit.rides.single;
    await pumpEventQueue();
    expect(question.aborted, isTrue);
    question.answer.complete(
      fakeRideLeg('t40c', const [
        [0.6, 2.1],
        [0.9, 3.2],
      ], departure: fakeStart + 1320),
    );
    await pumpEventQueue();
    expect(legs.legOf(later.rides[0], walking), isNull);
  });
}
