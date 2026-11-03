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
  (style['sources'] as Map<String, Object?>)[selectionSource] = {'type': 'geojson', 'data': _empty};
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
    'type': 'circle',
    'source': selectionSource,
    'filter': [
      '==',
      ['get', 'role'],
      'pin',
    ],
    'paint': {
      'circle-radius': 9,
      'circle-color': '#d93025',
      'circle-stroke-width': 3,
      'circle-stroke-color': '#ffffff',
    },
  });
}

/// The place [showSelection] last showed.
String? _shown;

/// Shows [place]'s pin, and its boundary if it has a shape, or nothing for null.
void showSelection(SparkleMapController map, Place? place) {
  _shown = place?.id;
  if (place == null) {
    map.setGeoJson(selectionSource, _empty);
    return;
  }
  map.setGeoJson(selectionSource, {
    'type': 'FeatureCollection',
    'features': [
      if (place.shape case final shape?)
        {
          'type': 'Feature',
          'properties': {'role': 'boundary'},
          'geometry': shape,
        },
      {
        'type': 'Feature',
        'properties': {'role': 'pin'},
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

/// Makes [place] the map's subject: it flies to show its pin at [placeZoom], or all of
/// its bounds (with the pin) if they need a wider view, and keeps it in view in what
/// the sheet leaves uncovered as the sheet moves.
void focusPlace(SparkleMapController map, Place place) {
  final c = place.center;
  final bbox = place.bbox ?? Bounds(west: c.lon, south: c.lat, east: c.lon, north: c.lat);
  final crosses = bbox.west > bbox.east;
  map.setSubject(
    west: crosses ? bbox.west : math.min(bbox.west, c.lon),
    south: math.min(bbox.south, c.lat),
    east: crosses ? bbox.east : math.max(bbox.east, c.lon),
    north: math.max(bbox.north, c.lat),
    maxZoom: placeZoom,
    margin: _margin,
  );
}
