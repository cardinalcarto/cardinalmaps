/// The selected place on the map: a GeoJSON source the style draws its pin and boundary
/// from, and where the camera goes to show it.
library;

import 'dart:math' as math;

import 'package:sparkle/sparkle.dart';

import '../store/store.dart';

/// The style's GeoJSON source for the selected place.
const selectionSource = 'selection';

const _empty = {'type': 'FeatureCollection', 'features': <Object>[]};

/// Adds the selection source and the layers that draw it to [style], a MapLibre style:
/// the boundary under the first symbol layer, so labels stay on top, and the pin above
/// everything.
void addSelectionLayers(Map<String, Object?> style) {
  (style['sources'] as Map<String, Object?>)[selectionSource] = {
    'type': 'geojson',
    'data': _empty,
  };
  const boundary = [
    '==',
    ['get', 'role'],
    'boundary',
  ];
  const color = '#1a73e8';
  final layers = style['layers'] as List<Object?>;
  final firstSymbol = layers.indexWhere((l) => (l as Map)['type'] == 'symbol');
  layers.insertAll(firstSymbol < 0 ? layers.length : firstSymbol, [
    {
      'id': 'selection-fill',
      'type': 'fill',
      'source': selectionSource,
      'filter': boundary,
      'paint': {'fill-color': color, 'fill-opacity': 0.12},
    },
    {
      'id': 'selection-outline',
      'type': 'line',
      'source': selectionSource,
      'filter': boundary,
      'paint': {'line-color': color, 'line-width': 2},
    },
  ]);
  layers.add({
    'id': 'selection-pin',
    'type': 'symbol',
    'source': selectionSource,
    'filter': [
      '==',
      ['get', 'role'],
      'pin',
    ],
    // Drawn by `tool/pin_sprite.py`. Its point is on the place, and it never gives way to
    // labels or hides them.
    'layout': {
      'icon-image': [
        'case',
        ['get', 'dark'],
        'selection_pin_dark',
        'selection_pin',
      ],
      'icon-anchor': 'bottom',
      'icon-allow-overlap': true,
      'icon-ignore-placement': true,
    },
  });
}

/// How wide an area needs to be on screen, in logical pixels, at [placeZoom], for its
/// boundary to show where it is without a pin.
const areaMinSize = 40.0;

/// Whether [place] is a polygon at least [areaMinSize] across at [placeZoom]: a park or a
/// city, say, rather than a building, whose boundary is a speck.
bool showsAsArea(Place place) {
  final shape = place.shape;
  if (shape == null ||
      (shape['type'] != 'Polygon' && shape['type'] != 'MultiPolygon')) {
    return false;
  }
  var west = double.infinity,
      east = -double.infinity,
      south = double.infinity,
      north = -double.infinity;
  void visit(Object? coords) {
    if (coords is List &&
        coords.length >= 2 &&
        coords[0] is num &&
        coords[1] is num) {
      final lng = (coords[0] as num).toDouble(),
          lat = (coords[1] as num).toDouble();
      west = math.min(west, lng);
      east = math.max(east, lng);
      south = math.min(south, lat);
      north = math.max(north, lat);
    } else if (coords is List) {
      coords.forEach(visit);
    }
  }

  visit(shape['coordinates']);
  if (west > east) return false;
  // Web Mercator with 512-pixel tiles: a degree of longitude is this many pixels at
  // [placeZoom], and a degree of latitude near [lat] about as many over cos(lat).
  final lat = (south + north) / 2 * math.pi / 180;
  final pixelsPerDegree = 512 * math.pow(2, placeZoom) / 360;
  final wide = (east - west) * pixelsPerDegree;
  final tall =
      (north - south) * pixelsPerDegree / math.max(math.cos(lat), 1e-6);
  return math.max(wide, tall) >= areaMinSize;
}

/// The place [showSelection] last showed.
String? _shown;

/// Shows [place]'s boundary if it has a shape, and its pin unless it's an area big enough
/// for its boundary to show where it is (see [showsAsArea]); or nothing for null. [dark]
/// is whether the app is in dark mode, which has a pin of its own.
void showSelection(
  SparkleMapController map,
  Place? place, {
  bool dark = false,
}) {
  _shown = place?.id;
  if (place == null) {
    map.setGeoJson(selectionSource, _empty);
    return;
  }
  final area = showsAsArea(place);
  map.setGeoJson(selectionSource, {
    'type': 'FeatureCollection',
    'features': [
      if (place.shape case final shape?)
        {
          'type': 'Feature',
          'properties': {'role': 'boundary'},
          'geometry': shape,
        },
      if (!area)
        {
          'type': 'Feature',
          'properties': {'role': 'pin', 'dark': dark},
          'geometry': {
            'type': 'Point',
            'coordinates': [place.center.lon, place.center.lat],
          },
        },
    ],
  });
}

/// Shows nothing, and lets the camera go, if the place with [id] is what's showing;
/// another page may have shown its own place since.
void clearSelection(SparkleMapController map, String id) {
  if (_shown != id) return;
  showSelection(map, null);
  map.clearSubject();
}

/// The zoom a place is shown at, unless its bounds need a wider view.
const placeZoom = 14.0;

/// Space kept around a place's bounds, in logical pixels.
const _margin = 48.0;

/// Makes [place] the map's subject: it flies to show its pin at [placeZoom], or closer if
/// the map is already closer, or all of its bounds (with the pin) if they need a wider
/// view, and keeps it in view in what the sheet leaves uncovered as the sheet moves.
void focusPlace(SparkleMapController map, Place place) {
  final c = place.center;
  final bbox =
      place.bbox ??
      Bounds(west: c.lon, south: c.lat, east: c.lon, north: c.lat);
  final crosses = bbox.west > bbox.east;
  map.setSubject(
    west: crosses ? bbox.west : math.min(bbox.west, c.lon),
    south: math.min(bbox.south, c.lat),
    east: crosses ? bbox.east : math.max(bbox.east, c.lon),
    north: math.max(bbox.north, c.lat),
    maxZoom: math.max(placeZoom, map.camera?.zoom ?? placeZoom),
    margin: _margin,
  );
}
