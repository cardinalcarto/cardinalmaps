import 'package:cardinal/home/navigation_bar.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/search/search_sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';

void main() {
  late Store store;

  Future<void> place(String id, {String? name}) async {
    await store.places.put(
      Place(id: id, kind: 'poi', name: name ?? id, center: const LngLat(0, 0)),
    );
    await store.saved.savePlace(id);
  }

  Future<SavedTree> tree() => store.saved.watch().first;

  setUp(() => store = fakeStore());

  Future<GoRouter> open(WidgetTester tester, [String at = '/saved']) =>
      pumpFakeApp(tester, store: store, location: at);

  Future<void> tapAndSettle(WidgetTester tester, Finder finder) async {
    await tester.tap(finder);
    await tester.pumpAndSettle();
  }

  Finder text(String t) => find.text(t);

  group('browsing', () {
    testWidgets('an empty level says so, with nothing to edit', (tester) async {
      await open(tester);
      expect(text('No saved places yet'), findsOneWidget);
      expect(
        tester
            .widget<TextButton>(find.widgetWithText(TextButton, 'Edit'))
            .enabled,
        isFalse,
      );
    });

    testWidgets('lists come before places, with counts', (tester) async {
      await place('poi:1', name: 'Cafe');
      final food = await store.saved.createList('Food');
      await store.saved.createList('Italian', parent: food);
      await open(tester);

      expect(
        tester.getTopLeft(text('Food')).dy,
        lessThan(tester.getTopLeft(text('Cafe')).dy),
      );
      expect(text('1'), findsOneWidget);
    });

    testWidgets('a place opens its card', (tester) async {
      await place('poi:1', name: 'Cafe');
      final router = await open(tester);
      await tapAndSettle(tester, text('Cafe'));
      expect(router.state.uri.toString(), placeLocation('poi:1'));
    });

    testWidgets('lists open as pages, crumbs and back go up', (tester) async {
      final food = await store.saved.createList('Food');
      final italian = await store.saved.createList('Italian', parent: food);
      await store.saved.createList('Pizza', parent: italian);
      final router = await open(tester);

      await tapAndSettle(tester, text('Food'));
      await tapAndSettle(tester, text('Italian'));
      expect(text('Pizza'), findsOneWidget);
      expect(router.state.uri.path, '/saved/$italian');

      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(router.state.uri.path, '/saved/$food');

      await tapAndSettle(tester, text('Italian'));
      await tapAndSettle(tester, text('Saved').first);
      expect(router.state.uri.path, '/saved');
    });

    testWidgets('dragging down leaves the page where it is', (tester) async {
      await place('poi:1', name: 'Cafe');
      await open(tester);
      final top = tester.getTopLeft(text('Saved places').first).dy;
      final gesture = await tester.startGesture(tester.getCenter(text('Cafe')));
      await tester.pump();
      for (var i = 0; i < 6; i++) {
        await gesture.moveBy(const Offset(0, 25));
        await tester.pump(const Duration(milliseconds: 16));
        expect(tester.getTopLeft(text('Saved places').first).dy, top);
      }
      await gesture.up();
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(text('Saved places').first).dy, top);
    });

    testWidgets('back from the top level goes to search', (tester) async {
      await place('poi:1');
      await open(tester);
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byType(SearchSheet), findsOneWidget);
    });

    testWidgets('New list makes one here', (tester) async {
      final food = await store.saved.createList('Food');
      await open(tester, '/saved/$food');

      await tapAndSettle(tester, find.byTooltip('New list'));
      await tester.enterText(find.byType(TextField), 'Italian');
      await tester.pump();
      await tapAndSettle(tester, text('Create'));

      expect((await tree()).listsIn(food).single.name, 'Italian');
      expect(text('Italian'), findsOneWidget);
    });

    testWidgets('the page reaches the bar without rows hiding under it', (
      tester,
    ) async {
      for (var i = 0; i < 30; i++) {
        await place('poi:$i');
      }
      await open(tester);
      await tester.drag(find.byType(Scrollable).first, const Offset(0, -5000));
      await tester.pumpAndSettle();
      expect(
        tester.getBottomLeft(text('poi:0')).dy,
        lessThanOrEqualTo(tester.getTopLeft(find.byType(AppNavigationBar)).dy),
      );
    });
  });

  group('editing', () {
    Future<void> startEditing(WidgetTester tester) async {
      await tapAndSettle(tester, find.widgetWithText(TextButton, 'Edit'));
    }

    testWidgets('selects rows instead of opening them, until Done', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      final router = await open(tester);
      await startEditing(tester);
      expect(find.byIcon(Icons.drag_handle), findsOneWidget);

      await tapAndSettle(tester, text('Cafe'));
      expect(text('1 selected'), findsOneWidget);
      expect(router.state.uri.path, '/saved');

      await tapAndSettle(tester, text('Done'));
      expect(find.byIcon(Icons.drag_handle), findsNothing);
      expect(find.byType(Checkbox), findsNothing);
    });

    testWidgets('back stops editing before it leaves the page', (tester) async {
      await place('poi:1');
      await open(tester);
      await startEditing(tester);

      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.byIcon(Icons.drag_handle), findsNothing);
      expect(find.byType(SearchSheet), findsNothing);
    });

    testWidgets('actions wait for a selection they make sense of', (
      tester,
    ) async {
      await store.saved.createList('Food');
      await open(tester);
      await startEditing(tester);

      bool enabled(String label) => tester
          .widget<TextButton>(
            find.ancestor(
              of: text(label),
              matching: find.byWidgetPredicate((w) => w is TextButton),
            ),
          )
          .enabled;
      expect(enabled('Move'), isFalse);
      expect(enabled('Delete'), isFalse);

      await tapAndSettle(tester, text('Food'));
      expect(enabled('Move'), isTrue);
      expect(enabled('Edit'), isTrue);
      // Only places can be pinned.
      expect(enabled('Pin'), isFalse);
    });

    testWidgets('Move to a nested list, and Undo', (tester) async {
      final food = await store.saved.createList('Food');
      final italian = await store.saved.createList('Italian', parent: food);
      await place('poi:1', name: 'Cafe');
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await tapAndSettle(tester, text('Move'));

      final move = tester.widget<FilledButton>(
        find.widgetWithText(FilledButton, 'Move here'),
      );
      expect(move.enabled, isFalse);
      await tapAndSettle(tester, find.byTooltip('Expand'));
      await tapAndSettle(tester, text('Italian').last);
      await tapAndSettle(tester, text('Move here'));

      expect((await tree()).copiesOf('poi:1').single.parent, italian);
      expect(text('Cafe'), findsNothing);
      expect(text('Moved to Italian'), findsOneWidget);

      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1').single.parent, isNull);
      expect(text('Cafe'), findsOneWidget);
    });

    testWidgets('Copy puts a place in a second list, and Undo takes it out', (
      tester,
    ) async {
      final food = await store.saved.createList('Food');
      await place('poi:1', name: 'Cafe');
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await tapAndSettle(tester, text('Copy'));
      expect(text('Copy 1 item to'), findsOneWidget);
      await tapAndSettle(tester, text('Food').last);
      await tapAndSettle(tester, text('Copy here'));

      expect((await tree()).copiesOf('poi:1'), hasLength(2));
      // It's still here too.
      expect(text('Cafe'), findsOneWidget);
      expect((await tree()).placesIn(food).single.id, 'poi:1');
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1'), hasLength(1));
    });

    testWidgets('lists can\'t be copied', (tester) async {
      await store.saved.createList('Food');
      await place('poi:1', name: 'Cafe');
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Food'));
      final copy = find.ancestor(
        of: text('Copy'),
        matching: find.byWidgetPredicate((w) => w is TextButton),
      );
      expect(tester.widget<TextButton>(copy).enabled, isFalse);
    });

    testWidgets('where a place already is, is disabled', (tester) async {
      final food = await store.saved.createList('Food');
      await store.saved.createList('Bars');
      await place('poi:1', name: 'Cafe');
      await store.saved.copy([(await tree()).copiesOf('poi:1').single], food);
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await tapAndSettle(tester, text('Copy'));

      bool enabled(String label) => tester
          .widget<ListTile>(find.widgetWithText(ListTile, label).last)
          .enabled;
      expect(enabled('Saved'), isFalse);
      expect(enabled('Food'), isFalse);
      expect(enabled('Bars'), isTrue);
    });

    testWidgets('a list can\'t be moved into itself or below', (tester) async {
      final food = await store.saved.createList('Food');
      await store.saved.createList('Italian', parent: food);
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Food'));
      await tapAndSettle(tester, text('Move'));
      await tapAndSettle(tester, find.byTooltip('Expand'));

      bool enabled(String label) => tester
          .widget<ListTile>(find.widgetWithText(ListTile, label).last)
          .enabled;
      expect(enabled('Food'), isFalse);
      expect(enabled('Italian'), isFalse);
      // And it's already at the top level.
      expect(enabled('Saved'), isFalse);
    });

    group('with many lists', () {
      late int food;
      late int italian;

      setUp(() async {
        food = await store.saved.createList('Food');
        italian = await store.saved.createList('Italian', parent: food);
        await store.saved.createList('Pizza', parent: italian);
        await store.saved.createList('Bars');
        await store.saved.createList('Pizza');
      });

      Future<void> openPicker(WidgetTester tester, String at) async {
        await place('poi:1', name: 'Cafe');
        await open(tester, at);
        await startEditing(tester);
      }

      Future<void> pickCafe(WidgetTester tester) async {
        await tapAndSettle(tester, text('Cafe'));
        await tapAndSettle(tester, text('Move'));
      }

      testWidgets('nested lists stay folded until expanded', (tester) async {
        await openPicker(tester, '/saved');
        await pickCafe(tester);
        final sheet = find.byType(ListView);
        Finder inPicker(String t) =>
            find.descendant(of: sheet.last, matching: text(t));
        expect(inPicker('Food'), findsOneWidget);
        expect(inPicker('Italian'), findsNothing);

        await tapAndSettle(tester, find.byTooltip('Expand').first);
        expect(inPicker('Italian'), findsOneWidget);
        await tapAndSettle(tester, find.byTooltip('Collapse'));
        expect(inPicker('Italian'), findsNothing);
      });

      testWidgets('the filter finds nested lists, by their paths', (
        tester,
      ) async {
        await openPicker(tester, '/saved');
        await pickCafe(tester);
        await tester.enterText(find.byType(TextField), 'piz');
        await tester.pumpAndSettle();

        expect(text('Food › Italian › Pizza'), findsOneWidget);
        Finder inSheet(String t) =>
            find.descendant(of: find.byType(BottomSheet), matching: text(t));
        expect(inSheet('Pizza'), findsOneWidget);
        expect(inSheet('Bars'), findsNothing);

        await tapAndSettle(tester, text('Food › Italian › Pizza'));
        await tapAndSettle(tester, text('Move here'));
        final t = await tree();
        expect(t.list(t.copiesOf('poi:1').single.parent!)!.parent, italian);
      });

      testWidgets('a pick hidden by the filter can\'t be moved to', (
        tester,
      ) async {
        await openPicker(tester, '/saved');
        await pickCafe(tester);
        await tapAndSettle(tester, find.widgetWithText(ListTile, 'Bars').last);
        FilledButton button() => tester.widget<FilledButton>(
          find.widgetWithText(FilledButton, 'Move here'),
        );
        expect(button().enabled, isTrue);

        await tester.enterText(find.byType(TextField), 'piz');
        await tester.pumpAndSettle();
        expect(button().enabled, isFalse);

        await tester.enterText(find.byType(TextField), '');
        await tester.pumpAndSettle();
        expect(button().enabled, isTrue);
      });

      testWidgets('the way to where it is starts open', (tester) async {
        final pizza = (await tree()).listsIn(italian).single.id;
        await store.places.put(
          const Place(
            id: 'poi:2',
            kind: 'poi',
            name: 'Slice',
            center: LngLat(0, 0),
          ),
        );
        await store.saved.savePlace('poi:2');
        await store.saved.move([
          (await tree()).copiesOf('poi:2').single,
        ], pizza);
        await open(tester, '/saved/$pizza');
        await startEditing(tester);
        await tapAndSettle(tester, text('Slice'));
        await tapAndSettle(tester, text('Move'));
        expect(find.byTooltip('Collapse'), findsNWidgets(2));
      });

      testWidgets('the sheet doesn\'t change height while filtering', (
        tester,
      ) async {
        await openPicker(tester, '/saved');
        await pickCafe(tester);
        double top() => tester.getTopLeft(text('Move 1 item to')).dy;
        final before = top();
        await tester.enterText(find.byType(TextField), 'zzz');
        await tester.pumpAndSettle();
        expect(top(), before);
      });
    });

    testWidgets('a new list in the picker becomes the destination', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await tapAndSettle(tester, text('Move'));
      await tapAndSettle(tester, text('New list here'));
      await tester.enterText(find.byType(TextField).last, 'Coffee');
      await tester.pump();
      await tapAndSettle(tester, text('Create'));
      await tapAndSettle(tester, text('Move here'));

      final t = await tree();
      expect(t.copiesOf('poi:1').single.parent, t.listsIn(null).single.id);
    });

    testWidgets('Pin puts a chip on the home sheet, and Unpin takes it off', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await tapAndSettle(tester, text('Pin'));
      expect((await tree()).pinned, hasLength(1));

      await tapAndSettle(tester, text('Unpin'));
      expect((await tree()).pinned, isEmpty);
    });

    testWidgets('Edit renames, and Delete with Undo', (tester) async {
      await place('poi:1', name: 'Cafe');
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));

      await tapAndSettle(tester, find.widgetWithText(TextButton, 'Edit').last);
      await tester.enterText(find.byType(TextField).first, 'Work');
      await tester.enterText(find.byType(TextField).last, 'Quiet');
      await tester.pump();
      await tapAndSettle(tester, text('Save'));
      expect(text('Work'), findsOneWidget);
      expect(text('Quiet'), findsOneWidget);

      await tapAndSettle(tester, text('Delete'));
      expect((await tree()).copiesOf('poi:1'), isEmpty);
      expect(text('Work'), findsNothing);
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1'), isNotEmpty);
    });

    testWidgets('handles reorder within places', (tester) async {
      await place('poi:1');
      await place('poi:2');
      await place('poi:3');
      await open(tester);
      await startEditing(tester);

      final handle = find.byIcon(Icons.drag_handle).first;
      final gesture = await tester.startGesture(tester.getCenter(handle));
      await tester.pump();
      for (var i = 0; i < 12; i++) {
        await gesture.moveBy(const Offset(0, 20));
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await tester.pumpAndSettle();

      final order = (await tree()).placesIn(null).map((p) => p.id).toList();
      expect(order.first, isNot('poi:3'));
      expect(order, containsAll(['poi:1', 'poi:2', 'poi:3']));
    });

    testWidgets('selecting everything, then clearing', (tester) async {
      await place('poi:1');
      await place('poi:2');
      await open(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Select all'));
      expect(text('2 selected'), findsOneWidget);
      await tapAndSettle(tester, text('Clear'));
      expect(text('Select items'), findsOneWidget);
    });
  });

  group('the place card', () {
    Future<GoRouter> card(WidgetTester tester) async {
      await store.places.put(
        const Place(
          id: 'poi:1',
          kind: 'poi',
          name: 'Cafe',
          center: LngLat(0, 0),
        ),
      );
      final router = await open(tester, '/');
      router.push(placeLocation('poi:1')).ignore();
      await tester.pumpAndSettle();
      return router;
    }

    testWidgets('Save saves to the top level, and offers to move it', (
      tester,
    ) async {
      await card(tester);
      await tapAndSettle(tester, find.widgetWithText(OutlinedButton, 'Save'));
      expect((await tree()).copiesOf('poi:1').single.parent, isNull);
      expect(find.widgetWithText(OutlinedButton, 'Manage'), findsOneWidget);
      expect(text('Move…'), findsOneWidget);
    });

    testWidgets('Move… files it in a list', (tester) async {
      final food = await store.saved.createList('Food');
      await card(tester);
      await tapAndSettle(tester, find.widgetWithText(OutlinedButton, 'Save'));
      await tapAndSettle(tester, text('Move…'));
      await tapAndSettle(tester, text('Food'));
      await tapAndSettle(tester, text('Move here'));
      expect((await tree()).copiesOf('poi:1').single.parent, food);
    });

    Future<void> saveCafe() async {
      await store.places.put(
        const Place(
          id: 'poi:1',
          kind: 'poi',
          name: 'Cafe',
          center: LngLat(0, 0),
        ),
      );
      await store.saved.savePlace('poi:1');
    }

    Future<void> openManage(WidgetTester tester) async {
      final router = await open(tester, '/');
      router.push(placeLocation('poi:1')).ignore();
      await tester.pumpAndSettle();
      await tapAndSettle(tester, find.widgetWithText(OutlinedButton, 'Manage'));
    }

    Finder row(String label) => find.widgetWithText(CheckboxListTile, label);

    testWidgets('Manage shows only where it is, and adds through the picker', (
      tester,
    ) async {
      await saveCafe();
      final food = await store.saved.createList('Food');
      await store.saved.createList('Bars');
      await openManage(tester);
      expect(tester.widget<CheckboxListTile>(row('Saved')).value, isTrue);
      expect(row('Food'), findsNothing);
      expect(row('Bars'), findsNothing);

      await tapAndSettle(tester, text('Add to list…'));
      await tapAndSettle(tester, find.widgetWithText(ListTile, 'Food').last);
      await tapAndSettle(tester, text('Add here'));
      expect(
        tester.widget<CheckboxListTile>(row('Saved › Food')).value,
        isTrue,
      );
      // Nothing happens until Done.
      expect((await tree()).copiesOf('poi:1'), hasLength(1));

      await tapAndSettle(tester, text('Done'));
      expect(
        (await tree()).copiesOf('poi:1').map((c) => c.parent),
        unorderedEquals([null, food]),
      );
      expect(text('Added to Food'), findsOneWidget);
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1').single.parent, isNull);
    });

    testWidgets('where it already is can\'t be added again', (tester) async {
      await saveCafe();
      final food = await store.saved.createList('Food');
      await store.saved.copy([(await tree()).copiesOf('poi:1').single], food);
      await openManage(tester);
      await tapAndSettle(tester, text('Add to list…'));
      bool enabled(String label) => tester
          .widget<ListTile>(find.widgetWithText(ListTile, label).last)
          .enabled;
      expect(enabled('Saved'), isFalse);
      expect(enabled('Food'), isFalse);
    });

    testWidgets('Manage removes from one list, and Undo puts it back', (
      tester,
    ) async {
      await saveCafe();
      final food = await store.saved.createList('Food');
      await store.saved.copy([(await tree()).copiesOf('poi:1').single], food);
      await openManage(tester);
      await tapAndSettle(tester, row('Saved › Food'));
      await tapAndSettle(tester, text('Done'));

      expect((await tree()).copiesOf('poi:1').single.parent, isNull);
      expect(text('Removed from Food'), findsOneWidget);
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).placesIn(food), hasLength(1));
    });

    testWidgets('unticking everything unsaves it', (tester) async {
      await saveCafe();
      await openManage(tester);
      await tapAndSettle(tester, row('Saved'));
      await tapAndSettle(tester, text('Done'));
      expect((await tree()).copiesOf('poi:1'), isEmpty);
      expect(find.widgetWithText(OutlinedButton, 'Save'), findsOneWidget);
      expect(text('Removed from Saved'), findsOneWidget);
    });

    testWidgets('a new list from the picker is added', (tester) async {
      await saveCafe();
      await openManage(tester);
      await tapAndSettle(tester, text('Add to list…'));
      await tapAndSettle(tester, text('New list here'));
      await tester.enterText(find.byType(TextField).last, 'Coffee');
      await tester.pump();
      await tapAndSettle(tester, text('Create'));
      await tapAndSettle(tester, text('Add here'));
      await tapAndSettle(tester, text('Done'));
      final t = await tree();
      expect(t.placesIn(t.listsIn(null).single.id), hasLength(1));
    });

    testWidgets('dismissing changes nothing', (tester) async {
      await saveCafe();
      await openManage(tester);
      await tester.tap(row('Saved'));
      await tester.pumpAndSettle();
      await tester.tapAt(const Offset(200, 50));
      await tester.pumpAndSettle();
      expect((await tree()).copiesOf('poi:1').single.parent, isNull);
    });
  });

  testWidgets('a pinned copy\'s chip uses that copy\'s name', (tester) async {
    await place('poi:1', name: 'Cafe');
    final food = await store.saved.createList('Food');
    await store.saved.copy([(await tree()).copiesOf('poi:1').single], food);
    final copy = (await tree()).placesIn(food).single;
    await store.saved.edit(copy, name: 'Lunch spot');
    await store.saved.setPinned([copy.entryId], true);
    await open(tester, '/');
    expect(find.widgetWithText(FilledButton, 'Lunch spot'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Cafe'), findsNothing);
  });

  testWidgets('pinned places have chips under the bar, gone while searching', (
    tester,
  ) async {
    await place('poi:1', name: 'Cafe');
    await store.saved.setPinned([
      (await tree()).copiesOf('poi:1').single.entryId,
    ], true);
    final router = await open(tester, '/');
    expect(find.widgetWithText(FilledButton, 'Cafe'), findsOneWidget);

    await tapAndSettle(tester, find.byType(TextField));
    expect(find.widgetWithText(FilledButton, 'Cafe'), findsNothing);
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    await tapAndSettle(tester, find.widgetWithText(FilledButton, 'Cafe'));
    expect(router.state.uri.toString(), placeLocation('poi:1'));
  });
}
