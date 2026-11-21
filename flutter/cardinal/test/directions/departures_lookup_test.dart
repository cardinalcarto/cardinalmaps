import 'dart:async';
import 'dart:convert';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;

import '../fakes/fake_transit.dart';

/// 2026-06-15 08:00 in Seattle, in POSIX seconds.
const _eight = 1781535600;

DateTime _at(int seconds) =>
    DateTime.fromMillisecondsSinceEpoch(seconds * 1000);

Map<String, Object?> _alert(String id, String header) => {
  'feed': 'metro',
  'id': id,
  'effect': 'detour',
  'active_periods': <Object>[],
  'header': [
    {'text': header, 'language': 'en'},
  ],
  'description': <Object>[],
  'url': <Object>[],
};

/// A departures lookup's answer, as the router gives it: every departure with its
/// route, headsign and service day, and one list of alerts.
Map<String, Object?> _answer() => {
  'generated_at': _eight - 585,
  'realtime': {
    'level': 'realtime',
    'built_at': _eight - 585,
    'feeds': <Object>[],
  },
  'departures': [
    {
      'trip_id': 'R1-0800',
      'start_date': '20260615',
      'headsign': 'North',
      'route': {'route_id': 'R1', 'short_name': '1', 'route_type': 3},
      'status': 'live',
      'departure': _eight + 120,
      'arrival': _eight + 1320,
      'scheduled_departure': _eight,
      'scheduled_arrival': _eight + 1200,
      'delay_seconds': 120,
      'realtime': true,
      'vehicle_id': 'bus-7',
      'accessibility': 'accessible',
      'bikes_allowed': 'yes',
      'missed': true,
      'alerts': [0],
    },
    {
      'trip_id': 'R1-0900',
      'start_date': '20260615',
      'headsign': 'North',
      'route': {'route_id': 'R1', 'short_name': '1', 'route_type': 3},
      'status': 'canceled',
      'departure': _eight + 3600,
      'arrival': _eight + 4800,
      'scheduled_departure': _eight + 3600,
      'scheduled_arrival': _eight + 4800,
      'accessibility': 'accessible',
      'alerts': [0, 1],
    },
    {
      'trip_id': 'F1',
      'start_date': '20260616',
      'headsign': 'Express',
      'route': {'route_id': 'R3', 'short_name': '3', 'route_type': 3},
      'status': 'scheduled',
      'departure': _eight + 7200,
      'arrival': _eight + 8400,
      'scheduled_departure': _eight + 7200,
      'scheduled_arrival': _eight + 8400,
      'accessibility': 'unknown',
      'last_of_day': true,
    },
  ],
  'alerts': [
    _alert('route-1', 'Route 1 detoured'),
    _alert('trip-0900', 'The 9:00 is canceled'),
  ],
};

List<int> _bytes(Object? json) => utf8.encode(jsonEncode(json));

