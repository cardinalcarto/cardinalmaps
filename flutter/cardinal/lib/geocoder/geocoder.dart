import 'dart:async';
import 'dart:convert';

import 'package:flutter/widgets.dart';
import 'package:http/http.dart' as http;

import '../store/store.dart';

/// A request to the geocoder failed.
class GeocoderException implements Exception {
  GeocoderException(this.message);

  final String message;

  @override
  String toString() => 'GeocoderException: $message';
}

/// Cardinal's geocoder (airmail's `/v1` API; see `airmail_service/openapi.yaml`).
/// What the map shows, in degrees.
typedef Viewport = ({double west, double south, double east, double north});

class Geocoder {
  Geocoder(this.base, {http.Client? client})
    : _client = client ?? http.Client();

  /// Where the API is: requests go to `<base>/v1/...`.
  final Uri base;
  final http.Client _client;

  /// Places matching [query], most relevant first.
  ///
  /// [prefix] matches the last word as a prefix, for search as you type; [typos]
  /// tolerates typos. Results near [focus] rank higher. Completing [abort] cancels the
  /// request, which then throws [http.RequestAbortedException].
  Future<List<Place>> search(
    String query, {
    LngLat? focus,
    Viewport? viewport,
    String? lang,
    bool prefix = false,
    bool typos = false,
    int? limit,
    Future<void>? abort,
  }) async {
    final json = await _get('/v1/search', {
      'q': query,
      'lang': ?lang,
      if (prefix) 'prefix': 'true',
      if (typos) 'typos': 'true',
      if (focus != null) 'focus': '${focus.lon},${focus.lat}',
      if (viewport != null)
        'viewport':
            '${viewport.west},${viewport.south},${viewport.east},${viewport.north}',
      if (limit != null) 'limit': '$limit',
    }, abort);
    return [
      for (final r in json['results'] as List<Object?>)
        placeFromResult(r as _Json),
    ];
  }

  /// The place with [id], a result ID such as `poi:97175`, or null if there's none.
  Future<Place?> place(String id, {String? lang, Future<void>? abort}) async {
    try {
      final json = await _get('/v1/place', {'id': id, 'lang': ?lang}, abort);
      return placeFromResult(json['result'] as _Json);
    } on _NotFound {
      return null;
    }
  }

  Future<_Json> _get(
    String path,
    Map<String, String> query,
    Future<void>? abort,
  ) async {
    final basePath = base.path.endsWith('/')
        ? base.path.substring(0, base.path.length - 1)
        : base.path;
    final uri = base.replace(path: '$basePath$path', queryParameters: query);
    final request = http.AbortableRequest('GET', uri, abortTrigger: abort);
    final response = await http.Response.fromStream(
      await _client.send(request),
    );
    if (response.statusCode == 404) {
      throw _NotFound();
    }
    if (response.statusCode != 200) {
      throw GeocoderException(
        '${response.statusCode} from $uri: ${_problem(response)}',
      );
    }
    return jsonDecode(utf8.decode(response.bodyBytes)) as _Json;
  }

  /// The detail of an RFC 9457 problem, or the body.
  static String _problem(http.Response response) {
    try {
      final json = jsonDecode(response.body) as _Json;
      return (json['detail'] ?? json['title'] ?? response.body).toString();
    } on FormatException {
      return response.body;
    }
  }

  void close() => _client.close();
}

class _NotFound implements Exception {}

typedef _Json = Map<String, Object?>;

/// A geocoder result (the API's `Result`) as a [Place].
Place placeFromResult(Map<String, Object?> result) {
  final center = result['center'] as List<Object?>;
  final feature = result['feature'] as _Json?;
  final bbox = feature?['bbox'] as List<Object?>?;
  final geometry = feature?['geometry'] as _Json?;
  final address = result['address'] as _Json?;
  final poi = result['poi'] as _Json?;
  final osm = result['osm'] as _Json?;
  final tags = osm?['tags'] as _Json?;
  return Place(
    id: result['id'] as String,
    kind: result['kind'] as String,
    version: (poi?['version'] ?? osm?['version']) as int?,
    name: result['name'] as String?,
    countryCode: result['country_code'] as String?,
    center: LngLat(_double(center[0]), _double(center[1])),
    bbox: bbox == null
        ? null
        : Bounds(
            west: _double(bbox[0]),
            south: _double(bbox[1]),
            east: _double(bbox[2]),
            north: _double(bbox[3]),
          ),
    // A point says no more than the center.
    shape: geometry == null || geometry['type'] == 'Point' ? null : geometry,
    address: address == null ? null : Address.fromJson(address),
    admins: [
      for (final a in result['admins'] as List<Object?>? ?? const [])
        AdminRef.fromJson(a as _Json),
    ],
    tags: {
      for (final MapEntry(:key, :value) in (tags ?? const {}).entries)
        if (value is String) key: value,
    },
  );
}

double _double(Object? n) => (n as num).toDouble();

/// Provides the [Geocoder] to the widgets below it.
class GeocoderScope extends InheritedWidget {
  const GeocoderScope({
    super.key,
    required this.geocoder,
    required super.child,
  });

  final Geocoder geocoder;

  static Geocoder of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<GeocoderScope>()!.geocoder;

  @override
  bool updateShouldNotify(GeocoderScope oldWidget) =>
      geocoder != oldWidget.geocoder;
}
