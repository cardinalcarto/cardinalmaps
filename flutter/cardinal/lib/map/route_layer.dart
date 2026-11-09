/// Routes on the map: their lines, the destination's pin, and a callout with each route's
/// travel time.
library;

import 'dart:async';

import 'package:routing/routing.dart';

import '../directions/callouts.dart';
import '../directions/format.dart';
import '../store/store.dart';

/// The style's GeoJSON source for the routes' lines and the destination.
const routeSource = 'route';

/// The style's GeoJSON source for the callouts, which change as the map moves.
const routeCalloutSource = 'route-callouts';

/// The property of a callout saying which route it's for.
const routeIndexProperty = 'route';

/// The route whose callout was tapped, by index. The map hears the tap, and the
/// directions page, if one's showing, knows what to do with it.
final routeTaps = StreamController<int>.broadcast();

const empty = {'type': 'FeatureCollection', 'features': <Object>[]};

const _selectedColor = '#3b81de';
const _selectedCasing = '#1a4c90';
const _alternateColor = '#7f8a99';
const _alternateCasing = '#4a525c';

Map<String, Object?> _line(
  String id,
  String color,
  double width, {
  required bool selected,
}) => {
  'id': id,
  'type': 'line',
  'source': routeSource,
  'filter': [
    'all',
    [
      '==',
      ['get', 'role'],
      'line',
    ],
    [
      '==',
      ['get', 'selected'],
      selected,
    ],
  ],
  'layout': {'line-cap': 'round', 'line-join': 'round'},
  'paint': {'line-color': color, 'line-width': width},
};

/// Adds the sources for routes and the layers that draw them to [style], a MapLibre
/// style: the lines under the first symbol layer, so labels stay on top, with the selected
/// route over the others; and the pin and callouts above everything.
void addRouteLayers(Map<String, Object?> style) {
  final sources = style['sources'] as Map<String, Object?>;
  sources[routeSource] = {'type': 'geojson', 'data': empty};
  sources[routeCalloutSource] = {'type': 'geojson', 'data': empty};
  final layers = style['layers'] as List<Object?>;
  final firstSymbol = layers.indexWhere((l) => (l as Map)['type'] == 'symbol');
  layers.insertAll(firstSymbol < 0 ? layers.length : firstSymbol, [
    _line('route-alternate-casing', _alternateCasing, 11, selected: false),
    _line('route-alternate', _alternateColor, 8, selected: false),
    _line('route-selected-casing', _selectedCasing, 9, selected: true),
    _line('route-selected', _selectedColor, 6, selected: true),
  ]);
  layers.addAll([
    {
      'id': 'route-destination',
      'type': 'symbol',
      'source': routeSource,
      'filter': [
        '==',
        ['get', 'role'],
        'pin',
      ],
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
    },
    {
      'id': 'route-callouts',
      'type': 'symbol',
      'source': routeCalloutSource,
      // A bubble drawn by `tool/callout_sprite.py`, stretched to fit the text, with its
      // arrow's tip on the route: the text sits above the point by the arrow and the body
      // under the text. Callouts never give way to labels or hide them.
      'layout': {
        'icon-image': 'route_callout',
        'icon-text-fit': 'both',
        'icon-text-fit-padding': [1, 2, 1, 2],
        'icon-allow-overlap': true,
        'icon-ignore-placement': true,
        'text-field': ['get', 'duration'],
        'text-font': ['Roboto Medium'],
        'text-size': 12,
        'text-anchor': 'bottom',
        'text-offset': [0, -1.1],
        'text-allow-overlap': true,
        'text-ignore-placement': true,
      },
      'paint': {'text-color': '#ffffff'},
    },
  ]);
}

/// [routes]' lines and the pin at [destination], as GeoJSON.
Map<String, Object?> routesGeoJson(
  List<DirectionsRoute> routes, {
  required int selected,
  required LngLat? destination,
  required bool dark,
}) => {
  'type': 'FeatureCollection',
  'features': [
    for (final (index, route) in routes.indexed)
      {
        'type': 'Feature',
        'properties': {'role': 'line', 'selected': index == selected},
        'geometry': {
          'type': 'LineString',
          'coordinates': [
            for (final p in route.geometry) [p.lng, p.lat],
          ],
        },
      },
    if (destination != null)
      {
        'type': 'Feature',
        'properties': {'role': 'pin', 'dark': dark},
        'geometry': {
          'type': 'Point',
          'coordinates': [destination.lon, destination.lat],
        },
      },
  ],
};

/// [callouts], each showing its route's travel time, as GeoJSON.
Map<String, Object?> calloutsGeoJson(
  List<Callout> callouts,
  List<DirectionsRoute> routes,
) => {
  'type': 'FeatureCollection',
  'features': [
    for (final callout in callouts)
      {
        'type': 'Feature',
        // The map only reports taps on features with IDs, and zero means none.
        'id': callout.route + 1,
        'properties': {
          routeIndexProperty: callout.route.toString(),
          'duration': formatDuration(routes[callout.route].duration),
        },
        'geometry': {
          'type': 'Point',
          'coordinates': [callout.lng, callout.lat],
        },
      },
  ],
};
