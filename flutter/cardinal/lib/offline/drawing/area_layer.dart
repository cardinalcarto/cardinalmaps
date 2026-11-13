/// An area drawn to download, on the map: a GeoJSON source the style fills and outlines.
library;

import 'package:sparkle/sparkle.dart';

import '../downloads/geometry.dart';

/// The style's GeoJSON source for the area being drawn.
const areaSource = 'drawn-area';

const _empty = {'type': 'FeatureCollection', 'features': <Object>[]};

/// `areaColor` in `map/map_drawing.dart`, as the style writes it.
const _color = '#3b81de';

/// Adds the area's source and the layers that draw it to [style], a MapLibre style:
/// filled and outlined under the first symbol layer, so labels stay on top.
void addAreaLayers(Map<String, Object?> style) {
  (style['sources'] as Map<String, Object?>)[areaSource] = {
    'type': 'geojson',
    'data': _empty,
  };
  final layers = style['layers'] as List<Object?>;
  final firstSymbol = layers.indexWhere((l) => (l as Map)['type'] == 'symbol');
  layers.insertAll(firstSymbol < 0 ? layers.length : firstSymbol, [
    {
      'id': 'drawn-area-fill',
      'type': 'fill',
      'source': areaSource,
      'paint': {'fill-color': _color, 'fill-opacity': 0.16},
    },
    {
      'id': 'drawn-area-outline',
      'type': 'line',
      'source': areaSource,
      'layout': {'line-cap': 'round', 'line-join': 'round'},
      'paint': {'line-color': _color, 'line-width': 3},
    },
  ]);
}

/// Shows [area] on [map], or nothing for null.
void showArea(SparkleMapController map, GeoPolygon? area) {
  map.setGeoJson(areaSource, area == null ? _empty : areaGeoJson(area));
}

/// [area] as a GeoJSON feature collection of one polygon.
Map<String, Object?> areaGeoJson(GeoPolygon area) => {
  'type': 'FeatureCollection',
  'features': [
    {
      'type': 'Feature',
      'properties': <String, Object?>{},
      'geometry': {
        'type': 'Polygon',
        'coordinates': [
          [
            for (final p in [...area.points, area.points.first]) [p.lng, p.lat],
          ],
        ],
      },
    },
  ],
};
