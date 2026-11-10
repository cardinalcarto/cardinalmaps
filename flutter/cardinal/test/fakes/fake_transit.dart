import 'dart:async';

import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/store/store.dart';

/// A question [FakeTransit] was asked, waiting to be answered.
class TransitQuestion {
  TransitQuestion(this.access, this.from, this.to, this.abort);

  final TransitAccess access;
  final LngLat from;
  final LngLat to;
  final Future<void>? abort;
  final answer = Completer<TransitPlan>();
}

/// Transit plans answered by the test, or at once with [immediately] if it's set.
class FakeTransit implements TransitSource {
  FakeTransit({this.immediately});

  TransitPlan? immediately;
  final asked = <TransitQuestion>[];
  var prepared = 0;

  @override
  void prepare() => prepared++;

  @override
  Future<TransitPlan> plan({
    required TransitAccess access,
    required LngLat from,
    required LngLat to,
    Future<void>? abort,
  }) {
    final question = TransitQuestion(access, from, to, abort);
    asked.add(question);
    final now = immediately;
    if (now != null) question.answer.complete(now);
    return question.answer.future;
  }
}

/// 2026-11-09 17:30 in Seattle (UTC-8), in POSIX seconds.
const fakeStart = 1794792600;

/// `[[lng, lat], ...]` as polyline6.
String polyline6(List<List<double>> points) {
  final out = StringBuffer();
  var lastLat = 0, lastLng = 0;
  void put(int value) {
    var v = value < 0 ? ~(value << 1) : value << 1;
    while (v >= 0x20) {
      out.writeCharCode((0x20 | (v & 0x1f)) + 63);
      v >>= 5;
    }
    out.writeCharCode(v + 63);
  }

  for (final p in points) {
    final lat = (p[1] * 1e6).round(), lng = (p[0] * 1e6).round();
    put(lat - lastLat);
    put(lng - lastLng);
    lastLat = lat;
    lastLng = lng;
  }
  return out.toString();
}

Map<String, Object?> _stop(String name, double lng, double lat, int time) => {
  'accessibility': 'accessible',
  'feed': 'us-wa-Metro-Transit',
  'stop_id': name,
  'name': name,
  'latitude': lat,
  'longitude': lng,
  'arrival': time,
  'departure': time,
  'scheduled_arrival': time,
  'scheduled_departure': time,
  'skipped': false,
  'removed_by_alert': false,
  'utc_offset_seconds': -8 * 3600,
};

Map<String, Object?> _alert(String id, String header, {String? description}) =>
    {
      'id': id,
      'effect': 'detour',
      'active_periods': <Object>[],
      'communication_periods': <Object>[],
      'impact_periods': <Object>[],
      'header': [
        {'text': header},
      ],
      'description': [
        if (description != null) {'text': description},
      ],
      'url': <Object>[],
    };

Map<String, Object?> _departure(
  String trip,
  int departure,
  int ride, {
  String status = 'scheduled',
  int delay = 0,
  bool suggested = false,
  bool missed = false,
  bool last = false,
  String accessibility = 'accessible',
  List<int> alerts = const [],
  List<int> departureAlerts = const [],
}) => {
  'trip_id': trip,
  'status': status,
  'departure': departure,
  'arrival': departure + ride,
  'scheduled_departure': departure - delay,
  'scheduled_arrival': departure + ride - delay,
  if (delay != 0) 'delay_seconds': delay,
  if (status == 'live') 'realtime': true,
  'accessibility': accessibility,
  if (suggested) 'suggested': true,
  if (missed) 'missed': true,
  if (last) 'last_of_day': true,
  if (alerts.isNotEmpty) 'alerts': alerts,
  if (departureAlerts.isNotEmpty) 'departure_alerts': departureAlerts,
};

