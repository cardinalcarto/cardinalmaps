/// The places checked in the Saved Places layer, as markers on the map: a GeoJSON source
/// fed from the store, and markers drawn when first needed and added to the map's sprite.
library;

import 'dart:async';
import 'dart:ui' as ui;

import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../saved/saved_names.dart';
import '../store/store.dart';

/// The style's GeoJSON source for the checked saved places.
const savedSource = 'saved';

/// The feature property naming the place's ID, for opening it when it's tapped.
const savedPlaceProperty = 'place';

const _empty = {'type': 'FeatureCollection', 'features': <Object>[]};

/// A marker's size, in logical pixels: room for the disc and its shadow.
const _markerSize = 34.0;
const _discRadius = 14.0;
const _emojiSize = 17.0;

/// Adds the saved places' source and layer to [style], a MapLibre style, on top of what's
/// there. Before the layers for the selected place, which stays above them.
void addSavedLayers(Map<String, Object?> style) {
  (style['sources'] as Map<String, Object?>)[savedSource] = {
    'type': 'geojson',
    'data': _empty,
  };
  final layers = style['layers'] as List<Object?>;
  layers.add({
    'id': 'saved-markers',
    'type': 'symbol',
    'source': savedSource,
    'layout': {
      // Earlier in Saved goes first: it wins the room for a name, and it's on top.
      'symbol-sort-key': ['get', 'order'],
      'icon-image': ['get', 'icon'],
      'icon-allow-overlap': true,
      'text-field': ['get', 'name'],
      'text-font': _styleFont(layers),
      'text-size': 12,
      'text-anchor': 'top',
      'text-offset': [0, 1.3],
      // The marker always shows; its name, where there's room.
      'text-optional': true,
    },
    'paint': {
      'text-color': '#202124',
      'text-halo-color': '#ffffff',
      'text-halo-width': 1.5,
    },
  });
}

/// The first plain font list any of the style's [layers] uses, which it's sure to have.
List<Object?> _styleFont(List<Object?> layers) {
  for (final layer in layers.cast<Map<String, Object?>>()) {
    final layout = layer['layout'];
    if (layout is! Map<String, Object?>) continue;
    final font = layout['text-font'];
    if (font is List<Object?> &&
        font.isNotEmpty &&
        font.every((f) => f is String && f.contains(' '))) {
      return font;
    }
  }
  return const ['Noto Sans Regular'];
}

/// The sprite image's name for a marker of [emoji], or the plain marker if null.
String markerName(String? emoji) => emoji == null
    ? 'saved_marker'
    : 'saved_marker_${emoji.runes.map((r) => r.toRadixString(16)).join('_')}';

/// The GeoJSON for what's checked in [tree].
Map<String, Object?> savedGeoJson(SavedTree tree) => {
  'type': 'FeatureCollection',
  'features': [
    for (final (order, place) in tree.shownPlaces.indexed)
      {
        'type': 'Feature',
        'id': place.entryId,
        'properties': {
          savedPlaceProperty: place.id,
          'name': savedName(place),
          'icon': markerName(place.emoji),
          'order': order,
        },
        'geometry': {
          'type': 'Point',
          'coordinates': [place.place.center.lon, place.place.center.lat],
        },
      },
  ],
};

/// Draws the marker for [emoji], or the plain one if null, as a sprite image.
typedef MarkerPainter = Future<SpriteImageInput> Function(String? emoji);

/// Keeps the map's saved places in step with the store.
class SavedMapLayer {
  SavedMapLayer({
    required this.store,
    required this.map,
    required this.painter,
  });

  final SavedStore store;
  final SparkleMapController map;
  final MarkerPainter painter;

  StreamSubscription<SavedTree>? _watch;

  /// The markers the map has.
  final _painted = <String>{};

  /// The newest tree not shown yet, while one's being shown.
  SavedTree? _next;
  bool _showing = false;

  void start() => _watch ??= store.watch().listen(_changed);

  void dispose() => _watch?.cancel();

  void _changed(SavedTree tree) {
    _next = tree;
    if (!_showing) unawaited(_showNewest());
  }

  /// Shows trees one at a time, skipping any that a newer one replaced while the
  /// markers were being drawn.
  Future<void> _showNewest() async {
    _showing = true;
    try {
      while (_next != null) {
        final tree = _next!;
        _next = null;
        await _show(tree);
      }
    } finally {
      _showing = false;
    }
  }

  Future<void> _show(SavedTree tree) async {
    final places = tree.shownPlaces;
    final missing = {
      for (final place in places)
        if (!_painted.contains(markerName(place.emoji))) place.emoji,
    };
    if (missing.isNotEmpty) {
      final images = await Future.wait(missing.map(painter));
      map.addImages(images);
      _painted.addAll(images.map((image) => image.name));
    }
    map.setGeoJson(savedSource, savedGeoJson(tree));
  }
}

/// A [MarkerPainter] that draws with the platform's fonts at [pixelRatio]: the emoji, or
/// a star, on a white disc with a soft shadow.
MarkerPainter discMarkerPainter(double pixelRatio) => (emoji) async {
  final size = (_markerSize * pixelRatio).ceil();
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder)..scale(pixelRatio);
  const center = Offset(_markerSize / 2, _markerSize / 2);
  canvas
    ..drawCircle(
      center.translate(0, 1),
      _discRadius,
      Paint()
        ..color = const Color(0x40000000)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 1.5),
    )
    ..drawCircle(center, _discRadius, Paint()..color = Colors.white);
  final painter = TextPainter(
    textDirection: TextDirection.ltr,
    text: TextSpan(
      text: emoji ?? String.fromCharCode(Icons.star.codePoint),
      style: TextStyle(
        fontFamily: emoji == null ? Icons.star.fontFamily : null,
        fontSize: _emojiSize,
        // Emoji keep their colors, but draw with this one's opacity.
        color: emoji == null ? const Color(0xFFF9AB00) : Colors.black,
      ),
    ),
  )..layout();
  painter.paint(canvas, center - Offset(painter.width / 2, painter.height / 2));
  painter.dispose();
  final picture = recorder.endRecording();
  final image = await picture.toImage(size, size);
  picture.dispose();
  // Premultiplied, as sprites are.
  final bytes = await image.toByteData();
  image.dispose();
  return SpriteImageInput(
    name: markerName(emoji),
    pixels: bytes!.buffer.asUint8List(bytes.offsetInBytes, bytes.lengthInBytes),
    width: size,
    height: size,
    pixelRatio: pixelRatio,
  );
};
