/// Where the map was looking when the app last closed, so it opens there again.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../config.dart';
import '../store/store.dart';

/// Where the camera looks from: as sparkle's `--view` takes it.
@immutable
class MapViewport {
  const MapViewport({
    required this.lng,
    required this.lat,
    required this.zoom,
    this.bearing = 0,
    this.pitch = 0,
  });

  /// Where [map]'s camera is, if it has one yet.
  static MapViewport? of(SparkleMapController map) {
    final c = map.camera;
    return c == null
        ? null
        : MapViewport(
            lng: c.lng,
            lat: c.lat,
            zoom: c.zoom,
            bearing: c.bearing,
            pitch: c.pitch,
          );
  }

  /// Parses `lng,lat,zoom[,bearing,pitch]`, or null if [text] isn't one.
  static MapViewport? parse(String text) {
    final parts = text
        .split(',')
        .map((p) => double.tryParse(p.trim()))
        .toList();
    if (parts.contains(null) || (parts.length != 3 && parts.length != 5)) {
      return null;
    }
    final [lng!, lat!, zoom!, ...rest] = parts;
    return MapViewport(
      lng: lng,
      lat: lat,
      zoom: zoom,
      bearing: rest.isEmpty ? 0 : rest[0]!,
      pitch: rest.isEmpty ? 0 : rest[1]!,
    );
  }

  final double lng;
  final double lat;
  final double zoom;

  /// Degrees clockwise from north.
  final double bearing;

  /// Degrees from looking straight down.
  final double pitch;

  /// As sparkle's `--view` argument.
  String get arg => '$lng,$lat,$zoom,$bearing,$pitch';

  /// The same view, no closer than [maxZoom].
  MapViewport zoomedOutTo(double maxZoom) => MapViewport(
    lng: lng,
    lat: lat,
    zoom: math.min(zoom, maxZoom),
    bearing: bearing,
    pitch: pitch,
  );

  @override
  bool operator ==(Object other) =>
      other is MapViewport &&
      other.lng == lng &&
      other.lat == lat &&
      other.zoom == zoom &&
      other.bearing == bearing &&
      other.pitch == pitch;

  @override
  int get hashCode => Object.hash(lng, lat, zoom, bearing, pitch);

  @override
  String toString() => 'MapViewport($arg)';
}

/// The view the app closed with, if it's closed before.
final savedViewport = Setting<MapViewport?>(
  'map.viewport',
  null,
  encode: (v) => v == null ? null : [v.lng, v.lat, v.zoom, v.bearing, v.pitch],
  decode: (json) {
    if (json is! List || json.length != 5 || json.any((n) => n is! num)) {
      return null;
    }
    final [lng, lat, zoom, bearing, pitch] = [
      for (final n in json) (n as num).toDouble(),
    ];
    return MapViewport(
      lng: lng,
      lat: lat,
      zoom: zoom,
      bearing: bearing,
      pitch: pitch,
    );
  },
);

/// A view restored from closer in than this opens at this zoom: after time away, the
/// streets you were looking at are less likely to be what you want than the area.
const restoredMaxZoom = 12.0;

/// The whole planet, for the first launch.
const planetView = MapViewport(lng: 0, lat: 20, zoom: 0);

/// Whether this is a build for `tool/map_bench.sh`, which opens at [initialView] every
/// time so every run draws the same frames.
const _bench = bool.fromEnvironment('SPARKLE_BENCH');

/// Where the map opens: where it was when the app closed, zoomed out to
/// [restoredMaxZoom]; or else, on the first launch, [initialView] if the build sets one,
/// or the planet. A benchmark build always opens at [initialView].
MapViewport openingView(MapViewport? saved) {
  final fixed = MapViewport.parse(initialView);
  if (_bench && fixed != null) return fixed;
  return saved?.zoomedOutTo(restoredMaxZoom) ?? fixed ?? planetView;
}
