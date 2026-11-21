import 'package:cardinal/home/navigation_bar.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/saved/saved_names.dart';
import 'package:cardinal/saved/saved_page.dart';
import 'package:cardinal/search/search_sheet.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoIcons, CupertinoMenuItem;
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

  SheetController sheetOf(WidgetTester tester) =>
      SheetScaffold.controllerOf(tester.element(find.byType(SavedPage).last));

  Future<void> expand(WidgetTester tester) async {
    sheetOf(tester).animateTo(const SheetAnchor.full());
    await tester.pumpAndSettle();
  }

  Future<void> tapAndSettle(WidgetTester tester, Finder finder) async {
    await tester.tap(finder);
    await tester.pumpAndSettle();
  }

  Finder text(String t) => find.text(t);

  group('browsing', () {
    testWidgets('an empty level says so, with nothing to edit', (tester) async {
      await open(tester);
      expect(text('No saved places yet'), findsOneWidget);
      await tapAndSettle(tester, find.byTooltip('More options'));
      expect(
        tester
            .widget<PopupMenuItem<void>>(
              find.widgetWithText(PopupMenuItem<void>, 'Select'),
            )
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
      // No back arrow at the top level; the arrow goes up a list.
      expect(find.byTooltip('Back'), findsNothing);
      await tapAndSettle(tester, text('Food'));
      await tapAndSettle(tester, find.byTooltip('Back'));
      expect(router.state.uri.path, '/saved');
    });

    testWidgets('opens at 40%, and snaps to it, full or collapsed', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await open(tester);
      final sheet = sheetOf(tester);
      expect(sheet.extent, appScreen.height * 0.4);
      // A little way up, it snaps back...
      await tester.drag(text('Saved places').first, const Offset(0, -100));
      await tester.pumpAndSettle();
      expect(sheet.extent, appScreen.height * 0.4);
      // ...most of the way, it's full...
      await tester.drag(text('Saved places').first, const Offset(0, -400));
      await tester.pumpAndSettle();
      expect(sheet.extent, appScreen.height);
      // ...and from 40%, most of the way down, it collapses.
      sheet.animateTo(const SheetAnchor.fraction(0.4));
      await tester.pumpAndSettle();
      await tester.drag(text('Saved places').first, const Offset(0, 250));
      await tester.pumpAndSettle();
      expect(sheet.isCollapsed, isTrue);
    });

    testWidgets('a list opens at the height it was opened from, and back', (
      tester,
    ) async {
      final id = await store.saved.createList('Trips');
      await place('poi:1', name: 'Cafe');
      await open(tester);
      final sheet = sheetOf(tester);
      sheet.animateTo(const SheetAnchor.full());
      await tester.pumpAndSettle();
      await tapAndSettle(tester, text('Trips'));
      expect(
        find.byWidgetPredicate((w) => w is SavedPage && w.listId == id),
        findsOneWidget,
      );
      expect(sheet.extent, appScreen.height);
      sheet.animateTo(const SheetAnchor.fraction(0.4));
      await tester.pumpAndSettle();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(text('Trips'), findsOneWidget);
      expect(sheet.extent, appScreen.height * 0.4);
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
      await expand(tester);
      await tester.drag(find.byType(Scrollable).first, const Offset(0, -5000));
      await tester.pumpAndSettle();
      expect(
        tester.getBottomLeft(text('poi:0')).dy,
        lessThanOrEqualTo(tester.getTopLeft(find.byType(AppNavigationBar)).dy),
      );
    });
  });

  group('editing', () {
    /// Saved, full, with room for every row.
    Future<GoRouter> openFull(
      WidgetTester tester, [
      String at = '/saved',
    ]) async {
      final router = await open(tester, at);
      await expand(tester);
      return router;
    }

    /// Selecting, by the header's ⋮.
    Future<void> startEditing(WidgetTester tester) async {
      await tapAndSettle(tester, find.byTooltip('More options').first);
      await tapAndSettle(tester, text('Select'));
    }

    /// One of the selection's actions: in the contextual bar, or its ⋮.
    Future<void> act(WidgetTester tester, String action) async {
      if (const {'Move', 'Delete', 'Done'}.contains(action)) {
        await tapAndSettle(tester, find.byTooltip(action));
      } else {
        await tapAndSettle(tester, find.byTooltip('More options'));
        await tapAndSettle(tester, text(action));
      }
    }

    /// One of [row]'s ⋮ actions, while browsing.
    Future<void> rowAction(
      WidgetTester tester,
      String row,
      String action,
    ) async {
      await tapAndSettle(
        tester,
        find.descendant(
          of: find.widgetWithText(ListTile, row),
          matching: find.byTooltip('More options'),
        ),
      );
      await tapAndSettle(tester, text(action));
    }

    bool buttonEnabled(WidgetTester tester, String tooltip) =>
        tester
            .widget<IconButton>(
              find.ancestor(
                of: find.byTooltip(tooltip),
                matching: find.byType(IconButton),
              ),
            )
            .onPressed !=
        null;

    /// Whether the open menu's [label] is enabled.
    bool menuEnabled(WidgetTester tester, String label) => tester
        .widget<PopupMenuItem<Object?>>(
          find.ancestor(
            of: text(label),
            matching: find.byWidgetPredicate((w) => w is PopupMenuItem),
          ),
        )
        .enabled;

    testWidgets('selects rows instead of opening them, until Done', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      final router = await openFull(tester);
      await startEditing(tester);
      expect(find.byIcon(Icons.drag_handle), findsOneWidget);

      await tapAndSettle(tester, text('Cafe'));
      expect(text('1 selected'), findsOneWidget);
      expect(router.state.uri.path, '/saved');

      await act(tester, 'Done');
      expect(find.byIcon(Icons.drag_handle), findsNothing);
      expect(find.byType(Checkbox), findsNothing);
    });

    testWidgets('a long press selects, and toggles while selecting', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await place('poi:2', name: 'Bar');
      final router = await openFull(tester);
      await tester.longPress(text('Cafe'));
      await tester.pumpAndSettle();
      expect(text('1 selected'), findsOneWidget);
      expect(router.state.uri.path, '/saved');
      await tester.longPress(text('Bar'));
      await tester.pumpAndSettle();
      expect(text('2 selected'), findsOneWidget);
      await tester.longPress(text('Cafe'));
      await tester.pumpAndSettle();
      expect(text('1 selected'), findsOneWidget);
    });

    testWidgets('selecting, at whatever height the sheet is at', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await open(tester);
      final before = sheetOf(tester).extent;
      await startEditing(tester);
      expect(sheetOf(tester).extent, before);
      await tapAndSettle(tester, text('Cafe'));
      // The selection's actions show at 40%, along the top.
      expect(buttonEnabled(tester, 'Move'), isTrue);
    });

    testWidgets('a row swiped while selecting goes, with Undo', (tester) async {
      await place('poi:1', name: 'Cafe');
      await place('poi:2', name: 'Bar');
      await openFull(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await tester.drag(text('Cafe'), const Offset(-300, 0));
      await tester.pumpAndSettle();
      expect(text('Cafe'), findsNothing);
      expect((await tree()).copiesOf('poi:1'), isEmpty);
      // Out of the selection, and still selecting.
      expect(text('Select items'), findsOneWidget);
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1'), isNotEmpty);
      expect(text('Cafe'), findsOneWidget);
    });

    testWidgets('browsing, a swipe deletes nothing', (tester) async {
      await place('poi:1', name: 'Cafe');
      await openFull(tester);
      await tester.drag(text('Cafe'), const Offset(-300, 0));
      await tester.pumpAndSettle();
      expect(text('Cafe'), findsOneWidget);
      expect((await tree()).copiesOf('poi:1'), isNotEmpty);
      expect(find.byType(Dismissible), findsNothing);
    });

    testWidgets('back stops editing before it leaves the page', (tester) async {
      await place('poi:1');
      await openFull(tester);
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
      await openFull(tester);
      await startEditing(tester);

      expect(buttonEnabled(tester, 'Move'), isFalse);
      expect(buttonEnabled(tester, 'Delete'), isFalse);

      await tapAndSettle(tester, text('Food'));
      expect(buttonEnabled(tester, 'Move'), isTrue);
      expect(buttonEnabled(tester, 'Delete'), isTrue);
    });

    testWidgets('Move to a nested list, and Undo', (tester) async {
      final food = await store.saved.createList('Food');
      final italian = await store.saved.createList('Italian', parent: food);
      await place('poi:1', name: 'Cafe');
      await openFull(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await act(tester, 'Move');

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
      await openFull(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await act(tester, 'Copy');
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
      await openFull(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Food'));
      await tapAndSettle(tester, find.byTooltip('More options'));
      expect(menuEnabled(tester, 'Copy'), isFalse);
    });

    testWidgets('where a place already is, is disabled', (tester) async {
      final food = await store.saved.createList('Food');
      await store.saved.createList('Bars');
      await place('poi:1', name: 'Cafe');
      await store.saved.copy([(await tree()).copiesOf('poi:1').single], food);
      await openFull(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await act(tester, 'Copy');

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
      await openFull(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Food'));
      await act(tester, 'Move');
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
        await openFull(tester, at);
        await startEditing(tester);
      }

      Future<void> pickCafe(WidgetTester tester) async {
        await tapAndSettle(tester, text('Cafe'));
        await act(tester, 'Move');
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
        await openFull(tester, '/saved/$pizza');
        await startEditing(tester);
        await tapAndSettle(tester, text('Slice'));
        await act(tester, 'Move');
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
      await openFull(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Cafe'));
      await act(tester, 'Move');
      await tapAndSettle(tester, text('New list here'));
      await tester.enterText(find.byType(TextField).last, 'Coffee');
      await tester.pump();
      await tapAndSettle(tester, text('Create'));
      await tapAndSettle(tester, text('Move here'));

      final t = await tree();
      expect(t.copiesOf('poi:1').single.parent, t.listsIn(null).single.id);
    });

    testWidgets('a place\'s ⋮ pins it, and unpins it, without selecting', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await openFull(tester);
      await rowAction(tester, 'Cafe', 'Pin');
      expect((await tree()).pinned, hasLength(1));
      expect(text('Select items'), findsNothing);
      expect(find.byIcon(Icons.push_pin), findsOneWidget);

      await rowAction(tester, 'Cafe', 'Unpin');
      expect((await tree()).pinned, isEmpty);
    });

    testWidgets('pins are per row, and lists have none', (tester) async {
      await store.saved.createList('Food');
      await place('poi:1', name: 'Cafe');
      await place('poi:2', name: 'Bar');
      await openFull(tester);
      await tapAndSettle(
        tester,
        find.descendant(
          of: find.widgetWithText(ListTile, 'Food'),
          matching: find.byTooltip('More options'),
        ),
      );
      expect(text('Pin'), findsNothing);
      expect(text('Copy'), findsNothing);
      expect(text('Edit'), findsOneWidget);
      await tester.tapAt(Offset.zero);
      await tester.pumpAndSettle();

      await rowAction(tester, 'Cafe', 'Pin');
      final pinned = (await tree()).pinned.single;
      expect(savedName(pinned), 'Cafe');
    });

    testWidgets('Edit in the bar is for one selected item, and edits it', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await place('poi:2', name: 'Bar');
      await openFull(tester);
      await startEditing(tester);
      await act(tester, 'Select all');
      await tapAndSettle(tester, find.byTooltip('More options'));
      expect(menuEnabled(tester, 'Edit'), isFalse);
      await tester.tapAt(Offset.zero);
      await tester.pumpAndSettle();

      await tapAndSettle(tester, text('Bar'));
      expect(text('1 selected'), findsOneWidget);
      await act(tester, 'Edit');
      await tester.enterText(find.byType(TextField).first, 'Work');
      await tester.pump();
      await tapAndSettle(tester, text('Save'));
      expect((await tree()).copiesOf('poi:1').single.customName, 'Work');
      expect((await tree()).copiesOf('poi:2').single.customName, isNull);
      expect(text('1 selected'), findsOneWidget);
    });

    testWidgets('Pin pins the places selected, then unpins them', (
      tester,
    ) async {
      await store.saved.createList('Food');
      await place('poi:1', name: 'Cafe');
      await place('poi:2', name: 'Bar');
      await openFull(tester);
      await startEditing(tester);
      await tapAndSettle(tester, text('Food'));
      await tapAndSettle(tester, find.byTooltip('More options'));
      // Not with a list selected.
      expect(menuEnabled(tester, 'Pin'), isFalse);
      await tester.tapAt(Offset.zero);
      await tester.pumpAndSettle();

      await tapAndSettle(tester, text('Food'));
      await tapAndSettle(tester, text('Cafe'));
      await tapAndSettle(tester, text('Bar'));
      await act(tester, 'Pin');
      expect((await tree()).pinned, hasLength(2));
      await act(tester, 'Unpin');
      expect((await tree()).pinned, isEmpty);
    });

    testWidgets('the bar has only what acts on the selection', (tester) async {
      await place('poi:1', name: 'Cafe');
      await openFull(tester);
      await startEditing(tester);
      expect(find.byTooltip('Move'), findsOneWidget);
      expect(find.byTooltip('Delete'), findsOneWidget);
      await tapAndSettle(tester, find.byTooltip('More options'));
      expect(text('Copy'), findsOneWidget);
      expect(text('Edit'), findsOneWidget);
      expect(text('Select all'), findsOneWidget);
    });

    testWidgets('Edit renames, and Delete with Undo', (tester) async {
      await place('poi:1', name: 'Cafe');
      await openFull(tester);
      await rowAction(tester, 'Cafe', 'Edit');
      await tester.enterText(find.byType(TextField).first, 'Work');
      await tester.enterText(find.byType(TextField).at(1), 'Quiet');
      await tester.pump();
      await tapAndSettle(tester, text('Save'));
      expect(text('Work'), findsOneWidget);
      expect(text('Quiet'), findsOneWidget);

      // Deleting goes by way of selecting, with the row selected.
      await rowAction(tester, 'Work', 'Delete');
      expect(text('1 selected'), findsOneWidget);
      expect((await tree()).copiesOf('poi:1'), isNotEmpty);
      await act(tester, 'Delete');
      expect((await tree()).copiesOf('poi:1'), isEmpty);
      expect(text('Work'), findsNothing);
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1'), isNotEmpty);
    });

    group('emoji', () {
      Future<void> editCafe(WidgetTester tester) async {
        await place('poi:1', name: 'Cafe');
        await openFull(tester);
        await rowAction(tester, 'Cafe', 'Edit');
      }

      Finder anyEmoji() => find.widgetWithText(TextField, 'Any emoji');

      testWidgets('a common one is a tap, and shows on the row', (
        tester,
      ) async {
        await editCafe(tester);
        await tapAndSettle(tester, text('🍕'));
        await tapAndSettle(tester, text('Save'));
        expect((await tree()).copiesOf('poi:1').single.emoji, '🍕');
        expect(text('🍕'), findsOneWidget);
      });

      testWidgets('the field keeps only the last emoji typed', (tester) async {
        await editCafe(tester);
        await tester.enterText(anyEmoji(), 'abc🦄');
        await tester.pump();
        expect(tester.widget<TextField>(anyEmoji()).controller!.text, '🦄');
        await tester.enterText(anyEmoji(), 'abc');
        await tester.pump();
        expect(tester.widget<TextField>(anyEmoji()).controller!.text, '');
        await tester.enterText(anyEmoji(), '🦄🐙');
        await tester.pump();
        await tapAndSettle(tester, text('Save'));
        expect((await tree()).copiesOf('poi:1').single.emoji, '🐙');
      });

      testWidgets('a typed emoji shows once', (tester) async {
        await editCafe(tester);
        await tester.enterText(anyEmoji(), '🦄');
        await tester.pump();
        expect(text('🦄'), findsOneWidget);
        expect(
          tester.widget<TextField>(anyEmoji()).decoration!.prefixText,
          null,
        );
      });

      testWidgets('one from the keyboard is in the field next time', (
        tester,
      ) async {
        await place('poi:1', name: 'Cafe');
        await store.saved.edit(
          (await tree()).copiesOf('poi:1').single,
          name: '',
          emoji: '🦄',
        );
        await openFull(tester);
        await rowAction(tester, 'Cafe', 'Edit');
        expect(tester.widget<TextField>(anyEmoji()).controller!.text, '🦄');

        // And emptying the field takes it away.
        await tester.enterText(anyEmoji(), '');
        await tester.pump();
        await tapAndSettle(tester, text('Save'));
        expect((await tree()).copiesOf('poi:1').single.emoji, isNull);
      });

      testWidgets('a common one is kept if the field is typed in', (
        tester,
      ) async {
        await editCafe(tester);
        await tapAndSettle(tester, text('🍕'));
        await tester.enterText(anyEmoji(), 'abc');
        await tester.pump();
        await tapAndSettle(tester, text('Save'));
        expect((await tree()).copiesOf('poi:1').single.emoji, '🍕');
      });

      testWidgets('None takes it away', (tester) async {
        await place('poi:1', name: 'Cafe');
        await store.saved.edit(
          (await tree()).copiesOf('poi:1').single,
          name: '',
          emoji: '🍕',
        );
        await openFull(tester);
        await rowAction(tester, 'Cafe', 'Edit');
        await tapAndSettle(tester, text('None'));
        await tapAndSettle(tester, text('Save'));
        expect((await tree()).copiesOf('poi:1').single.emoji, isNull);
      });

      testWidgets('lists don\'t have one', (tester) async {
        await store.saved.createList('Food');
        await openFull(tester);
        await rowAction(tester, 'Food', 'Edit');
        expect(text('Emoji'), findsNothing);
      });
    });

    testWidgets('handles reorder within places', (tester) async {
      await place('poi:1');
      await place('poi:2');
      await place('poi:3');
      await openFull(tester);
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
      await openFull(tester);
      await startEditing(tester);
      await act(tester, 'Select all');
      expect(text('2 selected'), findsOneWidget);
      await act(tester, 'Deselect all');
      expect(text('Select items'), findsOneWidget);
    });
  });

  group('iOS', () {
    /// Saved on iOS, full, with room for every row.
    Future<GoRouter> openIos(
      WidgetTester tester, [
      String at = '/saved',
    ]) async {
      final router = await pumpFakeApp(
        tester,
        store: store,
        location: at,
        platform: TargetPlatform.iOS,
      );
      await expand(tester);
      return router;
    }

    /// A long press frame by frame, as a finger's: the context menu opens as it goes.
    Future<void> hold(WidgetTester tester, Finder finder) async {
      final gesture = await tester.startGesture(tester.getCenter(finder));
      for (var i = 0; i < 60; i++) {
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await tester.pumpAndSettle();
    }

    /// A swipe by [dx], step by step as a finger's, and let go.
    Future<void> swipe(WidgetTester tester, Finder finder, double dx) async {
      final gesture = await tester.startGesture(tester.getCenter(finder));
      const steps = 20;
      for (var i = 0; i < steps; i++) {
        await gesture.moveBy(Offset(dx / steps, 0));
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await tester.pumpAndSettle();
    }

    Future<void> moreMenu(WidgetTester tester, String item) async {
      await tapAndSettle(tester, find.byIcon(CupertinoIcons.ellipsis_circle));
      await tapAndSettle(tester, text(item));
    }

    bool barShown(WidgetTester tester) =>
        find.byType(AppNavigationBar).hitTestable().evaluate().isNotEmpty;

    testWidgets('⋯ selects, the toolbar taking the bar\'s place, until Done', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      final router = await openIos(tester);
      expect(barShown(tester), isTrue);
      await moreMenu(tester, 'Select');
      expect(text('Select items'), findsOneWidget);
      expect(barShown(tester), isFalse);
      expect(text('Move'), findsOneWidget);
      expect(text('Pin'), findsOneWidget);

      await tapAndSettle(tester, text('Cafe'));
      expect(text('1 selected'), findsOneWidget);
      expect(router.state.uri.path, '/saved');
      // All there is, so it's Deselect All.
      await tapAndSettle(tester, text('Deselect All'));
      expect(text('Select items'), findsOneWidget);
      await tapAndSettle(tester, text('Select All'));
      expect(text('1 selected'), findsOneWidget);

      await tapAndSettle(tester, text('Done'));
      expect(text('Select items'), findsNothing);
      expect(barShown(tester), isTrue);
    });

    testWidgets('⋯ makes a new list here', (tester) async {
      await openIos(tester);
      await moreMenu(tester, 'New List');
      await tester.enterText(find.byType(EditableText), 'Trips');
      await tester.pump();
      await tapAndSettle(tester, text('Create'));
      expect((await tree()).listsIn(null).single.name, 'Trips');
    });

    testWidgets('a row\'s long press menu pins, and unpins', (tester) async {
      await place('poi:1', name: 'Cafe');
      final router = await openIos(tester);
      await hold(tester, text('Cafe'));
      expect(router.state.uri.path, '/saved');
      for (final choice in [
        'Pin',
        'Edit',
        'Move',
        'Copy',
        'Select',
        'Delete',
      ]) {
        expect(text(choice), findsOneWidget, reason: choice);
      }
      await tapAndSettle(tester, text('Pin'));
      expect((await tree()).pinned, hasLength(1));

      await hold(tester, text('Cafe'));
      await tapAndSettle(tester, text('Unpin'));
      expect((await tree()).pinned, isEmpty);
    });

    testWidgets('a list\'s menu has no Pin or Copy', (tester) async {
      await store.saved.createList('Food');
      await openIos(tester);
      await hold(tester, text('Food'));
      expect(text('Edit'), findsOneWidget);
      expect(text('Pin'), findsNothing);
      expect(text('Copy'), findsNothing);
    });

    testWidgets('the menu\'s Delete deletes at once, with Undo', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await openIos(tester);
      await hold(tester, text('Cafe'));
      await tapAndSettle(tester, text('Delete'));
      expect((await tree()).copiesOf('poi:1'), isEmpty);
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1'), isNotEmpty);
    });

    testWidgets('the menu\'s Select selects that row', (tester) async {
      await place('poi:1', name: 'Cafe');
      await place('poi:2', name: 'Bar');
      await openIos(tester);
      await hold(tester, text('Bar'));
      await tapAndSettle(tester, text('Select'));
      expect(text('1 selected'), findsOneWidget);
    });

    testWidgets('swiped left all the way, a row goes, with Undo', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await openIos(tester);
      await swipe(tester, text('Cafe'), -380);
      expect(text('Cafe'), findsNothing);
      expect((await tree()).copiesOf('poi:1'), isEmpty);
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1'), isNotEmpty);
    });

    testWidgets('swiped left a little, More has the menu as a sheet', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await openIos(tester);
      await swipe(tester, text('Cafe'), -150);
      expect(find.bySemanticsLabel('Delete'), findsOneWidget);
      await tapAndSettle(tester, find.bySemanticsLabel('More'));
      expect(text('Cancel'), findsOneWidget);
      await tapAndSettle(tester, text('Edit'));
      expect(text('Edit place'), findsOneWidget);
    });

    testWidgets('a place swiped right all the way is pinned, and stays', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await openIos(tester);
      await swipe(tester, text('Cafe'), 380);
      expect((await tree()).pinned, hasLength(1));
      expect(text('Cafe'), findsOneWidget);
    });

    testWidgets('a list swiped right pins nothing', (tester) async {
      await store.saved.createList('Food');
      await openIos(tester);
      await swipe(tester, text('Food'), 150);
      expect(find.bySemanticsLabel('Pin'), findsNothing);
    });

    testWidgets('selecting, a row swiped away goes, with Undo', (tester) async {
      await place('poi:1', name: 'Cafe');
      await place('poi:2', name: 'Bar');
      await openIos(tester);
      await moreMenu(tester, 'Select');
      await swipe(tester, text('Cafe'), -380);
      expect((await tree()).copiesOf('poi:1'), isEmpty);
      expect(text('Select items'), findsOneWidget);
      await tapAndSettle(tester, text('Undo'));
      expect((await tree()).copiesOf('poi:1'), isNotEmpty);
    });

    testWidgets('the toolbar pins the places selected, then unpins them', (
      tester,
    ) async {
      await place('poi:1', name: 'Cafe');
      await place('poi:2', name: 'Bar');
      await openIos(tester);
      await moreMenu(tester, 'Select');
      await tapAndSettle(tester, text('Select All'));
      await tapAndSettle(tester, text('Pin'));
      expect((await tree()).pinned, hasLength(2));
      await tapAndSettle(tester, text('Unpin'));
      expect((await tree()).pinned, isEmpty);
    });

    testWidgets('the back button goes up, and its menu further', (
      tester,
    ) async {
      final food = await store.saved.createList('Food');
      final italian = await store.saved.createList('Italian', parent: food);
      final router = await openIos(tester, '/saved/$food');
      await tapAndSettle(tester, text('Italian'));
      expect(router.state.uri.path, '/saved/$italian');
      // Named for the list above.
      await tapAndSettle(tester, text('Food'));
      expect(router.state.uri.path, '/saved/$food');

      await tapAndSettle(tester, text('Italian'));
      await hold(tester, text('Food'));
      final saved = find.widgetWithText(CupertinoMenuItem, 'Saved');
      expect(saved, findsOneWidget);
      await tapAndSettle(tester, saved);
      expect(router.state.uri.path, '/saved');
    });

    testWidgets('selecting, the sheet can\'t be collapsed', (tester) async {
      await place('poi:1', name: 'Cafe');
      await openIos(tester);
      expect(sheetOf(tester).anchors.first.extent, 0);
      await moreMenu(tester, 'Select');
      expect(sheetOf(tester).anchors.first.extent, greaterThan(0));
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

    testWidgets('Move… into a list that is on shows it on the map', (
      tester,
    ) async {
      final food = await store.saved.createList('Food');
      await store.saved.setShown((await tree()).list(food)!, true);
      await card(tester);
      await tapAndSettle(tester, find.widgetWithText(OutlinedButton, 'Save'));
      await tapAndSettle(tester, text('Move…'));
      await tapAndSettle(tester, text('Food'));
      await tapAndSettle(tester, text('Move here'));
      expect((await tree()).shownPlaces.single.id, 'poi:1');
    });

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
