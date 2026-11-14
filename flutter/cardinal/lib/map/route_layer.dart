/// Routes on the map: their lines, the start's marker, the destination's pin, and a
/// callout with each route's travel time.
library;

import 'dart:async';

import 'package:routing/routing.dart';

import '../directions/callouts.dart';
import '../directions/format.dart';
import '../directions/transit_plan.dart';
import '../store/store.dart';

/// The style's GeoJSON source for the routes' lines, the start, and the destination.
const routeSource = 'route';

/// The style's GeoJSON source for the callouts, which change as the map moves.
const routeCalloutSource = 'route-callouts';

/// The style's GeoJSON source for a transit journey's legs and the stops it boards and
/// leaves vehicles at.
const transitSource = 'transit';

/// The style's GeoJSON source for a nearby route's detail: its trip's line and stops,
/// the stop the rider gets on at, and its vehicles.
const nearbyRouteSource = 'nearby-route';

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
  sources[transitSource] = {'type': 'geojson', 'data': empty};
  sources[nearbyRouteSource] = {'type': 'geojson', 'data': empty};
  final layers = style['layers'] as List<Object?>;
  final firstSymbol = layers.indexWhere((l) => (l as Map)['type'] == 'symbol');
  layers.insertAll(firstSymbol < 0 ? layers.length : firstSymbol, [
    _line('route-alternate-casing', _alternateCasing, 11, selected: false),
    _line('route-alternate', _alternateColor, 8, selected: false),
    _line('route-selected-casing', _selectedCasing, 9, selected: true),
    _line('route-selected', _selectedColor, 6, selected: true),
    ..._transitLayers,
    ..._nearbyRouteLines,
  ]);
  layers.addAll([
    ..._nearbyRouteMarkers,
    {
      'id': 'route-start',
      'type': 'circle',
      'source': routeSource,
      'filter': [
        '==',
        ['get', 'role'],
        'start',
      ],
      'paint': {
        'circle-radius': 6,
        'circle-color': '#ffffff',
        'circle-stroke-color': '#333333',
        'circle-stroke-width': 3,
      },
    },
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

/// [routes]' lines, a marker at [start], and the pin at [destination], as GeoJSON.
///
/// [start] is null when the trip starts where you are, which the map already marks.
Map<String, Object?> routesGeoJson(
  List<DirectionsRoute> routes, {
  required int selected,
  required LngLat? destination,
  required bool dark,
  LngLat? start,
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
    if (start != null)
      {
        'type': 'Feature',
        'properties': {'role': 'start'},
        'geometry': {
          'type': 'Point',
          'coordinates': [start.lon, start.lat],
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

List<Object?> _roleFilter(String role) => [
  '==',
  ['get', 'role'],
  role,
];

/// A journey's walks dashed in the selected route's color, and its rides in their routes'
/// colors over a darker casing, with a dot where the rider gets on or off.
final _transitLayers = <Map<String, Object?>>[
  {
    'id': 'transit-street',
    'type': 'line',
    'source': transitSource,
    'filter': _roleFilter('street'),
    'layout': {'line-cap': 'butt', 'line-join': 'round'},
    'paint': {
      'line-color': _selectedColor,
      'line-width': 4,
      'line-dasharray': [1.2, 1],
    },
  },
  {
    'id': 'transit-ride-casing',
    'type': 'line',
    'source': transitSource,
    'filter': _roleFilter('ride'),
    'layout': {'line-cap': 'round', 'line-join': 'round'},
    'paint': {
      'line-color': [
        'to-color',
        ['get', 'casing'],
      ],
      'line-width': 9,
    },
  },
  {
    'id': 'transit-ride',
    'type': 'line',
    'source': transitSource,
    'filter': _roleFilter('ride'),
    'layout': {'line-cap': 'round', 'line-join': 'round'},
    'paint': {
      'line-color': [
        'to-color',
        ['get', 'color'],
      ],
      'line-width': 6,
    },
  },
  {
    'id': 'transit-stops',
    'type': 'circle',
    'source': transitSource,
    'filter': _roleFilter('stop'),
    'paint': {
      'circle-radius': 4,
      'circle-color': '#ffffff',
      'circle-stroke-color': '#333333',
      'circle-stroke-width': 2,
    },
  },
];

/// The color a ride is drawn in when its route has none.
const _defaultRideColor = '#3b81de';

/// [journey]'s legs and the stops it boards and leaves vehicles at, as GeoJSON.
Map<String, Object?> journeyGeoJson(Journey? journey) => {
  'type': 'FeatureCollection',
  'features': [
    if (journey != null)
      for (final leg in journey.legs)
        if (leg.shape.length >= 2)
          {
            'type': 'Feature',
            'properties': switch (leg) {
              TransitLeg(:final route) => {
                'role': 'ride',
                'color': _hex(route?.color) ?? _defaultRideColor,
                'casing': _darker(_hex(route?.color) ?? _defaultRideColor),
              },
              StreetLeg() => {'role': 'street'},
            },
            'geometry': {
              'type': 'LineString',
              'coordinates': [
                for (final p in leg.shape) [p.lng, p.lat],
              ],
            },
          },
    if (journey != null)
      for (final ride in journey.rides)
        for (final visit in [ride.boarding, ride.alighting])
          if (visit.stop.point case final p?)
            {
              'type': 'Feature',
              'properties': {'role': 'stop'},
              'geometry': {
                'type': 'Point',
                'coordinates': [p.lng, p.lat],
              },
            },
  ],
};

/// `#RRGGBB` for a GTFS color, or null if it isn't one.
String? _hex(String? color) =>
    color != null && RegExp(r'^[0-9a-fA-F]{6}$').hasMatch(color)
    ? '#$color'
    : null;

/// [hex] at half its brightness, for a casing.
String _darker(String hex) {
  final value = int.parse(hex.substring(1), radix: 16);
  final r = (value >> 16 & 0xff) ~/ 2;
  final g = (value >> 8 & 0xff) ~/ 2;
  final b = (value & 0xff) ~/ 2;
  return '#${[r, g, b].map((c) => c.toRadixString(16).padLeft(2, '0')).join()}';
}

List<Object?> _nearbyRole(String role) => [
  '==',
  ['get', 'role'],
  role,
];

/// A nearby route's trip in its color over a darker casing, with its stops as small
/// dots, under the map's labels.
final _nearbyRouteLines = <Map<String, Object?>>[
  {
    'id': 'nearby-route-casing',
    'type': 'line',
    'source': nearbyRouteSource,
    'filter': _nearbyRole('line'),
    'layout': {'line-cap': 'round', 'line-join': 'round'},
    'paint': {
      'line-color': [
        'to-color',
        ['get', 'casing'],
      ],
      'line-width': 9,
    },
  },
  {
    'id': 'nearby-route-line',
    'type': 'line',
    'source': nearbyRouteSource,
    'filter': _nearbyRole('line'),
    'layout': {'line-cap': 'round', 'line-join': 'round'},
    'paint': {
      'line-color': [
        'to-color',
        ['get', 'color'],
      ],
      'line-width': 6,
    },
  },
  {
    'id': 'nearby-route-stops',
    'type': 'circle',
    'source': nearbyRouteSource,
    'filter': _nearbyRole('stop'),
    'paint': {
      'circle-radius': 3.5,
      'circle-color': '#ffffff',
      'circle-stroke-color': [
        'to-color',
        ['get', 'casing'],
      ],
      'circle-stroke-width': 2,
    },
  },
];

/// The stop the rider gets on at, larger, and the route's vehicles, each with a pointer
/// the way it's heading when that's known, above everything.
final _nearbyRouteMarkers = <Map<String, Object?>>[
  {
    'id': 'nearby-route-boarding',
    'type': 'circle',
    'source': nearbyRouteSource,
    'filter': _nearbyRole('boarding'),
    'paint': {
      'circle-radius': 7,
      'circle-color': '#ffffff',
      'circle-stroke-color': [
        'to-color',
        ['get', 'casing'],
      ],
      'circle-stroke-width': 4,
    },
  },
  {
    'id': 'nearby-route-headings',
    'type': 'symbol',
    'source': nearbyRouteSource,
    'filter': _nearbyRole('heading'),
    'layout': {
      'icon-image': ['get', 'icon'],
      'icon-rotate': ['get', 'bearing'],
      'icon-rotation-alignment': 'map',
      'icon-allow-overlap': true,
      'icon-ignore-placement': true,
    },
  },
  {
    'id': 'nearby-route-vehicles',
    'type': 'symbol',
    'source': nearbyRouteSource,
    'filter': _nearbyRole('vehicle'),
    'layout': {
      'icon-image': ['get', 'icon'],
      'icon-allow-overlap': true,
      'icon-ignore-placement': true,
    },
  },
];

/// A nearby route's detail as GeoJSON: [ride]'s line and its stops, from [boarding],
/// emphasized, to the end of its trip, in [route]'s color; and [vehicles], drawn with
/// [vehicleIcon] and turned to their bearing by [headingIcon] when they have one.
Map<String, Object?> nearbyRouteGeoJson({
  required TransitRoute? route,
  required TransitLeg? ride,
  required StopRef? boarding,
  required List<TransitVehicle> vehicles,
  required String vehicleIcon,
  required String headingIcon,
}) {
  final color = _hex(route?.color) ?? _defaultRideColor;
  final casing = _darker(color);
  Map<String, Object?> point(DirectionsPoint p, Map<String, Object?> props) => {
    'type': 'Feature',
    'properties': props,
    'geometry': {
      'type': 'Point',
      'coordinates': [p.lng, p.lat],
    },
  };
  final shape = ride?.shape ?? const <DirectionsPoint>[];
  return {
    'type': 'FeatureCollection',
    'features': [
      if (shape.length >= 2)
        {
          'type': 'Feature',
          'properties': {'role': 'line', 'color': color, 'casing': casing},
          'geometry': {
            'type': 'LineString',
            'coordinates': [
              for (final p in shape) [p.lng, p.lat],
            ],
          },
        },
      if (ride != null)
        for (final visit in ride.stops.skip(1))
          if (visit.stop.point case final p?)
            point(p, {'role': 'stop', 'casing': casing}),
      if (boarding?.point case final p?)
        point(p, {'role': 'boarding', 'casing': casing}),
      for (final v in vehicles)
        if (v.point case final p?) ...[
          if (v.bearing case final bearing?)
            point(p, {
              'role': 'heading',
              'icon': headingIcon,
              'bearing': bearing,
            }),
          point(p, {'role': 'vehicle', 'icon': vehicleIcon}),
        ],
    ],
  };
}
