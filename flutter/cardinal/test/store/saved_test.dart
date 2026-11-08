import 'package:cardinal/store/store.dart';
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';

Place _place(String id) =>
    Place(id: id, kind: 'poi', name: id, center: const LngLat(0, 0));

void main() {
  late CardinalDatabase db;
  late PlaceStore places;
  late SavedStore saved;

  Future<void> saveAll(List<String> ids) async {
    for (final id in ids) {
      await places.put(_place(id));
      await saved.savePlace(id);
    }
  }

  Future<SavedTree> tree() => saved.watch().first;

  List<String> names(Iterable<SavedItem> items) => [
    for (final item in items)
      switch (item) {
        SavedList(:final name) => name,
        SavedPlace(:final id) => id,
      },
  ];

  setUp(() {
    db = CardinalDatabase(NativeDatabase.memory());
    places = PlaceStore(db);
    saved = SavedStore(db);
  });
  tearDown(() => db.close());

  test('new lists go last and new places first, in their own list', () async {
    final food = await saved.createList('Food');
    final bars = await saved.createList('Bars');
    final italian = await saved.createList('Italian', parent: food);
    await saveAll(['poi:1', 'poi:2']);

    final t = await tree();
    expect(names(t.listsIn(null)), ['Food', 'Bars']);
    expect(names(t.listsIn(food)), ['Italian']);
    expect(names(t.placesIn(null)), ['poi:2', 'poi:1']);
    expect(t.listsIn(bars), isEmpty);
    expect(t.ancestors(italian).map((l) => l.id), [food]);
    expect(t.count(food), 1);
  });

  test('a new list goes after lists that have been reordered', () async {
    final a = await saved.createList('A');
    final b = await saved.createList('B');
    await saved.reorderLists([b, a]);
    await saved.createList('C');
    expect(names((await tree()).listsIn(null)), ['B', 'A', 'C']);
  });

  test('saving what is saved changes nothing', () async {
    await saveAll(['poi:1', 'poi:2']);
    await saved.savePlace('poi:1');
    expect(names((await tree()).placesIn(null)), ['poi:2', 'poi:1']);
  });

  test(
    'moving goes to the end of the destination, keeping its order',
    () async {
      final food = await saved.createList('Food');
      await saveAll(['poi:1', 'poi:2', 'poi:3']);
      await saved.move([(await tree()).copiesOf('poi:3').single], food);
      final t = await tree();
      await saved.move([
        t.copiesOf('poi:2').single,
        t.copiesOf('poi:1').single,
      ], food);

      expect(names((await tree()).placesIn(food)), ['poi:3', 'poi:2', 'poi:1']);
      expect((await tree()).placesIn(null), isEmpty);
    },
  );

  test('a list cannot go inside itself', () async {
    final a = await saved.createList('A');
    final b = await saved.createList('B', parent: a);
    final list = (await tree()).list(a)!;
    await expectLater(saved.move([list], a), throwsA(isA<Exception>()));
    await expectLater(saved.move([list], b), throwsA(isA<Exception>()));
    expect((await tree()).list(a)!.parent, isNull);
  });

  test('restore undoes a move', () async {
    final food = await saved.createList('Food');
    await saveAll(['poi:1', 'poi:2']);
    final before = names((await tree()).placesIn(null));
    final undo = await saved.move([
      (await tree()).copiesOf('poi:1').single,
    ], food);
    await saved.restore(undo);
    expect(names((await tree()).placesIn(null)), before);
    expect((await tree()).placesIn(food), isEmpty);
  });

  test(
    'deleting a list takes what is in it, and restore puts it back',
    () async {
      final a = await saved.createList('A');
      final b = await saved.createList('B', parent: a);
      await saveAll(['poi:1']);
      await saved.move([(await tree()).copiesOf('poi:1').single], b);

      final undo = await saved.delete([(await tree()).list(a)!]);
      var t = await tree();
      expect(t.listsIn(null), isEmpty);
      expect(t.copiesOf('poi:1'), isEmpty);

      await saved.restore(undo);
      t = await tree();
      expect(t.list(a)!.name, 'A');
      expect(t.list(b)!.parent, a);
      expect(t.copiesOf('poi:1').single.parent, b);
    },
  );

  test('reordering a list', () async {
    await saveAll(['poi:1', 'poi:2', 'poi:3']);
    final t = await tree();
    await saved.reorderPlaces([
      for (final id in ['poi:1', 'poi:3', 'poi:2'])
        t.copiesOf(id).single.entryId,
    ]);
    expect(names((await tree()).placesIn(null)), ['poi:1', 'poi:3', 'poi:2']);
  });

  test(
    'editing gives a place a name and note, and blank clears them',
    () async {
      await saveAll(['poi:1']);
      final item = (await tree()).copiesOf('poi:1').single;
      await saved.edit(item, name: 'Home', note: 'Back door');
      var place = (await tree()).copiesOf('poi:1').single;
      expect(place.customName, 'Home');
      expect(place.note, 'Back door');

      await saved.edit(item, name: '  ', note: '');
      place = (await tree()).copiesOf('poi:1').single;
      expect(place.customName, isNull);
      expect(place.note, isNull);
    },
  );

  test('a list keeps its name if a blank one is given', () async {
    final id = await saved.createList('A');
    await saved.edit((await tree()).list(id)!, name: '  ');
    expect((await tree()).list(id)!.name, 'A');
  });

  test('pins', () async {
    await saveAll(['poi:1', 'poi:2']);
    await saved.setPinned([
      (await tree()).copiesOf('poi:1').single.entryId,
    ], true);
    expect((await tree()).pinned.map((p) => p.id), ['poi:1']);
  });

  test('watch tells of changes, and stops when cancelled', () async {
    final seen = <int>[];
    final watching = saved.watch().listen(
      (t) => seen.add(t.listsIn(null).length),
    );
    await pumpEventQueue();
    await saved.createList('A');
    await pumpEventQueue();
    expect(seen.last, 1);
    await watching.cancel();
  });

  group('cycles are refused by the database', () {
    test('however a parent is written', () async {
      final a = await saved.createList('A');
      final b = await saved.createList('B', parent: a);
      final c = await saved.createList('C', parent: b);
      Future<void> setParent(int id, int parent) => db.customStatement(
        'UPDATE saved_lists SET parent_id = $parent WHERE id = $id',
      );
      await expectLater(setParent(a, a), throwsA(isA<Exception>()));
      await expectLater(setParent(a, b), throwsA(isA<Exception>()));
      await expectLater(setParent(a, c), throwsA(isA<Exception>()));
      expect((await tree()).list(a)!.parent, isNull);
    });

    test('including by an undo', () async {
      final a = await saved.createList('A');
      final b = await saved.createList('B', parent: a);
      final undo = await saved.move([(await tree()).list(b)!], null);
      await saved.move([(await tree()).list(a)!], b);
      await expectLater(saved.restore(undo), throwsA(isA<Exception>()));
      final t = await tree();
      expect(t.list(a)!.parent, b);
      expect(t.list(b)!.parent, isNull);
    });

    test('and by a new list that is its own parent', () async {
      await expectLater(
        db.customStatement(
          "INSERT INTO saved_lists (id, parent_id, name, position) "
          "VALUES (99, 99, 'X', 0)",
        ),
        throwsA(isA<Exception>()),
      );
    });

    test('moving to a different branch is fine', () async {
      final a = await saved.createList('A');
      final b = await saved.createList('B', parent: a);
      final c = await saved.createList('C');
      await saved.move([(await tree()).list(b)!], c);
      expect((await tree()).list(b)!.parent, c);
    });
  });

  group('copies', () {
    test('a place can be in several lists, with its name and note', () async {
      final food = await saved.createList('Food');
      await saveAll(['poi:1']);
      final original = (await tree()).copiesOf('poi:1').single;
      await saved.edit(original, name: 'Home', note: 'Back door');
      await saved.copy([(await tree()).copiesOf('poi:1').single], food);

      final t = await tree();
      expect(
        t.copiesOf('poi:1').map((c) => c.parent),
        unorderedEquals([null, food]),
      );
      expect(t.placesIn(food).single.customName, 'Home');
      expect(t.placesIn(food).single.note, 'Back door');
    });

    test('but only once in each, including the top level', () async {
      final food = await saved.createList('Food');
      await saveAll(['poi:1']);
      final copy = (await tree()).copiesOf('poi:1').single;
      await saved.copy([copy], food);
      await saved.copy([copy], food);
      await saved.copy([copy], null);
      await saved.savePlace('poi:1');
      expect((await tree()).copiesOf('poi:1'), hasLength(2));
      await expectLater(
        db.customStatement(
          "INSERT INTO saved_places (place_id, position, saved_at, pinned) "
          "VALUES ('poi:1', 0, 0, 0)",
        ),
        throwsA(isA<Exception>()),
      );
    });

    test('names and notes are each copy\'s own', () async {
      final food = await saved.createList('Food');
      await saveAll(['poi:1']);
      await saved.copy([(await tree()).copiesOf('poi:1').single], food);
      await saved.edit((await tree()).placesIn(food).single, name: 'Lunch');
      final t = await tree();
      expect(t.placesIn(food).single.customName, 'Lunch');
      expect(t.placesIn(null).single.customName, isNull);
    });

    test('undoing a copy takes it away', () async {
      final food = await saved.createList('Food');
      await saveAll(['poi:1']);
      final undo = await saved.copy([
        (await tree()).copiesOf('poi:1').single,
      ], food);
      await saved.restore(undo);
      expect((await tree()).copiesOf('poi:1').single.parent, isNull);
    });

    test('deleting a copy leaves the others', () async {
      final food = await saved.createList('Food');
      await saveAll(['poi:1']);
      await saved.copy([(await tree()).copiesOf('poi:1').single], food);
      await saved.delete([(await tree()).placesIn(null).single]);
      expect((await tree()).copiesOf('poi:1').single.parent, food);
    });

    test('deleting a list leaves copies elsewhere', () async {
      final food = await saved.createList('Food');
      await saveAll(['poi:1']);
      await saved.copy([(await tree()).copiesOf('poi:1').single], food);
      await saved.delete([(await tree()).list(food)!]);
      expect((await tree()).copiesOf('poi:1').single.parent, isNull);
    });

    test(
      'moving where a copy already is joins it, and undo splits them',
      () async {
        final food = await saved.createList('Food');
        await saveAll(['poi:1']);
        await saved.copy([(await tree()).copiesOf('poi:1').single], food);
        final undo = await saved.move([
          (await tree()).placesIn(null).single,
        ], food);
        expect((await tree()).copiesOf('poi:1').single.parent, food);
        await saved.restore(undo);
        expect((await tree()).copiesOf('poi:1'), hasLength(2));
      },
    );

    test('a pin is one copy\'s, and copying doesn\'t bring it', () async {
      final food = await saved.createList('Food');
      await saveAll(['poi:1']);
      final original = (await tree()).copiesOf('poi:1').single;
      await saved.setPinned([original.entryId], true);
      await saved.copy([(await tree()).copiesOf('poi:1').single], food);
      await saved.edit((await tree()).placesIn(food).single, name: 'Lunch');

      var t = await tree();
      expect(t.placesIn(food).single.pinned, isFalse);
      expect(t.pinned.single.parent, isNull);

      await saved.setPinned([t.placesIn(food).single.entryId], true);
      t = await tree();
      expect(t.pinned.map((p) => p.parent), unorderedEquals([null, food]));
    });

    test(
      'setLocations adds and removes at once, and undo reverses both',
      () async {
        final food = await saved.createList('Food');
        final bars = await saved.createList('Bars');
        await saveAll(['poi:1']);
        await saved.edit((await tree()).copiesOf('poi:1').single, name: 'Home');

        final undo = await saved.setLocations('poi:1', {food, bars});
        var t = await tree();
        expect(
          t.copiesOf('poi:1').map((c) => c.parent),
          unorderedEquals([food, bars]),
        );
        expect(t.placesIn(food).single.customName, 'Home');

        await saved.restore(undo);
        t = await tree();
        expect(t.copiesOf('poi:1').single.parent, isNull);
        expect(t.copiesOf('poi:1').single.customName, 'Home');
      },
    );

    test('setLocations to nowhere unsaves it, and keeping what is there changes nothing', () async {
      await saveAll(['poi:1']);
      final entry = (await tree()).copiesOf('poi:1').single.entryId;
      final undo = await saved.setLocations('poi:1', {null});
      expect(undo.added, isEmpty);
      expect(undo.places, isEmpty);
      expect((await tree()).copiesOf('poi:1').single.entryId, entry);

      await saved.setLocations('poi:1', {});
      expect((await tree()).copiesOf('poi:1'), isEmpty);
    });
  });

  group('the map layer', () {
    Future<(int, int)> foodAndItalian() async {
      final food = await saved.createList('Food');
      final italian = await saved.createList('Italian', parent: food);
      await saveAll(['poi:1', 'poi:2']);
      final t = await tree();
      await saved.move([t.copiesOf('poi:1').single], food);
      await saved.move([t.copiesOf('poi:2').single], italian);
      return (food, italian);
    }

    test('starts with everything off', () async {
      final (food, _) = await foodAndItalian();
      final t = await tree();
      expect(t.shownState(food), isFalse);
      expect(t.shownPlaces, isEmpty);
    });

    test('ticking a list turns on all of it, at every level', () async {
      final (food, italian) = await foodAndItalian();
      await saved.setShown((await tree()).list(food)!, true);
      final t = await tree();
      expect(t.shownState(food), isTrue);
      expect(t.shownState(italian), isTrue);
      expect(
        t.shownPlaces.map((p) => p.id),
        unorderedEquals(['poi:1', 'poi:2']),
      );

      await saved.setShown(t.list(food)!, false);
      expect((await tree()).shownPlaces, isEmpty);
    });

    test('one place turns its lists partly on, all the way up', () async {
      final (food, italian) = await foodAndItalian();
      await saved.createList('Bars', parent: food);
      await saved.setShown((await tree()).copiesOf('poi:2').single, true);
      final t = await tree();
      expect(t.shownState(italian), isTrue);
      expect(t.shownState(food), isNull);
      expect(t.shownPlaces.single.id, 'poi:2');
    });

    test('top-level places are on or off one by one', () async {
      await saveAll(['poi:1', 'poi:2']);
      await saved.setShown((await tree()).copiesOf('poi:1').single, true);
      expect((await tree()).shownPlaces.single.id, 'poi:1');
    });

    test('an empty list keeps its own state', () async {
      final id = await saved.createList('Empty');
      await saved.setShown((await tree()).list(id)!, true);
      expect((await tree()).shownState(id), isTrue);
    });

    test('what is new starts on only if all of its list is', () async {
      final (food, italian) = await foodAndItalian();
      await saved.setShown((await tree()).list(italian)!, true);
      await places.put(_place('poi:3'));
      await saved.setLocations('poi:3', {italian, food});
      final t = await tree();
      expect(t.placesIn(italian).first.shownOnMap, isTrue);
      expect(
        t.copiesOf('poi:3').where((c) => c.parent == food).single.shownOnMap,
        isFalse,
      );
      final nested = await saved.createList('Pizza', parent: italian);
      expect((await tree()).shownState(nested), isTrue);
    });

    test(
      'a place shows once, as a copy that is on, with an emoji if any',
      () async {
        final food = await saved.createList('Food');
        final bars = await saved.createList('Bars');
        await saveAll(['poi:1']);
        await saved.setLocations('poi:1', {food, bars});
        var t = await tree();
        final inFood = t.placesIn(food).single;
        final inBars = t.placesIn(bars).single;
        await saved.edit(inBars, name: '', emoji: '🍺');
        await saved.setShown(inFood, true);
        expect((await tree()).shownPlaces.single.emoji, isNull);

        t = await tree();
        await saved.setShown(t.placesIn(bars).single, true);
        expect((await tree()).shownPlaces.single.emoji, '🍺');
      },
    );

    group('moving', () {
      Future<int> shownList() async {
        final id = await saved.createList('Shown');
        await saved.setShown((await tree()).list(id)!, true);
        return id;
      }

      test('into a list that is all on turns a place on', () async {
        final shown = await shownList();
        await saveAll(['poi:1']);
        await saved.move([(await tree()).copiesOf('poi:1').single], shown);
        final t = await tree();
        expect(t.copiesOf('poi:1').single.shownOnMap, isTrue);
        expect(t.shownState(shown), isTrue);
      });

      test('and undoing it turns the place off again', () async {
        final shown = await shownList();
        await saveAll(['poi:1']);
        final undo = await saved.move([
          (await tree()).copiesOf('poi:1').single,
        ], shown);
        await saved.restore(undo);
        final copy = (await tree()).copiesOf('poi:1').single;
        expect(copy.parent, isNull);
        expect(copy.shownOnMap, isFalse);
      });

      test('a list brings what is inside it, and undo too', () async {
        final shown = await shownList();
        final food = await saved.createList('Food');
        final italian = await saved.createList('Italian', parent: food);
        await saveAll(['poi:1']);
        await saved.move([(await tree()).copiesOf('poi:1').single], italian);

        final undo = await saved.move([(await tree()).list(food)!], shown);
        var t = await tree();
        expect(t.shownState(shown), isTrue);
        expect(t.copiesOf('poi:1').single.shownOnMap, isTrue);

        await saved.restore(undo);
        t = await tree();
        expect(t.list(food)!.parent, isNull);
        expect(t.shownState(food), isFalse);
        expect(t.copiesOf('poi:1').single.shownOnMap, isFalse);
      });

      test('into a list that is off leaves a place as it was', () async {
        final off = await saved.createList('Off');
        await saveAll(['poi:1', 'poi:2']);
        var t = await tree();
        await saved.setShown(t.copiesOf('poi:1').single, true);
        t = await tree();
        await saved.move([
          t.copiesOf('poi:1').single,
          t.copiesOf('poi:2').single,
        ], off);
        t = await tree();
        expect(t.copiesOf('poi:1').single.shownOnMap, isTrue);
        expect(t.copiesOf('poi:2').single.shownOnMap, isFalse);
      });

      test('into a partly-on list leaves a place off', () async {
        final mixed = await saved.createList('Mixed');
        await saveAll(['poi:1', 'poi:2', 'poi:3']);
        var t = await tree();
        await saved.move([
          t.copiesOf('poi:1').single,
          t.copiesOf('poi:2').single,
        ], mixed);
        await saved.setShown((await tree()).copiesOf('poi:1').single, true);
        await saved.move([(await tree()).copiesOf('poi:3').single], mixed);
        t = await tree();
        expect(t.shownState(mixed), isNull);
        expect(t.copiesOf('poi:3').single.shownOnMap, isFalse);
      });
    });

    test('places show in the order of Saved, every level opened', () async {
      final a = await saved.createList('A');
      final inner = await saved.createList('Inner', parent: a);
      final b = await saved.createList('B');
      // Newest on top, so saved last to come first at their level.
      await saveAll(['top', 'in-b', 'in-a-2', 'in-a-1', 'in-inner']);
      var t = await tree();
      await saved.move([t.copiesOf('in-inner').single], inner);
      await saved.move([t.copiesOf('in-a-2').single], a);
      await saved.move([(await tree()).copiesOf('in-a-1').single], a);
      await saved.move([(await tree()).copiesOf('in-b').single], b);
      t = await tree();
      for (final list in t.walk) {
        await saved.setShown((await tree()).list(list.list.id)!, true);
      }
      await saved.setShown((await tree()).copiesOf('top').single, true);

      expect((await tree()).shownPlaces.map((p) => p.id), [
        'in-inner',
        'in-a-2',
        'in-a-1',
        'in-b',
        'top',
      ]);
    });

    test('a reordered list changes the order', () async {
      final a = await saved.createList('A');
      final b = await saved.createList('B');
      await saveAll(['one', 'two']);
      var t = await tree();
      await saved.move([t.copiesOf('one').single], a);
      await saved.move([(await tree()).copiesOf('two').single], b);
      t = await tree();
      await saved.setShown(t.list(a)!, true);
      await saved.setShown((await tree()).list(b)!, true);
      await saved.reorderLists([b, a]);
      expect((await tree()).shownPlaces.map((p) => p.id), ['two', 'one']);
    });

    test('copying and adding to a list go to its end too', () async {
      final food = await saved.createList('Food');
      await saveAll(['one', 'two', 'three']);
      final t = await tree();
      await saved.move([t.copiesOf('one').single], food);
      await saved.copy([t.copiesOf('two').single], food);
      await saved.setLocations('three', {null, food});
      expect(names((await tree()).placesIn(food)), ['one', 'two', 'three']);
    });

    test('a newly saved place still goes on top of the top level', () async {
      await saveAll(['one', 'two']);
      expect(names((await tree()).placesIn(null)), ['two', 'one']);
    });

    test('moving several keeps the order they were selected in', () async {
      final food = await saved.createList('Food');
      await saveAll(['one', 'two']);
      final t = await tree();
      await saved.move([
        t.copiesOf('two').single,
        t.copiesOf('one').single,
      ], food);
      expect(names((await tree()).placesIn(food)), ['two', 'one']);
    });

    test('emoji travel with copies, and blank clears them', () async {
      final food = await saved.createList('Food');
      final bars = await saved.createList('Bars');
      await saveAll(['poi:1']);
      await saved.edit(
        (await tree()).copiesOf('poi:1').single,
        name: '',
        emoji: '🍕',
      );
      await saved.copy([(await tree()).placesIn(null).single], food);
      await saved.setLocations('poi:1', {null, food, bars});
      final t = await tree();
      expect(t.copiesOf('poi:1').map((c) => c.emoji), everyElement('🍕'));

      await saved.edit(t.placesIn(food).single, name: '');
      final after = await tree();
      expect(after.placesIn(food).single.emoji, isNull);
      expect(after.placesIn(bars).single.emoji, '🍕');
    });
  });

  test('saved places are kept when others are pruned', () async {
    var now = DateTime(2026);
    places = PlaceStore(db, now: () => now);
    await saveAll(['poi:saved']);
    await places.put(_place('poi:other'));
    now = now.add(const Duration(days: 60));

    expect(await places.prune(), 1);
    expect(await places.get('poi:saved'), isNotNull);
    expect(await places.get('poi:other'), isNull);
  });
}
