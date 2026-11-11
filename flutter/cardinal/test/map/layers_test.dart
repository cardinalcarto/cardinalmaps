import 'dart:convert';

import 'package:cardinal/config.dart';
import 'package:cardinal/map/layers.dart';
import 'package:flutter/services.dart';
import 'package:cardinal/store/store.dart';
import 'package:material_ui/material_ui.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../tool/dark_style.dart';
import '../../tool/dark_topo_style.dart';
import '../fakes/fake_place_store.dart';
import '../fakes/fake_saved_store.dart';

Map<String, Object?> _state(Map<String, Object?> style) =>
    style['state']! as Map<String, Object?>;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('the topo style carries the chosen units, and no winter mode', () async {
    final style = await styleFor(
      const MapLayers(style: MapStyle.topo, units: ContourUnits.metric),
    );
    expect(style['name'], 'Cardinal Topo');
    expect(_state(style)['units'], {'default': 'metric'});
    expect(_state(style).containsKey('winter'), false);
    expect(jsonEncode(style), isNot(contains('piste')));
  });

  test('both styles have the selected place on top', () async {
    for (final s in MapStyle.values) {
      final style = await styleFor(MapLayers(style: s));
      final ids = [
        for (final layer in style['layers']! as List) (layer as Map)['id'],
      ];
      expect(ids.last, 'selection-pin', reason: '$s');
    }
  });

  test('a dark app gets the dark streets and topo styles', () async {
    final streets = await styleFor(const MapLayers(dark: true));
    expect(streets['id'], endsWith('-dark'));
    expect((await styleFor(const MapLayers()))['id'], isNot(endsWith('-dark')));
    final topo = await styleFor(
      const MapLayers(style: MapStyle.topo, dark: true),
    );
    expect(topo['name'], 'Cardinal Topo (dark)');
    // Topo's settings still apply in the dark.
    expect(_state(topo)['units'], {'default': 'imperial'});
  });

  test('the bundled dark style is the one made from the light style', () async {
    // Fails if the light style changed and tool/dark_style.dart wasn't rerun.
    final light = jsonDecode(
      await rootBundle.loadString('assets/map/$mapStyle'),
    ) as Map<String, Object?>;
    darken(light);
    final bundled = jsonDecode(
      await rootBundle.loadString('assets/map/$darkMapStyle'),
    );
    expect(jsonDecode(jsonEncode(light)), bundled);
  });

  test(
    'the bundled dark topo style is the one made from the light one',
    () async {
      // Fails if the light topo changed and tool/dark_topo_style.dart wasn't rerun, or
      // if it has a color the dark palette doesn't cover.
      final light = jsonDecode(
        await rootBundle.loadString('assets/map/$topoStyle'),
      ) as Map<String, Object?>;
      darkenTopo(light);
      final bundled = jsonDecode(
        await rootBundle.loadString('assets/map/$darkTopoStyle'),
      );
      expect(jsonDecode(jsonEncode(light)), bundled);
    },
  );

  test('only declared global state is set', () {
    final style = <String, Object?>{
      'state': {
        'units': {'default': 'imperial'},
      },
    };
    setGlobalStateDefaults(style, {'units': 'metric', 'winter': true});
    expect(style['state'], {
      'units': {'default': 'metric'},
    });
    final stateless = <String, Object?>{};
    setGlobalStateDefaults(stateless, {'units': 'metric'});
    expect(stateless, isEmpty);
  });

  Future<SettingsStore> pumpPicker(
    WidgetTester tester, [
    SettingsStore? stored,
  ]) async {
    final settings = stored ?? SettingsStore.inMemory();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.bottomCenter,
            child: LayerPicker(
              settings: settings,
              saved: FakeSavedStore(FakePlaceStore()),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    return settings;
  }

  testWidgets('the picker stores each choice', (tester) async {
    final settings = await pumpPicker(tester);

    await tester.tap(find.text('Topo'));
    await tester.pump();
    expect(await settings.get(mapStyleChoice), MapStyle.topo);

    await tester.tap(find.text('m'));
    await tester.pump();
    expect(await settings.get(contourUnits), ContourUnits.metric);

    await tester.tap(find.text('Streets'));
    await tester.pump();
    expect(await settings.get(mapStyleChoice), MapStyle.streets);
    // Topo's settings are kept for next time.
    expect(await settings.get(contourUnits), ContourUnits.metric);
  });

  testWidgets("choosing a topo setting chooses topo", (tester) async {
    final settings = await pumpPicker(tester);
    expect(await settings.get(mapStyleChoice), MapStyle.streets);
    await tester.tap(find.text('m'));
    await tester.pump();
    expect(await settings.get(mapStyleChoice), MapStyle.topo);
    expect(await settings.get(contourUnits), ContourUnits.metric);
  });

  testWidgets('nothing in the picker moves when the style changes', (
    tester,
  ) async {
    await pumpPicker(tester);
    Rect where(Finder finder) => tester.getRect(finder);
    final before = [where(find.byType(LayerPicker)), where(find.text('m'))];
    await tester.tap(find.text('Topo'));
    await tester.pumpAndSettle();
    expect([where(find.byType(LayerPicker)), where(find.text('m'))], before);
  });

  testWidgets('the picker opens on what was stored', (tester) async {
    final stored = SettingsStore.inMemory();
    await stored.set(mapStyleChoice, MapStyle.topo);
    await stored.set(contourUnits, ContourUnits.metric);
    await pumpPicker(tester, stored);
    final topo = tester.getSemantics(
      find
          .ancestor(of: find.text('Topo'), matching: find.byType(Semantics))
          .first,
    );
    expect(topo, isSemantics(isSelected: true));
    expect(
      tester
          .widget<SegmentedButton<ContourUnits>>(
            find.byType(SegmentedButton<ContourUnits>),
          )
          .selected,
      {ContourUnits.metric},
    );
  });
}
