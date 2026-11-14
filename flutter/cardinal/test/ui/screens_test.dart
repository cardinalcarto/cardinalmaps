import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/map/layers.dart';
import 'package:cardinal/map/location.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/saved/saved_dialogs.dart';
import 'package:cardinal/settings/accessibility.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoAlertDialog,
        CupertinoButton,
        CupertinoIcons,
        CupertinoSearchTextField,
        CupertinoSlidingSegmentedControl,
        CupertinoSwitch;
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_location.dart';
import '../fakes/fake_place_store.dart';
import '../fakes/fake_routes.dart';
import '../fakes/fake_saved_store.dart';

const _ios = TargetPlatform.iOS;

const _place = Place(
  id: 'poi:1',
  kind: 'poi',
  name: 'Chu Minh',
  center: LngLat(1, 4),
);

final _route = fakeRoute(
  [
    [0.5, 2],
    [1, 4],
  ],
  distance: 10460,
  duration: 672,
  steps: const [
    DirectionsStep(instruction: 'Drive north.', distance: 32, duration: 5),
    DirectionsStep(instruction: 'Turn right.', distance: 1000, duration: 60),
  ],
);

Future<Store> _store() async {
  final store = fakeStore();
  await store.places.put(_place);
  return store;
}

void main() {
  group('search on iOS', () {
    testWidgets('is an iOS search field, with Cancel while searching', (
      tester,
    ) async {
      await pumpFakeApp(tester, platform: _ios);
      expect(find.byType(CupertinoSearchTextField), findsOneWidget);
      expect(find.byType(SearchBar), findsNothing);
      expect(find.text('Cancel'), findsNothing);

      await tester.tap(find.byType(CupertinoSearchTextField));
      await tester.pumpAndSettle();
      expect(find.text('Cancel'), findsOneWidget);

      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(find.text('Cancel'), findsNothing);
    });

    testWidgets('Cancel clears the query, so nothing is kept to pull up to', (
      tester,
    ) async {
      await pumpFakeApp(tester, platform: _ios);
      await tester.tap(find.byType(CupertinoSearchTextField));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(EditableText), 'chu minh');
      await tester.pump(const Duration(milliseconds: 300));
      await tester.pumpAndSettle();

      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(find.text('chu minh'), findsNothing);
      final field = tester.widget<CupertinoSearchTextField>(
        find.byType(CupertinoSearchTextField),
      );
      expect(field.controller?.text, isEmpty);

      // The sheet stays at the bar when dragged: there are no results to show.
      final sheet = SheetScaffold.controllerOf(
        tester.element(find.byType(CupertinoSearchTextField)),
      );
      final peek = sheet.extent;
      await tester.drag(
        find.byType(CupertinoSearchTextField),
        const Offset(0, -300),
      );
      await tester.pumpAndSettle();
      expect(sheet.extent, moreOrLessEquals(peek));
    });
  });

  group('the place card on iOS', () {
    testWidgets('closes with a grey circle and acts on a row of buttons', (
      tester,
    ) async {
      final router = await pumpFakeApp(
        tester,
        platform: _ios,
        store: await _store(),
        location: placeLocation(_place.id),
      );
      expect(find.byIcon(CupertinoIcons.xmark), findsOneWidget);
      expect(find.byIcon(Icons.close), findsNothing);
      expect(
        find.ancestor(
          of: find.text('Directions'),
          matching: find.byType(CupertinoButton),
        ),
        findsOneWidget,
      );
      expect(find.text('Save'), findsOneWidget);
      // The actions share the card's width.
      final directions = tester.getSize(
        find.ancestor(
          of: find.text('Directions'),
          matching: find.byType(CupertinoButton),
        ),
      );
      expect(directions.width, greaterThan(appScreen.width / 3));

      await tester.tap(find.bySemanticsLabel('Close'));
      await tester.pumpAndSettle();
      expect(router.state.uri.path, '/');
    });
  });

  group('directions on iOS', () {
    Future<FakeRoutes> open(
      WidgetTester tester, {
      bool wheelchair = false,
    }) async {
      final routes = FakeRoutes(immediately: [_route]);
      final store = await _store();
      await store.settings.set(wheelchairByDefault, wheelchair);
      await pumpFakeApp(
        tester,
        platform: _ios,
        store: store,
        routes: routes,
        locationSource: FakeLocation(
          now: const Fix(lng: 0.5, lat: 2, accuracy: 3),
        ),
        location: placeLocation(_place.id),
      );
      await tester.tap(find.text('Directions'));
      await tester.pumpAndSettle();
      return routes;
    }

    testWidgets('shows the route with Go, its steps, and iOS modes', (
      tester,
    ) async {
      final routes = await open(tester);
      expect(find.text('Go'), findsOneWidget);
      expect(find.text('STEPS'), findsOneWidget);
      expect(find.text('Drive north.'), findsOneWidget);
      expect(
        find.byType(CupertinoSlidingSegmentedControl<TravelMode>),
        findsOneWidget,
      );

      await tester.tap(find.bySemanticsLabel('Walking'));
      await tester.pumpAndSettle();
      expect(routes.asked.last.mode, TravelMode.walking);
    });

    testWidgets('iOS walking is Wheelchair when that is the default', (
      tester,
    ) async {
      final routes = await open(tester, wheelchair: true);
      expect(find.bySemanticsLabel('Walking'), findsNothing);
      await tester.tap(find.bySemanticsLabel('Wheelchair'));
      await tester.pumpAndSettle();
      expect(routes.asked.last.mode, TravelMode.walking);
      expect(routes.asked.last.wheelchair, isTrue);
    });

    testWidgets('chooses an end with an iOS search field and Cancel', (
      tester,
    ) async {
      await open(tester);
      await tester.tap(find.text(_place.name!).last);
      await tester.pumpAndSettle();
      expect(find.byType(CupertinoSearchTextField), findsOneWidget);
      expect(find.text('My Location'), findsOneWidget);

      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(find.byType(CupertinoSearchTextField), findsNothing);
      expect(find.text('Go'), findsOneWidget);
    });
  });

  group('the layer picker on iOS', () {
    testWidgets('is grouped rows with a segmented control', (tester) async {
      final settings = SettingsStore.inMemory();
      await tester.pumpWidget(
        MaterialApp(
          theme: appTheme(Brightness.light, platform: _ios),
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
      expect(find.byType(CupertinoSwitch), findsNothing);
      expect(find.byType(SegmentedButton<ContourUnits>), findsNothing);
      expect(find.byIcon(CupertinoIcons.checkmark), findsOneWidget);

      await tester.tap(find.text('m'));
      await tester.pumpAndSettle();
      expect(await settings.get(contourUnits), ContourUnits.metric);
      // A topo setting chooses topo.
      expect(await settings.get(mapStyleChoice), MapStyle.topo);
    });
  });

  group('saved dialogs on iOS', () {
    Future<void> pump(WidgetTester tester, Widget child) => tester.pumpWidget(
      MaterialApp(
        theme: appTheme(Brightness.light, platform: _ios),
        home: Scaffold(body: Center(child: child)),
      ),
    );

    testWidgets('a new list is named in an iOS alert', (tester) async {
      String? named;
      await pump(
        tester,
        Builder(
          builder: (context) => TextButton(
            onPressed: () async => named = await showAdaptiveDialog<String>(
              context: context,
              builder: (_) => const NewListDialog(),
            ),
            child: const Text('Open'),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(find.byType(CupertinoAlertDialog), findsOneWidget);

      await tester.enterText(find.byType(EditableText), 'Cafés');
      await tester.pump();
      await tester.tap(find.text('Create'));
      await tester.pumpAndSettle();
      expect(named, 'Cafés');
    });

    testWidgets('an item is edited in a form sheet, saved from its bar', (
      tester,
    ) async {
      ({String name, String note, String? emoji})? edited;
      await pump(
        tester,
        Builder(
          builder: (context) => TextButton(
            onPressed: () async => edited =
                await showFormDialog<
                  ({String name, String note, String? emoji})
                >(
                  context,
                  const EditItemDialog(
                    title: 'Edit list',
                    name: 'Cafés',
                    note: null,
                  ),
                ),
            child: const Text('Open'),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      expect(find.text('Edit list'), findsOneWidget);

      await tester.enterText(find.byType(EditableText).first, 'Coffee');
      await tester.pump();
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(edited?.name, 'Coffee');
    });
  });
}
