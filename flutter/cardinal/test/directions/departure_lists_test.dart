import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/departure_lists.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;

import '../fakes/fake_transit.dart';

DateTime _at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch((fakeStart + seconds) * 1000);

void main() {
  late FakeTransit transit;
  late DepartureLists lists;
  late TransitPlan plan;
  late ConnectionChain chain;
  const walking = TransitAccess.walking;

  setUp(() {
    transit = FakeTransit();
    lists = DepartureLists(transit);
    plan = fakePlan();
    chain = ConnectionChain(plan.journeys.single, now: _at(0));
  });

  tearDown(() => lists.dispose());

  group('what is asked', () {
    test('each ride, around the departure it takes, two before and seven '
        'from it on', () {
      final later = chain.select(
        0,
        chain.rides[0].departures.indexWhere((d) => d.tripId == 't40c'),
      );
      lists.ask(fakePlan(), later, TransitAccess.wheelchair, _at(0));
      expect(transit.departuresAsked, hasLength(2));
      final bus = transit.departuresAsked[0];
      expect(bus.access, TransitAccess.wheelchair);
      expect(bus.query, (
        feed: 'us-wa-Metro-Transit',
        boardStopId: 'Fremont Ave N & N 34th St',
        alightStopId: 'Westlake Station',
        around: _at(1320),
        before: 2,
        after: 7,
        ready: null,
      ));
      final link = transit.departuresAsked[1];
      expect(link.query.boardStopId, 'Westlake Station Bay 1');
      expect(link.query.alightStopId, 'Capitol Hill Station');
      expect(link.query.around, _at(2040));
    });

    test('the first ride with every departure gone, around the time it is; '
        'the rides after it not at all', () {
      // The walk takes five minutes, and the last 40 leaves at 2040 s.
      final gone = chain.at(_at(1800));
      expect(gone.gone, isTrue);
      expect(gone.rides[1].connection, RideConnection.unknown);
      lists.ask(fakePlan(), gone, walking, _at(1800));
      expect(transit.departuresAsked.single.query.around, _at(1800));
      expect(
        transit.departuresAsked.single.query.boardStopId,
        'Fremont Ave N & N 34th St',
      );
    });

    test('a later ride with no connection, around when the rider is ready for '
        'it', () {
      final journey = connectionsJourney();
      var chain = ConnectionChain(journey);
      // The last 40 that runs reaches Stop B after every 1 Line listed has left.
      chain = chain.select(
        0,
        chain.rides[0].departures.indexWhere((d) => d.tripId == '31-1'),
      );
      expect(chain.rides[1].connection, RideConnection.none);
      expect(chain.rides[2].connection, RideConnection.unknown);
      lists.ask(fakePlan(), chain, walking, _at(0));
      expect(transit.departuresAsked.map((q) => q.query.boardStopId), [
        'Stop A',
        'Stop C',
      ]);
      // It gets to Stop B at 18:10, three minutes from Stop C.
      expect(transit.departuresAsked[1].query.around, _at(2400 + 180));
      expect(transit.departuresAsked[1].query.around, chain.rides[1].ready);
    });

    test('once for a plan, and again for the next', () {
      final first = fakePlan();
      lists.ask(first, chain, walking, _at(0));
      lists.ask(first, chain, walking, _at(30));
      expect(transit.departuresAsked, hasLength(2));
      lists.ask(fakePlan(), chain, walking, _at(30));
      expect(transit.departuresAsked, hasLength(4));
    });
  });

  group('an answer', () {
    test('with departures is kept, and listeners told', () async {
      var told = 0;
      lists.addListener(() => told++);
      lists.ask(fakePlan(), chain, walking, _at(0));
      expect(lists.asking(0), isTrue);
      final answer = fakeDepartures([('t40', 600), ('t40q', 900)]);
      transit.departuresAsked[0].answer.complete(answer);
      await pumpEventQueue();
      expect(told, 1);
      expect(lists.revision, 1);
      expect(lists.lists, {0: answer});
      expect(lists.asking(0), isFalse);
      expect(lists.asking(1), isTrue);
    });

    test('with none leaves what was looked up, and tells no one', () async {
      var told = 0;
      lists.addListener(() => told++);
      lists.ask(fakePlan(), chain, walking, _at(0));
      final answer = fakeDepartures([('t40', 600)]);
      transit.departuresAsked[0].answer.complete(answer);
      await pumpEventQueue();
      lists.ask(fakePlan(), chain, walking, _at(30));
      transit.departuresAsked[2].answer.complete(const []);
      await pumpEventQueue();
      expect(told, 1);
      expect(lists.lists, {0: answer});
      expect(lists.asking(0), isFalse);
    });

    test('that fails leaves what was looked up, and the next plan asks '
        'again', () async {
      var told = 0;
      lists.addListener(() => told++);
      lists.ask(fakePlan(), chain, walking, _at(0));
      transit.departuresAsked[0].answer.completeError(
        const TransitPlanError('unknown_stop', 'gone'),
      );
      transit.departuresAsked[1].answer.completeError(
        http.RequestAbortedException(),
      );
      await pumpEventQueue();
      expect(told, 0);
      expect(lists.lists, isEmpty);
      expect(lists.asking(0), isFalse);
      lists.ask(fakePlan(), chain, walking, _at(30));
      expect(transit.departuresAsked, hasLength(4));
    });

    test('for an older plan is ignored, the asking given up on', () async {
      lists.ask(fakePlan(), chain, walking, _at(0));
      final old = transit.departuresAsked[0];
      lists.ask(fakePlan(), chain, walking, _at(30));
      await pumpEventQueue();
      expect(old.aborted, isTrue);
      expect(transit.departuresAsked[1].aborted, isTrue);
      expect(transit.departuresAsked[2].aborted, isFalse);
      old.answer.complete(fakeDepartures([('t40', 600)]));
      await pumpEventQueue();
      expect(lists.lists, isEmpty);
      expect(lists.asking(0), isTrue);
    });
  });

  test('a source that can\'t be asked isn\'t asked again, by anyone', () async {
    lists.ask(fakePlan(), chain, walking, _at(0));
    final answer = fakeDepartures([('link', 2040)]);
    transit.departuresAsked[1].answer.complete(answer);
    await pumpEventQueue();
    transit.departuresAsked[0].answer.completeError(
      const TransitDeparturesUnsupported(),
    );
    await pumpEventQueue();
    expect(lists.unsupported, isTrue);
    // What was looked up before stays.
    expect(lists.lists, {1: answer});
    lists.ask(fakePlan(), chain, walking, _at(30));
    expect(transit.departuresAsked, hasLength(2));
    final others = DepartureLists(transit);
    addTearDown(others.dispose);
    expect(others.unsupported, isTrue);
    others.ask(fakePlan(), chain, walking, _at(30));
    expect(transit.departuresAsked, hasLength(2));
    // Another source is asked.
    final other = FakeTransit();
    final theirs = DepartureLists(other);
    addTearDown(theirs.dispose);
    theirs.ask(fakePlan(), chain, walking, _at(30));
    expect(other.departuresAsked, hasLength(2));
  });

  test('the first of several to say so gives up on the rest', () async {
    lists.ask(fakePlan(), chain, walking, _at(0));
    transit.departuresAsked[0].answer.completeError(
      const TransitDeparturesUnsupported(),
    );
    await pumpEventQueue();
    expect(transit.departuresAsked[1].aborted, isTrue);
    expect(lists.asking(1), isFalse);
  });

  test('forgetting gives up on what\'s asked and drops what was looked '
      'up', () async {
    final first = fakePlan();
    lists.ask(first, chain, walking, _at(0));
    transit.departuresAsked[0].answer.complete(fakeDepartures([('t40', 600)]));
    await pumpEventQueue();
    lists.forget();
    await pumpEventQueue();
    expect(lists.lists, isEmpty);
    expect(transit.departuresAsked[1].aborted, isTrue);
    expect(lists.asking(1), isFalse);
    // The same plan asks again, of a journey it lost a second time.
    lists.ask(first, chain, walking, _at(0));
    expect(transit.departuresAsked, hasLength(4));
  });

  test('disposed, it gives up and takes no answer', () async {
    final lists = DepartureLists(transit);
    lists.ask(fakePlan(), chain, walking, _at(0));
    lists.dispose();
    await pumpEventQueue();
    expect(transit.departuresAsked.every((q) => q.aborted), isTrue);
    transit.departuresAsked[0].answer.complete(fakeDepartures([('t40', 600)]));
    await pumpEventQueue();
  });
}