/// A plan's JSON with one journey: a walk to a moved stop, a stay-seated bus ride that
/// runs 4 minutes late under a detour alert, a transfer, a ride on Link, and a walk.
///
/// The bus lists departures around its own: two the rider misses, then its own, one 12
/// minutes later with an alert of its own, a canceled one, and the day's last. Link's
/// own departure is the day's last, after one the rider misses.
Map<String, Object?> fakePlanJson({int journeys = 1}) {
  final t = fakeStart;
  Map<String, Object?> journey(int shift) => {
    'departure': t + shift,
    'arrival': t + shift + 2700,
    'duration_seconds': 2700,
    'trips': 2,
    'walking': {
      'access_seconds': 300,
      'transfer_seconds': 180,
      'egress_seconds': 120,
      'total_seconds': 600,
    },
    'wait_seconds': 60,
    'legs': [
      {
        'type': 'access',
        'mode': 'walking',
        'accessibility': 'unknown',
        'duration_seconds': 300,
        'departure': t + shift,
        'arrival': t + shift + 300,
        'stop': _stop('Fremont Ave N & N 34th St', 0.6, 2.1, t + shift + 300),
        'walk': {
          'distance_meters': 400.0,
          'duration_seconds': 300.0,
          'shape': polyline6([
            [0.5, 2],
            [0.6, 2.1],
          ]),
          'maneuvers': [
            {
              'type': 'start',
              'instruction': 'Walk north on Fremont Avenue North.',
              'distance_meters': 400.0,
              'duration_seconds': 300.0,
              'begin_shape_index': 0,
              'end_shape_index': 1,
            },
          ],
        },
        'stop_moved': [
          {
            'walk_end': 'end',
            'stop_id': '1',
            'name': 'Fremont Ave N & N 34th St',
            'header': [
              {'text': 'Stop relocated', 'language': 'en'},
            ],
            'description': [
              {
                'text': 'Board at the temporary stop 50 feet north.',
                'language': 'en',
              },
            ],
            'url': <Object>[],
          },
        ],
      },
      {
        'type': 'transit',
        'accessibility': 'accessible',
        'departure': t + shift + 600,
        'arrival': t + shift + 1800,
        'duration_seconds': 1200,
        'wait_seconds': 300,
        'delay_seconds': 240,
        'realtime': true,
        'route': {
          'route_id': '100040',
          'short_name': '40',
          'route_type': 3,
          'color': 'FDB71A',
          'text_color': '000000',
        },
        'agency': {'name': 'Metro Transit'},
        'trip': {
          'trip_id': 't40',
          'headsign': 'Downtown Seattle',
          'realtime_state': 'delayed',
        },
        'stops': [
          {
            ..._stop('Fremont Ave N & N 34th St', 0.6, 2.1, t + shift + 600),
            'scheduled_departure': t + shift + 360,
            'predicted_departure': t + shift + 600,
            'departure_delay_seconds': 240,
          },
          _stop('3rd Ave & Pike St', 0.8, 3, t + shift + 1200),
          _stop('Westlake Station', 0.9, 3.2, t + shift + 1800),
        ],
        'segments': [
          {
            'trip_id': 't40',
            'route': {
              'route_id': '100040',
              'short_name': '40',
              'route_type': 3,
            },
            'headsign': 'Downtown Seattle',
            'first_stop_index': 0,
            'last_stop_index': 1,
          },
          {
            'trip_id': 't7',
            'route': {'route_id': '100007', 'short_name': '7', 'route_type': 3},
            'headsign': 'Rainier Beach',
            'first_stop_index': 1,
            'last_stop_index': 2,
          },
        ],
        'alerts': [
          {
            'id': 'a1',
            'effect': 'detour',
            'active_periods': <Object>[],
            'communication_periods': <Object>[],
            'impact_periods': <Object>[],
            'header': [
              {'text': 'Route 40 detoured'},
            ],
            'description': <Object>[],
            'url': <Object>[],
            'tts_header': <Object>[],
            'tts_description': <Object>[],
            'cause_detail': <Object>[],
            'effect_detail': <Object>[],
            'image': <Object>[],
            'image_alternative_text': <Object>[],
            'informed': <Object>[],
          },
        ],
        'departures': [
          _departure('t40a', t + shift, 1200, missed: true),
          _departure(
            't40b',
            t + shift + 300,
            1200,
            status: 'live',
            delay: 60,
            missed: true,
          ),
          _departure(
            't40',
            t + shift + 600,
            1200,
            status: 'live',
            delay: 240,
            suggested: true,
            alerts: [0],
          ),
          _departure(
            't40c',
            t + shift + 1320,
            1200,
            accessibility: 'inaccessible',
            departureAlerts: [0],
          ),
          _departure('t40d', t + shift + 1800, 1200, status: 'canceled'),
          _departure('t40e', t + shift + 2040, 1200, last: true),
        ],
        'departure_alerts': [
          _alert(
            'a2',
            'Trip 40c ends at Westlake',
            description: 'Riders continuing south should transfer.',
          ),
        ],
      },
      {
        'type': 'transfer',
        'mode': 'walking',
        'accessibility': 'unknown',
        'duration_seconds': 180,
        'departure': t + shift + 1800,
        'arrival': t + shift + 1980,
        'from': _stop('Westlake Station', 0.9, 3.2, t + shift + 1800),
        'to': _stop('Westlake Station Bay 1', 0.91, 3.21, t + shift + 1980),
      },
      {
        'type': 'transit',
        'accessibility': 'unknown',
        'departure': t + shift + 2040,
        'arrival': t + shift + 2580,
        'duration_seconds': 540,
        'wait_seconds': 60,
        'realtime': false,
        'route': {
          'route_id': '100479',
          'short_name': '1 Line',
          'route_type': 0,
          'color': '28813F',
        },
        'agency': {'name': 'Sound Transit'},
        'trip': {'trip_id': 'link', 'headsign': 'Angle Lake'},
        'stops': [
          _stop('Westlake Station Bay 1', 0.91, 3.21, t + shift + 2040),
          _stop('Capitol Hill Station', 1, 3.8, t + shift + 2580),
        ],
        'alerts': <Object>[],
        'departures': [
          _departure('link0', t + shift + 1500, 540, missed: true),
          _departure(
            'link',
            t + shift + 2040,
            540,
            suggested: true,
            last: true,
            accessibility: 'unknown',
          ),
        ],
      },
      {
        'type': 'egress',
        'mode': 'walking',
        'accessibility': 'unknown',
        'duration_seconds': 120,
        'departure': t + shift + 2580,
        'arrival': t + shift + 2700,
        'stop': _stop('Capitol Hill Station', 1, 3.8, t + shift + 2580),
      },
    ],
  };
  return {
    'generated_at': t,
    'window': {'earliest': t, 'latest': t + 3600},
    'realtime': {
      'level': 'degraded',
      'built_at': t,
      'feeds': [
        {
          'feed': 'us-wa-Metro-Transit',
          'kind': 'trip_updates',
          'state': 'applied',
          'stale_after_seconds': 300,
          'entities_dropped': 0,
        },
        {
          'feed': 'us-wa-Sound-Transit',
          'kind': 'trip_updates',
          'state': 'stale',
          'stale_after_seconds': 300,
          'entities_dropped': 0,
        },
      ],
    },
    'journeys': [for (var i = 0; i < journeys; i++) journey(i * 600)],
    'direct': {'status': 'found', 'seconds': 5400},
  };
}

TransitPlan fakePlan({int journeys = 1}) =>
    TransitPlan.fromJson(fakePlanJson(journeys: journeys));

/// A plan with no journeys, and why.
TransitPlan emptyPlan(String reason) => TransitPlan.fromJson({
  'generated_at': fakeStart,
  'window': {'earliest': fakeStart, 'latest': fakeStart + 3600},
  'realtime': {'level': 'schedule', 'built_at': fakeStart, 'feeds': <Object>[]},
  'journeys': <Object>[],
  'no_journeys_reason': reason,
  'direct': {'status': 'not_searched'},
});
