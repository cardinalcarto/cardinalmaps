// Derives the dark streets style from the light one:
//
//     dart run tool/dark_style.dart [light.json] [dark.json]
//
// Rerun it whenever the light style changes. Colors are remapped in OKLab by what they
// paint, keeping their hue: the ground goes dark, roads lighter than it (major ones
// most), casings to just under the ground, labels light with dark halos. Highway
// shields keep their colors, as their text is drawn on the (light) shield sprites.
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;

const _defaultLight = 'assets/map/headway.json';
const _defaultDark = 'assets/map/headway-dark.json';

void main(List<String> args) {
  final light = args.isNotEmpty ? args[0] : _defaultLight;
  final dark = args.length > 1 ? args[1] : _defaultDark;
  final style =
      jsonDecode(File(light).readAsStringSync()) as Map<String, Object?>;
  darken(style);
  // Through a symlink, as assets/map's styles are, write the file it points at.
  final out = File(dark);
  final target = FileSystemEntity.isLinkSync(dark)
      ? File(out.resolveSymbolicLinksSync())
      : out;
  target.writeAsStringSync(
    '${const JsonEncoder.withIndent('  ').convert(style)}\n',
  );
}

/// Makes [style], a light streets style, dark.
void darken(Map<String, Object?> style) {
  style['id'] = '${style['id']}-dark';
  style['name'] = '${style['name']} (dark)';
  final metadata = (style['metadata'] ??= <String, Object?>{}) as Map;
  metadata['cardinal:dark'] = 'Derived from the light style by tool/dark_style.dart; edit that, not this.';
  for (final layer in (style['layers'] as List).cast<Map<String, Object?>>()) {
    _darkenLayer(layer);
  }
}

/// What a color paints, which decides how it darkens.
enum Role { ground, green, water, waterway, road, casing, text, halo }

void _darkenLayer(Map<String, Object?> layer) {
  final id = layer['id'] as String;
  final type = layer['type'] as String;
  final paint = layer['paint'] as Map<String, Object?>?;
  if (paint == null || id == 'road_shield') return;

  if (type == 'raster') {
    // Low-zoom relief: dimmed and greyed toward the dark ground.
    paint['raster-brightness-max'] = 0.35;
    paint['raster-saturation'] = -0.4;
    return;
  }
  if (id == 'park') {
    // The park layer is protected areas only (national parks to conservation
    // easements; planetiler-openmaptiles' Park.java), which cover whole deserts
    // and ranges: a faint tint, its outline marking where it is. City parks are
    // landcover (see _landcoverGrass).
    // Overlapping ones (a national park and its wildernesses) stack, so the tint is
    // barely green: the outline says what it is.
    paint['fill-opacity'] = 0.12;
    paint['fill-color'] = _format(const _Lab(0.38, -0.025, 0.025), 1);
    paint['fill-outline-color'] = _format(const _Lab(0.45, -0.04, 0.04), 1);
    return;
  }
  if (id == 'landcover_grass') {
    paint['fill-color'] = _landcoverGrass(paint['fill-color']);
    return;
  }
  if (paint.containsKey('fill-pattern')) {
    // The pattern sprite is light; a flat fill like a path's instead.
    paint.remove('fill-pattern');
    paint['fill-color'] = _format(_Lab(0.27, 0, 0), 1);
    return;
  }

  final roadish =
      (id.startsWith('road_') ||
          id.startsWith('tunnel_') ||
          id.startsWith('bridge_')) &&
      !id.contains('rail') &&
      (type == 'line' || type == 'circle');

  for (final key in paint.keys.toList()) {
    final Role role;
    if (key == 'text-color' || key == 'icon-color') {
      role = Role.text;
    } else if (key == 'text-halo-color' || key == 'icon-halo-color') {
      role = Role.halo;
    } else if (!key.endsWith('-color')) {
      continue;
    } else if (type == 'fill-extrusion') {
      // Lit walls and roofs come out lighter than their color: kept near the ground,
      // so streets stay the lightest thing on a block.
      paint[key] = _format(_Lab(_darkGround + 0.04, _neutral.a, _neutral.b), 1);
      continue;
    } else if (id == 'landcover_wood') {
      paint[key] = _forest;
      continue;
    } else if (id == 'landcover_sand') {
      // Beaches and dunes: a warm tan, a step lighter than scrub's.
      paint[key] = _format(const _Lab(0.45, 0.015, 0.06), 1);
      continue;
    } else if (id == 'landcover_ice') {
      paint[key] = _ice;
      continue;
    } else if (id == 'road_crosswalk') {
      paint[key] = _format(_Lab(0.48, 0, 0), 1);
      continue;
    } else if (roadish &&
        (id.contains('casing') || key == 'circle-stroke-color')) {
      role = Role.casing;
    } else if (roadish) {
      role = Role.road;
    } else if (id.startsWith('waterway')) {
      role = Role.waterway;
    } else if (id == 'water') {
      role = Role.water;
    } else if (id == 'park') {
      role = Role.green;
    } else {
      role = Role.ground;
    }
    if (id == 'landcover' && key == 'fill-color') {
      paint[key] = _lowZoomLandcover(_mapColors(paint[key], role) as Object);
      continue;
    }
    paint[key] = _mapColors(
      paint[key],
      role,
      lift: role == Role.road ? _roadLift(id) : 0,
    );
  }
}