void main() {
  DeparturesQuery query({int? before, int? after, DateTime? ready}) => (
    feed: 'metro',
    boardStopId: 'A',
    alightStopId: 'C',
    around: _at(_eight),
    before: before,
    after: after,
    ready: ready,
  );

  group('the request', () {
    test('names the stops, the time and the mode, and leaves the rest out', () {
      expect(departuresRequest(query(), TransitAccess.wheelchair), {
        'feed': 'metro',
        'board_stop_id': 'A',
        'alight_stop_id': 'C',
        'around': _eight,
        'mode': 'wheelchair',
      });
    });

    test('says how many, and when the rider is ready', () {
      final asked = query(before: 0, after: 7, ready: _at(_eight + 90));
      expect(departuresRequest(asked, TransitAccess.walking), {
        'feed': 'metro',
        'board_stop_id': 'A',
        'alight_stop_id': 'C',
        'around': _eight,
        'departures_before': 0,
        'departures_after': 7,
        'ready': _eight + 90,
        'mode': 'walking',
      });
    });
  });

  group('the answer', () {
    test('is read as a ride\'s departures, each saying everything', () {
      final departures = RideDeparture.decodeLookup(_bytes(_answer()));
      expect(departures.map((d) => d.tripId), ['R1-0800', 'R1-0900', 'F1']);
      final live = departures[0];
      expect(live.startDate, '20260615');
      expect(live.headsign, 'North');
      expect(live.route?.id, 'R1');
      expect(live.status, DepartureStatus.live);
      expect(live.departure, _at(_eight + 120));
      expect(live.arrival, _at(_eight + 1320));
      expect(live.scheduledDeparture, _at(_eight));
      expect(live.scheduledArrival, _at(_eight + 1200));
      expect(live.delaySeconds, 120);
      expect(live.realtime, isTrue);
      expect(live.vehicleId, 'bus-7');
      expect(live.accessibility, Accessibility.accessible);
      expect(live.bikesAllowed, 'yes');
      expect(live.missed, isTrue);
      expect(live.lastOfDay, isFalse);
      expect(departures.every((d) => !d.suggested), isTrue);

      final canceled = departures[1];
      expect(canceled.status, DepartureStatus.canceled);
      expect(canceled.missed, isFalse);
      final last = departures[2];
      expect(last.route?.id, 'R3');
      expect(last.startDate, '20260616');
      expect(last.headsign, 'Express');
      expect(last.lastOfDay, isTrue);
    });

    test('has one list of alerts, which each departure\'s index', () {
      final departures = RideDeparture.decodeLookup(_bytes(_answer()));
      expect(departures[0].alerts.map((a) => a.header), ['Route 1 detoured']);
      expect(departures[1].alerts.map((a) => a.header), [
        'Route 1 detoured',
        'The 9:00 is canceled',
      ]);
      expect(departures[2].alerts, isEmpty);
    });

    test('can be of nothing: no departures, and no alerts', () {
      final answer = _answer()
        ..['departures'] = <Object>[]
        ..remove('alerts');
      expect(RideDeparture.decodeLookup(_bytes(answer)), isEmpty);
    });
  });

  group('the router\'s status', () {
    test('200 is the answer', () {
      final body = _bytes(_answer());
      expect(departuresAnswer(200, body), same(body));
    });

    test('a 404 that says why is the router\'s: something isn\'t there', () {
      for (final kind in ['unknown_feed', 'unknown_stop', 'date_not_covered']) {
        expect(
          () => departuresAnswer(
            404,
            _bytes({'error': 'find the stop the request names', 'kind': kind}),
          ),
          throwsA(
            isA<TransitPlanError>()
                .having((e) => e.kind, 'kind', kind)
                .having((e) => e.message, 'message', contains('find the stop')),
          ),
          reason: kind,
        );
      }
    });

    test('a 404 that doesn\'t is a router without the question', () {
      for (final body in [
        <int>[],
        utf8.encode('Not Found'),
        utf8.encode('<html><body>404</body></html>'),
        _bytes({'message': 'no such path'}),
        _bytes(['not', 'an', 'object']),
        _bytes({'kind': 404}),
      ]) {
        expect(
          () => departuresAnswer(404, body),
          throwsA(isA<TransitDeparturesUnsupported>()),
          reason: utf8.decode(body),
        );
      }
    });

    test('other failures are the router\'s errors, as a ride\'s are', () {
      expect(
        () => departuresAnswer(
          400,
          _bytes({'error': 'unknown mode "flying"', 'kind': 'invalid_request'}),
        ),
        throwsA(
          isA<TransitPlanError>().having(
            (e) => e.kind,
            'kind',
            'invalid_request',
          ),
        ),
      );
      expect(
        () => departuresAnswer(
          504,
          _bytes({'error': 'ran past its deadline', 'kind': 'timed_out'}),
        ),
        throwsA(
          isA<TransitPlanError>().having((e) => e.kind, 'kind', 'timed_out'),
        ),
      );
      // Not the router's words at all.
      expect(
        () => departuresAnswer(502, utf8.encode('<html>Bad Gateway</html>')),
        throwsA(
          isA<TransitPlanError>()
              .having((e) => e.kind, 'kind', 'failed')
              .having((e) => e.message, 'message', 'status 502'),
        ),
      );
      expect(
        () => departuresAnswer(500, const []),
        throwsA(
          isA<TransitPlanError>().having((e) => e.kind, 'kind', 'failed'),
        ),
      );
    });

    test('499 is the asker having given up', () {
      expect(
        () => departuresAnswer(499, _bytes({'kind': 'interrupted'})),
        throwsA(isA<http.RequestAbortedException>()),
      );
    });
  });

  group('the fake source', () {
    test('keeps what it was asked, and answers when the test does', () async {
      final transit = FakeTransit();
      final abort = Completer<void>();
      final asked = query(after: 3);
      final answer = transit.departures(
        query: asked,
        access: TransitAccess.cycling,
        abort: abort.future,
      );
      final question = transit.departuresAsked.single;
      expect(question.query, asked);
      expect(question.access, TransitAccess.cycling);
      expect(question.aborted, isFalse);
      final departures = RideDeparture.decodeLookup(_bytes(_answer()));
      question.answer.complete(departures);
      expect(await answer, same(departures));
      abort.complete();
      await pumpEventQueue();
      expect(question.aborted, isTrue);
    });

    test('can say the lookup isn\'t there', () async {
      final transit = FakeTransit();
      final answer = transit.departures(
        query: query(),
        access: TransitAccess.walking,
      );
      transit.departuresAsked.single.answer.completeError(
        const TransitDeparturesUnsupported(),
      );
      await expectLater(answer, throwsA(isA<TransitDeparturesUnsupported>()));
    });
  });
}
