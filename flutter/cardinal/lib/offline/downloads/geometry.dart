/// The tilings offline downloads choose files by: Valhalla's routing tiles, and the
/// slippy-map tiles the transit index and transfer tiles use.
library;

import 'dart:math' as math;

import 'package:flutter/foundation.dart';

/// A point of a region's polygon, in degrees.
@immutable
class GeoPoint {
  const GeoPoint(this.lat, this.lng);

  final double lat;
  final double lng;

  @override
  bool operator ==(Object other) =>
      other is GeoPoint && other.lat == lat && other.lng == lng;

  @override
  int get hashCode => Object.hash(lat, lng);

  @override
  String toString() => 'GeoPoint($lat, $lng)';
}

/// `[west, south, east, north]` in degrees.
typedef GeoRect = ({double west, double south, double east, double north});

/// A region's outline: a ring of points that doesn't cross the antimeridian. Edges are
/// straight in latitude and longitude.
class GeoPolygon {
  /// Throws [ArgumentError] for fewer than three points or one off the globe.
  GeoPolygon(List<GeoPoint> ring)
    : points = List.unmodifiable(
        ring.length > 1 && ring.first == ring.last
            ? ring.sublist(0, ring.length - 1)
            : ring,
      ) {
    if (points.length < 3 ||
        points.any(
          (p) =>
              !p.lat.isFinite ||
              !p.lng.isFinite ||
              p.lat.abs() > 90 ||
              p.lng.abs() > 180,
        )) {
      throw ArgumentError.value(
        ring,
        'ring',
        'needs three points on the globe',
      );
    }
    bounds = (
      west: points.map((p) => p.lng).reduce(math.min),
      south: points.map((p) => p.lat).reduce(math.min),
      east: points.map((p) => p.lng).reduce(math.max),
      north: points.map((p) => p.lat).reduce(math.max),
    );
  }

  final List<GeoPoint> points;
  late final GeoRect bounds;

  bool _contains(double x, double y) {
    var inside = false;
    for (var i = 0, j = points.length - 1; i < points.length; j = i++) {
      final (xi, yi) = (points[i].lng, points[i].lat);
      final (xj, yj) = (points[j].lng, points[j].lat);
      if ((yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
        inside = !inside;
      }
    }
    return inside;
  }

  /// Whether [rect] and the polygon share any point.
  bool intersects(GeoRect rect) {
    final b = bounds;
    if (b.west > rect.east ||
        b.east < rect.west ||
        b.south > rect.north ||
        b.north < rect.south) {
      return false;
    }
    if (points.any(
          (p) =>
              p.lng >= rect.west &&
              p.lng <= rect.east &&
              p.lat >= rect.south &&
              p.lat <= rect.north,
        ) ||
        _contains((rect.west + rect.east) / 2, (rect.south + rect.north) / 2)) {
      return true;
    }
    final corners = [
      (rect.west, rect.south),
      (rect.east, rect.south),
      (rect.east, rect.north),
      (rect.west, rect.north),
    ];
    for (var i = 0; i < points.length; i++) {
      final a = (points[i].lng, points[i].lat);
      final next = points[(i + 1) % points.length];
      final b = (next.lng, next.lat);
      for (var j = 0; j < 4; j++) {
        if (_cross(a, b, corners[j], corners[(j + 1) % 4])) return true;
      }
    }
    return false;
  }

  /// `[[lng, lat], ...]`, as stored.
  List<List<double>> toJson() => [
    for (final p in points) [p.lng, p.lat],
  ];

  static GeoPolygon fromJson(List<Object?> json) => GeoPolygon([
    for (final p in json.cast<List<Object?>>())
      GeoPoint((p[1]! as num).toDouble(), (p[0]! as num).toDouble()),
  ]);
}

double _orient((double, double) a, (double, double) b, (double, double) c) =>
    (b.$1 - a.$1) * (c.$2 - a.$2) - (b.$2 - a.$2) * (c.$1 - a.$1);

bool _within((double, double) a, (double, double) b, (double, double) p) =>
    p.$1 >= math.min(a.$1, b.$1) &&
    p.$1 <= math.max(a.$1, b.$1) &&
    p.$2 >= math.min(a.$2, b.$2) &&
    p.$2 <= math.max(a.$2, b.$2);

bool _cross(
  (double, double) a,
  (double, double) b,
  (double, double) c,
  (double, double) d,
) {
  final o1 = _orient(a, b, c), o2 = _orient(a, b, d);
  final o3 = _orient(c, d, a), o4 = _orient(c, d, b);
  if (((o1 > 0 && o2 < 0) || (o1 < 0 && o2 > 0)) &&
      ((o3 > 0 && o4 < 0) || (o3 < 0 && o4 > 0))) {
    return true;
  }
  return (o1 == 0 && _within(a, b, c)) ||
      (o2 == 0 && _within(a, b, d)) ||
      (o3 == 0 && _within(c, d, a)) ||
      (o4 == 0 && _within(c, d, b));
}

/// Valhalla's tile hierarchy (`valinor/valhalla-graphtile`): level 0 is 4° tiles,
/// level 1 1°, level 2 0.25°, each numbered by row from the south pole and column from
/// the antimeridian. A level 0 tile holds exactly 16 level 1 tiles and 256 level 2 ones.
abstract final class ValhallaTiles {
  static const sizes = [4.0, 1.0, 0.25];