/// Landcover's `grass` class lumps desert scrub and city parks in with grass
/// (planetiler-openmaptiles' `FieldMappings.Class`), all one green in the light style:
/// here scrub is dry tan, parks and gardens a strong green, grass and meadows
/// [_grassland].
Object _landcoverGrass(Object? light) => [
  'match',
  ['get', 'subclass'],
  ['scrub', 'shrubbery', 'heath', 'fell', 'tundra'],
  _format(const _Lab(0.4, 0.012, 0.05), 0.6),
  [
    'park',
    'garden',
    'village_green',
    'recreation_ground',
    'golf_course',
    'allotments',
    'flowerbed',
  ],
  _cityPark,
  _grassland,
];

/// Forests: a vivid green, nearly opaque. The light style draws them at 30% over its
/// cream, which over the dark ground left them dim.
final _forest = _format(const _Lab(0.5, -0.12, 0.1), 0.9);

/// Grassland and meadows (California's hills, say): a softer, yellower green, a
/// step below [_forest].
final _grassland = _format(const _Lab(0.44, -0.075, 0.085), 0.8);

/// City parks and gardens: between grassland and forest.
final _cityPark = _format(const _Lab(0.47, -0.1, 0.09), 0.85);

/// Glaciers, ice and snow. They're barely off-white in the light, which the ground's
/// mapping takes to the ground itself: a bright, cool grey instead.
final _ice = _format(const _Lab(0.66, -0.005, -0.015), 0.85);

/// [color], the low-zoom landcover's `match` on class, drawn as the higher zooms'
/// layers draw the same ground, so zooming in doesn't change it.
Object _lowZoomLandcover(Object color) {
  if (color is! List || color.firstOrNull != 'match') return color;
  final classes = {
    'snow': _ice,
    'trees': _forest,
    'grass': _grassland,
    'shrub': _grassland,
  };
  return [
    for (var i = 0; i < color.length; i++)
      i > 0 && classes.containsKey(color[i - 1])
          ? classes[color[i - 1]]
          : color[i],
  ];
}

/// How much lighter than its light color says a road class is drawn. The light style
/// tells motorways apart by their dark casings, which go to the ground here; so the
/// dark one draws them lightest instead, and trunks a step down.
double _roadLift(String id) => id.contains('motorway')
    ? 0.16
    : id.contains('trunk')
    ? 0.08
    : 0;

/// [value] with every color in it, however deep in an expression, darkened for [role].
Object? _mapColors(
  Object? value,
  Role role, {
  double lift = 0,
}) => switch (value) {
  final String s => _parse(s).map((c) => _darkColor(c, role, lift: lift)) ?? s,
  final List<Object?> l => [for (final v in l) _mapColors(v, role, lift: lift)],
  final Map<String, Object?> m => {
    for (final MapEntry(:key, :value) in m.entries)
      key: _mapColors(value, role, lift: lift),
  },
  _ => value,
};

/// The light ground's lightness, and the dark one's.
const _lightGround = 0.95;
const _darkGround = 0.23;

/// How far from grey a color must be to keep its hue: nearer, it's taken for a neutral
/// (the light style's creams and greys) and goes [_neutral]'s slightly cool grey.
const _neutralChroma = 0.03;
const _neutral = (a: -0.002, b: -0.008);

String _darkColor((_Lab, double) color, Role role, {double lift = 0}) {
  final (lab, alpha) = color;
  final l = lab.l;
  final (double dl, double chroma) = switch (role) {
    // Lighter ground goes darker, keeping its differences and much of its color:
    // dark mode is about not glaring, not about being grey.
    Role.ground => (_darkGround + (_lightGround - l) * 0.8, 0.6),
    // Parks and vegetation: clearly lighter and green, at a set chroma (see below).
    Role.green => (_darkGround + 0.14 + (_lightGround - l) * 0.6, 1.0),
    // Water: a steel blue, a little lighter than the land so it reads as water.
    Role.water => (_darkGround + 0.14, 1.0),
    // Streams are thin: lighter than water's areas, or they'd vanish.
    Role.waterway => (0.48, 1.0),
    // Roads are drawn darker than the light ground: darker ones are bigger roads,
    // and come out lighter.
    Role.road => (0.31 + (1 - l) * 1.0, 0.6),
    // Casings sit just under the ground, edging roads without outlining them.
    Role.casing => (0.16 + (1 - l) * 0.1, 0.5),
    Role.text => (math.max(0.78, 1 - 0.4 * l), 1.0),
    Role.halo => (0.18, 0.5),
  };
  final c = math.sqrt(lab.a * lab.a + lab.b * lab.b);
  final neutral = c < _neutralChroma && role != Role.text;
  // Greens and water keep their hue at a set chroma: scaled, the light style's pale
  // tints came out either grey or garish.
  final fixed = switch (role) {
    Role.green => 0.11,
    Role.water => 0.1,
    _ => null,
  };
  final scale = fixed != null && c > 0 ? fixed / c : chroma;
  return _format(
    _Lab(
      (dl + lift).clamp(0.1, 0.96),
      neutral ? _neutral.a : lab.a * scale,
      neutral ? _neutral.b : lab.b * scale,
    ),
    alpha,
  );
}

