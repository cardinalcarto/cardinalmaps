import 'package:cardinal/map/layers.dart';
import 'package:cardinal/saved/saved_layers_sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';

void main() {
  late Store store;
  late int food;
  late int italian;

  Future<SavedTree> tree() => store.saved.watch().first;

  Future<void> save(String id, String name, int? list) async {
    await store.places.put(
      Place(id: id, kind: 'poi', name: name, center: const LngLat(0, 0)),
    );
    await store.saved.savePlace(id);
    final copy = (await tree()).copiesOf(id).single;
    if (list != null) await store.saved.move([copy], list);
  }

  setUp(() async {
    store = fakeStore();
    food = await store.saved.createList('Food');
    italian = await store.saved.createList('Italian', parent: food);
    await save('poi:1', 'Delancey', italian);
    await save('poi:2', 'Spinasse', italian);
    await save('poi:3', 'Il Corvo', food);
    await save('poi:4', 'Home', null);
  });

  Future<void> pump(
    WidgetTester tester,
    Widget Function(BuildContext) home,
  ) async {
    tester.view
      ..physicalSize = appScreen * 2
      ..devicePixelRatio = 2;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      StoreScope(
        store: store,
        child: MaterialApp(
          home: Scaffold(body: Builder(builder: home)),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  Future<void> open(WidgetTester tester) async {
    await pump(
      tester,
      (context) => TextButton(
        onPressed: () => showSavedLayersSheet(context),
        child: const Text('open'),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
  }

  Future<void> tapAndSettle(WidgetTester tester, Finder finder) async {
    await tester.tap(finder);
    await tester.pumpAndSettle();
  }

  bool? ticked(WidgetTester tester, String label) => tester
      .widget<Checkbox>(
        find.descendant(
          of: find
              .ancestor(of: find.text(label), matching: find.byType(InkWell))
              .first,
          matching: find.byType(Checkbox),
        ),
      )
      .value;

  testWidgets('it fills the screen', (tester) async {
    await open(tester);
    expect(tester.getTopLeft(find.byType(BottomSheet)).dy, 0);
    expect(tester.getSize(find.byType(BottomSheet)).height, appScreen.height);
  });

  testWidgets('its title and Close are below the status bar', (tester) async {
    tester.view
      ..padding = const FakeViewPadding(top: 96)
      ..viewPadding = const FakeViewPadding(top: 96);
    await open(tester);
    expect(
      tester.getTopLeft(find.byTooltip('Close')).dy,
      greaterThanOrEqualTo(48),
    );
    expect(
      tester.getTopLeft(find.text('Saved places on the map')).dy,
      greaterThanOrEqualTo(48),
    );
  });

  testWidgets('lists start folded, with top-level places beside them', (
    tester,
  ) async {
    await open(tester);
    expect(find.text('Food'), findsOneWidget);
    expect(find.text('Home'), findsOneWidget);
    expect(find.text('Italian'), findsNothing);
  });

  testWidgets('the whole hierarchy opens, down to the places', (tester) async {
    await open(tester);
    await tapAndSettle(tester, find.byTooltip('Expand'));
    expect(find.text('Italian'), findsOneWidget);
    expect(find.text('Il Corvo'), findsOneWidget);
    await tapAndSettle(tester, find.byTooltip('Expand'));
    expect(find.text('Delancey'), findsOneWidget);

    await tapAndSettle(tester, find.byTooltip('Collapse').first);
    expect(find.text('Italian'), findsNothing);
  });

  testWidgets('ticking a list turns it all on, and unticking all off', (
    tester,
  ) async {
    await open(tester);
    await tapAndSettle(tester, find.text('Food'));
    var t = await tree();
    expect(t.shownState(food), isTrue);
    expect(
      t.shownPlaces.map((p) => p.id),
      unorderedEquals(['poi:1', 'poi:2', 'poi:3']),
    );
    expect(ticked(tester, 'Food'), isTrue);

    await tapAndSettle(tester, find.text('Food'));
    t = await tree();
    expect(t.shownPlaces, isEmpty);
    expect(ticked(tester, 'Food'), isFalse);
  });

  testWidgets('a single address puts its lists in the partial state', (
    tester,
  ) async {
    await open(tester);
    await tapAndSettle(tester, find.byTooltip('Expand'));
    await tapAndSettle(tester, find.byTooltip('Expand'));
    await tapAndSettle(tester, find.text('Delancey'));

    expect((await tree()).shownPlaces.single.id, 'poi:1');
    expect(ticked(tester, 'Delancey'), isTrue);
    expect(ticked(tester, 'Italian'), isNull);
    expect(ticked(tester, 'Food'), isNull);
  });

  testWidgets('a partly-on list turns all on', (tester) async {
    await store.saved.setShown((await tree()).copiesOf('poi:1').single, true);
    await open(tester);
    await tapAndSettle(tester, find.text('Food'));
    expect((await tree()).shownState(food), isTrue);
  });

  testWidgets('it opens to where the picks are', (tester) async {
    await store.saved.setShown((await tree()).copiesOf('poi:1').single, true);
    await open(tester);
    expect(find.text('Delancey'), findsOneWidget);
    expect(find.text('Spinasse'), findsOneWidget);
  });

  testWidgets('a top-level place has a box of its own', (tester) async {
    await open(tester);
    await tapAndSettle(tester, find.text('Home'));
    expect((await tree()).shownPlaces.single.id, 'poi:4');
  });

  testWidgets('the filter finds lists and places, saying where they are', (
    tester,
  ) async {
    await open(tester);
    await tester.enterText(find.byType(TextField), 'delan');
    await tester.pumpAndSettle();
    expect(find.text('Food › Italian › Delancey'), findsOneWidget);
    expect(find.text('Home'), findsNothing);

    await tester.enterText(find.byType(TextField), 'ital');
    await tester.pumpAndSettle();
    expect(find.text('Food › Italian'), findsOneWidget);

    await tapAndSettle(tester, find.text('Food › Italian'));
    expect((await tree()).shownState(italian), isTrue);
  });

  testWidgets('nothing saved says so', (tester) async {
    store = fakeStore();
    await open(tester);
    expect(find.text('No saved places yet'), findsOneWidget);
  });

  group('from the layer picker', () {
    Future<void> openPicker(WidgetTester tester) async {
      await pump(tester, (context) => const LayerButton());
      await tester.tap(find.byType(LayerButton));
      await tester.pumpAndSettle();
    }

    testWidgets('a row says what is on, and opens the sheet', (tester) async {
      await openPicker(tester);
      expect(find.text('Off'), findsOneWidget);
      await tapAndSettle(tester, find.text('Saved places'));
      expect(find.text('Saved places on the map'), findsOneWidget);

      await tapAndSettle(tester, find.text('Food'));
      await tapAndSettle(tester, find.byTooltip('Close'));
      expect(find.text('Saved places on the map'), findsNothing);
      expect(find.text('3 on'), findsOneWidget);
    });

    testWidgets('back returns to the picker', (tester) async {
      await openPicker(tester);
      await tapAndSettle(tester, find.text('Saved places'));
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.text('Map style'), findsOneWidget);
      expect(find.text('Saved places on the map'), findsNothing);
    });

    testWidgets('the picker stays small', (tester) async {
      await openPicker(tester);
      expect(
        tester.getSize(find.byType(BottomSheet)).height,
        lessThan(appScreen.height * 0.6),
      );
    });
  });
}