  static int columns(int level) => (360 / sizes[level]).round();

  static int rows(int level) => (180 / sizes[level]).round();

  /// The tile ID of the level's tile holding [lat], [lng].
  static int idAt(int level, double lat, double lng) {
    final size = sizes[level];
    final row = ((lat + 90) / size).floor().clamp(0, rows(level) - 1);
    final column = ((lng + 180) / size).floor().clamp(0, columns(level) - 1);
    return row * columns(level) + column;
  }

  static GeoRect bounds(int level, int id) {
    final size = sizes[level];
    final row = id ~/ columns(level), column = id % columns(level);
    return (
      west: -180 + column * size,
      south: -90 + row * size,
      east: -180 + (column + 1) * size,
      north: -90 + (row + 1) * size,
    );
  }

  /// The tiles of [level] whose squares intersect [polygon].
  static List<int> covering(int level, GeoPolygon polygon) {
    final b = polygon.bounds;
    final size = sizes[level];
    final firstRow = ((b.south + 90) / size).floor().clamp(0, rows(level) - 1);
    final lastRow = ((b.north + 90) / size).floor().clamp(0, rows(level) - 1);
    final firstColumn = ((b.west + 180) / size).floor().clamp(
      0,
      columns(level) - 1,
    );
    final lastColumn = ((b.east + 180) / size).floor().clamp(
      0,
      columns(level) - 1,
    );
    return [
      for (var row = firstRow; row <= lastRow; row++)
        for (var column = firstColumn; column <= lastColumn; column++)
          if (polygon.intersects(bounds(level, row * columns(level) + column)))
            row * columns(level) + column,
    ];
  }

  /// The tile's path as Valhalla names it: `2/000/792/230.gph`, the ID's digits padded
  /// to a multiple of three for the level's largest ID and grouped by three.
  static String path(int level, int id) {
    final maxId = columns(level) * rows(level) - 1;
    var digits = maxId.toString().length;
    digits += (3 - digits % 3) % 3;
    final text = id.toString().padLeft(digits, '0');
    final groups = [
      for (var i = 0; i < digits; i += 3) text.substring(i, i + 3),
    ];
    return '$level/${groups.join('/')}.gph';
  }

  /// The level and ID of a tile path [path] names (`tiles/` and all), or null.
  static (int, int)? parsePath(String path) {
    final match = RegExp(r'(?:^|/)([0-2])/([0-9/]+)\.gph$').firstMatch(path);
    if (match == null) return null;
    return (int.parse(match[1]!), int.parse(match[2]!.replaceAll('/', '')));
  }
}

/// Web mercator tiles: the transit index's zoom 6 tiles and transfer tiles' zoom 12.
abstract final class SlippyTiles {
  static GeoRect bounds(int z, int x, int y) {
    final n = math.pow(2, z).toDouble();
    double lat(int y) {
      final t = math.pi * (1 - 2 * y / n);
      return math.atan((math.exp(t) - math.exp(-t)) / 2) * 180 / math.pi;
    }

    return (
      west: x / n * 360 - 180,
      south: lat(y + 1),
      east: (x + 1) / n * 360 - 180,
      north: lat(y),
    );
  }

  static (int, int) at(int z, double lat, double lng) {
    final n = 1 << z;
    final x = ((lng + 180) / 360 * n).floor().clamp(0, n - 1);
    final r = lat.clamp(-85.0511, 85.0511) * math.pi / 180;
    final y = ((1 - math.log(math.tan(r) + 1 / math.cos(r)) / math.pi) / 2 * n)
        .floor()
        .clamp(0, n - 1);
    return (x, y);
  }

  /// The tiles of zoom [z] that intersect [polygon], as `(x, y)`.
  static List<(int, int)> covering(int z, GeoPolygon polygon) {
    final b = polygon.bounds;
    final (west, north) = at(z, b.north, b.west);
    final (east, south) = at(z, b.south, b.east);
    return [
      for (var x = west; x <= east; x++)
        for (var y = north; y <= south; y++)
          if (polygon.intersects(bounds(z, x, y))) (x, y),
    ];
  }
}
