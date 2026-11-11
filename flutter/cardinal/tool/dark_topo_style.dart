// Derives the dark Cardinal Topo style from the light one:
//
//     dart run tool/dark_topo_style.dart [light.json] [dark.json]
//
// Rerun it whenever the light style changes. Topo is for finding your way, so every
// color is chosen by hand here, for what it marks to stand out on the dark ground:
// trails, roads, contours, water and labels light and saturated, large fills mid-dark
// so nothing glares. A color the table doesn't cover stops the script, so a new one
// can't slip through unreadable.
import 'dart:convert';
import 'dart:io';

const _defaultLight = 'assets/map/cardinal-topo.json';
const _defaultDark = 'assets/map/cardinal-topo-dark.json';

void main(List<String> args) {
  final light = args.isNotEmpty ? args[0] : _defaultLight;
  final dark = args.length > 1 ? args[1] : _defaultDark;
  final style =
      jsonDecode(File(light).readAsStringSync()) as Map<String, Object?>;
  darkenTopo(style);
  final out = File(dark);
  final target = FileSystemEntity.isLinkSync(dark)
      ? File(out.resolveSymbolicLinksSync())
      : out;
  target.writeAsStringSync(
    '${const JsonEncoder.withIndent(' ').convert(style)}\n',
  );
}

/// The dark ground, which labels' and trails' halos match.
const _ground = '#1d1f1c';

/// Each light color and its dark one, by what it marks.
const topoPalette = <String, String>{
  // Ground and halos.
  '#f6f3e9': _ground,
  // Landcover: mid-dark, told apart by hue, under every line.
  '#f0ecd8': '#2b2b23', // farmland
  '#e8edcf': '#2c3a24', // grass
  '#d6e5c3': '#1f3b24', // wood
  '#d5e6da': '#1f3f3a', // wetland
  '#efe6c8': '#4a3f28', // sand
  '#e6e3dc': '#3a3935', // rock
  '#ece7dc': '#2c2c29', // residential
  // Water: a mid blue; its lines and labels lighter, to read on it.
  '#a9d3e8': '#2f5f80', // water
  '#4f93b8': '#62aee0', // streams
  '#7fb2cf': '#a2cdea', // glacier edges
  '#1f6f9c': '#86c8f0', // rapids
  '#3d7fa6': '#a0d0f0', // crevasses, glacier names
  '#2d6f96': '#92ccf2', // water names
  // Contours: a light brown, the index ones heavier by width and opacity.
  '#a8703f': '#c99562',
  '#8a5728': '#e0b07c', // contour labels
  // Trails and routes: a bright red that holds up on dark.
  '#c2122d': '#ff5a6c',
  '#f3c8c0': '#5c2730', // route casing: dark, to set the route off
  '#8d0d21': '#ff98a2', // trail and route names
  '#8a2b2b': '#ff8f86', // closed trails
  // Roads: saturated fills on a dark casing.
  '#e9a35f': '#e8994c', // motorway, trunk
  '#f3cd86': '#e8c06a', // primary, secondary
  '#ffffff': '#cdc8bd', // minor roads (ice is set apart below)
  '#5b5146': '#0e0e0c', // road casing
  // Tracks and paths: light, so they read against woods and contours.
  '#58432a': '#c8a678', // tracks
  '#8d877b': '#bdb7aa', // footways
  '#6f6559': '#aaa194', // urban paths
  '#7c7a74': '#aeaca4', // unofficial trails
  // Structures and lines.
  '#55504a': '#aaa49b', // rail, dams
  '#6a6257': '#aaa295', // barriers
  '#8a8173': '#928a7e', // barrier fills
  '#7c8a62': '#acbc8e', // cutlines
  '#6b6673': '#aaa4b2', // pipelines
  '#5d5a78': '#a8a4d2', // power lines
  '#a39c8e': '#4c4942', // buildings
  '#9a9488': '#8e897e', // aeroways
  '#4b4037': '#d2c2ae', // cliffs
  // Boundaries and land status.
  '#6e5f79': '#bba9cc', // state lines and names
  '#7fa35a': '#86c460', // parks
  '#5f8a44': '#93cc70', // park outlines
  '#476a2f': '#acd88c', // park names
  '#b0895a': '#d0a870', // aboriginal lands
  '#8a6a3f': '#d0a870', // their outlines
  '#c9483b': '#e8604c', // danger areas
  '#a8332a': '#ff806c', // their outlines and names
  '#7a6b59': '#b6a690', // military land
  '#cfe0ee': '#8ea9c6', // winter sports areas
  // Labels: light, on dark halos.
  '#2f2a22': '#ece6da', // places, roads, POIs
  '#3b2a18': '#f2dcbc', // peaks and passes
  '#5c4a33': '#d8c4a4', // ridges, valleys
  '#6b5a43': '#ccb894', // ranges
};

