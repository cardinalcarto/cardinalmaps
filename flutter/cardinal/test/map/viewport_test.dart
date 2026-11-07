import 'package:cardinal/map/viewport.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('the first launch shows the planet', () {
    expect(openingView(null), planetView);
    expect(planetView.zoom, 0);
  });

  test('a view is restored as it was, unless it was closer than z12', () {
    const area = MapViewport(
      lng: -121.76,
      lat: 46.85,
      zoom: 10.5,
      bearing: 30,
      pitch: 40,
    );
    expect(openingView(area), area);
    const streets = MapViewport(
      lng: -122.34,
      lat: 47.61,
      zoom: 17.2,
      bearing: 15,
      pitch: 20,
    );
    expect(
      openingView(streets),
      const MapViewport(
        lng: -122.34,
        lat: 47.61,
        zoom: restoredMaxZoom,
        bearing: 15,
        pitch: 20,
      ),
    );
  });

  test('a view is stored and read back', () async {
    final settings = SettingsStore.inMemory();
    expect(await settings.get(savedViewport), isNull);
    const view = MapViewport(
      lng: 2.29,
      lat: 48.86,
      zoom: 14,
      bearing: 90,
      pitch: 10,
    );
    await settings.set(savedViewport, view);
    expect(await settings.get(savedViewport), view);
  });

  test('a stored view that no longer reads is forgotten', () async {
    final settings = SettingsStore.inMemory();
    await settings.set(Setting.flag('map.viewport', true), true);
    expect(await settings.get(savedViewport), isNull);
  });

  test('parses views as --view takes them', () {
    expect(
      MapViewport.parse('-122.335,47.608,13'),
      const MapViewport(lng: -122.335, lat: 47.608, zoom: 13),
    );
    expect(
      MapViewport.parse('1,2,3,4,5'),
      const MapViewport(lng: 1, lat: 2, zoom: 3, bearing: 4, pitch: 5),
    );
    expect(MapViewport.parse(''), isNull);
    expect(MapViewport.parse('1,2'), isNull);
    expect(MapViewport.parse('a,b,c'), isNull);
    const view = MapViewport(
      lng: 1.5,
      lat: -2.25,
      zoom: 3,
      bearing: 4,
      pitch: 5,
    );
    expect(MapViewport.parse(view.arg), view);
  });
}