extension<T extends Object> on T? {
  R? map<R>(R Function(T) f) => this == null ? null : f(this as T);
}

// Colors -------------------------------------------------------------------------

class _Lab {
  const _Lab(this.l, this.a, this.b);
  final double l, a, b;
}

/// A CSS color as OKLab and alpha, or null if [s] isn't one this understands.
(_Lab, double)? _parse(String s) {
  final t = s.trim().toLowerCase();
  List<double>? rgba;
  if (RegExp(r'^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$').hasMatch(t)) {
    var hex = t.substring(1);
    if (hex.length <= 4) hex = hex.split('').map((c) => '$c$c').join();
    final v = [
      for (var i = 0; i < hex.length; i += 2)
        int.parse(hex.substring(i, i + 2), radix: 16) / 255,
    ];
    rgba = [v[0], v[1], v[2], v.length > 3 ? v[3] : 1];
  } else if (RegExp(r'^(rgb|hsl)a?\(').firstMatch(t) case final m?) {
    final parts = t
        .substring(t.indexOf('(') + 1, t.lastIndexOf(')'))
        .split(RegExp(r'[,\s/]+'))
        .where((p) => p.isNotEmpty)
        .toList();
    double number(String p, double percentOf) => p.endsWith('%')
        ? double.parse(p.substring(0, p.length - 1)) / 100 * percentOf
        : double.parse(p);
    final alpha = parts.length > 3 ? number(parts[3], 1) : 1.0;
    if (m.group(1) == 'rgb') {
      rgba = [for (final p in parts.take(3)) number(p, 255) / 255, alpha];
    } else {
      final h = double.parse(parts[0].replaceAll('deg', ''));
      final sat = number(parts[1], 1);
      final light = number(parts[2], 1);
      rgba = [..._hslToRgb(h, sat, light), alpha];
    }
  }
  if (rgba == null) return null;
  return (_toLab(rgba[0], rgba[1], rgba[2]), rgba[3]);
}

List<double> _hslToRgb(double h, double s, double l) {
  final c = (1 - (2 * l - 1).abs()) * s;
  final hp = (h % 360) / 60;
  final x = c * (1 - (hp % 2 - 1).abs());
  final (r, g, b) = switch (hp.floor()) {
    0 => (c, x, 0.0),
    1 => (x, c, 0.0),
    2 => (0.0, c, x),
    3 => (0.0, x, c),
    4 => (x, 0.0, c),
    _ => (c, 0.0, x),
  };
  final m = l - c / 2;
  return [r + m, g + m, b + m];
}

double _toLinear(double c) =>
    c <= 0.04045 ? c / 12.92 : math.pow((c + 0.055) / 1.055, 2.4).toDouble();

double _fromLinear(double c) => c <= 0.0031308
    ? c * 12.92
    : 1.055 * math.pow(c, 1 / 2.4).toDouble() - 0.055;

double _cbrt(double x) =>
    x < 0 ? -math.pow(-x, 1 / 3).toDouble() : math.pow(x, 1 / 3).toDouble();

_Lab _toLab(double r, double g, double b) {
  final (lr, lg, lb) = (_toLinear(r), _toLinear(g), _toLinear(b));
  final l = _cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  final m = _cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  final s = _cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return _Lab(
    0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
  );
}

String _format(_Lab lab, double alpha) {
  final l = lab.l + 0.3963377774 * lab.a + 0.2158037573 * lab.b;
  final m = lab.l - 0.1055613458 * lab.a - 0.0638541728 * lab.b;
  final s = lab.l - 0.0894841775 * lab.a - 1.2914855480 * lab.b;
  final (l3, m3, s3) = (l * l * l, m * m * m, s * s * s);
  final rgb = [
    4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
    -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
    -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3,
  ].map((c) => (_fromLinear(c.clamp(0, 1)) * 255).round()).toList();
  if (alpha >= 1) {
    return '#${rgb.map((c) => c.toRadixString(16).padLeft(2, '0')).join()}';
  }
  return 'rgba(${rgb.join(',')},${alpha.toStringAsFixed(2)})';
}
