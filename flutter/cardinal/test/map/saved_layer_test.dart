import 'dart:typed_data';

import 'package:cardinal/map/overlays.dart';
import 'package:cardinal/map/saved_layer.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:sparkle/sparkle.dart';

import '../fakes/fake_map.dart';
import '../fakes/fake_place_store.dart';
import '../fakes/fake_saved_store.dart';

Future<SpriteImageInput> _stubMarker(
  String? emoji, {
  required bool dark,
}) async => SpriteImageInput(
  name: markerName(emoji, dark: dark),
  pixels: Uint8List(4),
  width: 1,
  height: 1,
  pixelRatio: 1,
);

void main() {
  late FakePlaceStore places;
  late FakeSavedStore saved;
  late FakeMap map;
  late MapOverlays overlays;
  late SavedMapLayer layer;

  Future<SavedPlace> add(String id, {String? emoji}) async {
    await places.put(
      Place(id: id, kind: 'poi', name: 'Name $id', center: const LngLat(1, 2)),
    );
    await saved.savePlace(id);
    var copy = (await saved.watch().first).copiesOf(id).single;
    if (emoji != null) await saved.edit(copy, name: '', emoji: emoji);
    copy = (await saved.watch().first).copiesOf(id).single;
    await saved.setShown(copy, true);
    return copy;
  }

  List<Map<String, Object?>> features() =>
      ((map.geoJson[savedSource] as Map)['features'] as List)
          .cast<Map<String, Object?>>();

  setUp(() {
    places = FakePlaceStore();
    saved = FakeSavedStore(places);
    map = FakeMap();
    overlays = MapOverlays();
    layer = SavedMapLayer(
      store: saved,
      map: map,
      overlays: overlays,
      painter: _stubMarker,
    )..start();
  });
  tearDown(() => layer.dispose());

  test(
    'hidden overlays take the places off the map, and they come back',
    () async {
      await add('poi:1');
      await pumpEventQueue();
      expect(features(), hasLength(1));

      final release = overlays.hide();
      await pumpEventQueue();
      expect(features(), isEmpty);

      release();
      await pumpEventQueue();
      expect(features(), hasLength(1));
    },
  );

  test('a place turned on while overlays are hidden waits for them', () async {
    final release = overlays.hide();
    await pumpEventQueue();
    await add('poi:1');
    await pumpEventQueue();
    expect(features(), isEmpty);

    release();
    await pumpEventQueue();
    expect(features(), hasLength(1));
  });

  test('a place that is on becomes a feature with its ID and marker', () async {
    final copy = await add('poi:1', emoji: '🍕');
    await pumpEventQueue();
    final feature = features().single;
    final properties = feature['properties'] as Map;
    expect(feature['id'], copy.entryId);
    expect(properties[savedPlaceProperty], 'poi:1');
    expect(properties['name'], 'Name poi:1');
    expect(properties['icon'], markerName('🍕'));
    expect((feature['geometry'] as Map)['coordinates'], [1.0, 2.0]);
  });

  test('turning it off takes it away', () async {
    final copy = await add('poi:1');
    await pumpEventQueue();
    await saved.setShown(copy, false);
    await pumpEventQueue();
    expect(features(), isEmpty);
  });

  test(
    'a marker is added before the data that uses it, and only once',
    () async {
      await add('poi:1', emoji: '🍕');
      await pumpEventQueue();
      expect(map.images.map((i) => i.name), [
        markerName('🍕'),
        markerName('🍕', dark: true),
      ]);
      expect(
        map.calls.indexOf('images'),
        lessThan(map.calls.lastIndexOf(savedSource)),
      );

      await add('poi:2', emoji: '🍕');
      await pumpEventQueue();
      expect(map.images, hasLength(2));

      await add('poi:3');
      await pumpEventQueue();
      expect(map.images.map((i) => i.name), [
        markerName('🍕'),
        markerName('🍕', dark: true),
        markerName(null),
        markerName(null, dark: true),
      ]);
    },
  );

  test('changes while drawing come down to the newest', () async {
    await add('poi:1');
    await add('poi:2', emoji: '🍕');
    await pumpEventQueue();
    expect(features(), hasLength(2));
  });

  test('earlier places get lower sort keys', () async {
    await add('poi:1');
    await add('poi:2');
    await pumpEventQueue();
    final orders = [
      for (final feature in features()) (feature['properties'] as Map)['order'],
    ];
    expect(orders, [0, 1]);
    // Newest first, as Saved shows them.
    expect(
      (features().first['properties'] as Map)[savedPlaceProperty],
      'poi:2',
    );
  });

  test('the layer sorts by that order', () {
    final style = <String, Object?>{
      'sources': <String, Object?>{},
      'layers': <Object?>[],
    };
    addSavedLayers(style);
    final layout = ((style['layers'] as List).single as Map)['layout'] as Map;
    expect(layout['symbol-sort-key'], ['get', 'order']);
  });

  test('markers are named by every code point of the emoji', () {
    expect(markerName(null), 'saved_marker');
    expect(markerName('🍕'), 'saved_marker_1f355');
    expect(markerName('❤️'), 'saved_marker_2764_fe0f');
    expect(markerName('❤'), isNot(markerName('❤️')));
    expect(markerName('🍕', dark: true), 'saved_marker_1f355_dark');
  });

  test('a dark style uses the dark markers, with light names', () {
    final style = <String, Object?>{
      'sources': <String, Object?>{},
      'layers': <Object?>[],
    };
    addSavedLayers(style, dark: true);
    final layer = (style['layers'] as List).single as Map;
    expect((layer['layout'] as Map)['icon-image'], [
      'concat',
      ['get', 'icon'],
      darkMarkerSuffix,
    ]);
    final paint = layer['paint'] as Map;
    expect(paint['text-color'], '#e8eaed');
    expect(paint['text-halo-color'], '#0f1215');
  });

  test('the layer goes on top, using a font the style has', () {
    final style = <String, Object?>{
      'sources': <String, Object?>{},
      'layers': <Object?>[
        {
          'id': 'label',
          'type': 'symbol',
          'layout': {
            'text-font': ['My Font Regular'],
          },
        },
      ],
    };
    addSavedLayers(style);
    final layer = (style['layers'] as List).last as Map;
    expect(layer['id'], 'saved-markers');
    expect((layer['layout'] as Map)['text-font'], ['My Font Regular']);
    expect((style['sources'] as Map)[savedSource], isNotNull);
  });

  testWidgets('markers are drawn at the screen\'s density, as a disc', (
    tester,
  ) async {
    final image = (await tester.runAsync(
      () => discMarkerPainter(2)('🍕', dark: false),
    ))!;
    expect(image.pixelRatio, 2);
    expect(image.width, 68);
    expect(image.height, 68);
    expect(image.pixels, hasLength(68 * 68 * 4));
    // Inside the disc, off the emoji: opaque white.
    final at = (34 + (34 - 24) * 68) * 4;
    expect(image.pixels.sublist(at, at + 4), [255, 255, 255, 255]);
    // A corner, outside it.
    expect(image.pixels[3], 0);
  });

  testWidgets('dark markers are a gray disc', (tester) async {
    final image = (await tester.runAsync(
      () => discMarkerPainter(2)('🍕', dark: true),
    ))!;
    expect(image.name, markerName('🍕', dark: true));
    final at = (34 + (34 - 24) * 68) * 4;
    expect(image.pixels.sublist(at, at + 4), [0x3c, 0x40, 0x43, 255]);
  });
}