/// Colors that differ by layer from [topoPalette]'s for the same light color.
const _byLayer = <String, String>{
  // Glaciers are white in the light; a bright, cool grey here, not glaring.
  'landcover-ice': '#8b939c',
};

/// Opacities changed for the dark ground: faint outlines raised so they read, and the
/// park tint kept faint, as it lies over forest and would lift it under the contours.
const _opacities = <String, Map<String, Object>>{
  'park': {'fill-opacity': 0.06},
  'aboriginal-lands': {'fill-opacity': 0.12},
  'danger-land': {'fill-opacity': 0.2},
  'park-outline': {'line-opacity': 0.7},
  'winter-sports': {'fill-opacity': 0.25},
};

/// A light disc under each peak's triangle: the triangle is a dark sprite, which the
/// style can't recolor, and would vanish on the dark ground.
Map<String, Object?> _peakMarker(Map<String, Object?> peak) => {
  'id': 'peak-marker',
  'type': 'circle',
  'source': peak['source'],
  'source-layer': peak['source-layer'],
  'minzoom': peak['minzoom'],
  'filter': peak['filter'],
  'paint': {
    'circle-color': '#e6d6bc',
    'circle-radius': [
      'interpolate',
      ['linear'],
      ['zoom'],
      9,
      4,
      14,
      5.5,
    ],
    'circle-stroke-color': _ground,
    'circle-stroke-width': 1,
  },
};

/// Makes [style], the light Cardinal Topo, dark.
void darkenTopo(Map<String, Object?> style) {
  style['name'] = '${style['name']} (dark)';
  final metadata = (style['metadata'] ??= <String, Object?>{}) as Map;
  metadata['cardinal:dark'] = 'Derived from the light style by tool/dark_topo_style.dart; edit that, not this.';
  for (final layer in (style['layers'] as List).cast<Map<String, Object?>>()) {
    final id = layer['id'] as String;
    final paint = layer['paint'] as Map<String, Object?>?;
    if (paint == null) continue;
    for (final key in paint.keys.toList()) {
      if (!key.endsWith('-color')) continue;
      final byLayer = _byLayer[id];
      paint[key] = byLayer ?? _map(paint[key], '$id.$key');
    }
    paint.addAll(_opacities[id] ?? const {});
  }
  final layers = (style['layers'] as List).cast<Map<String, Object?>>();
  final peak = layers.indexWhere((l) => l['id'] == 'peak');
  if (peak >= 0) layers.insert(peak, _peakMarker(layers[peak]));
}

Object? _map(Object? value, String where) => switch (value) {
  final String s when s.startsWith('#') =>
    topoPalette[s.toLowerCase()] ??
        (throw StateError('$where: $s has no dark color in topoPalette')),
  final List<Object?> l => [for (final v in l) _map(v, where)],
  final Map<String, Object?> m => {
    for (final MapEntry(:key, :value) in m.entries) key: _map(value, where),
  },
  _ => value,
};
