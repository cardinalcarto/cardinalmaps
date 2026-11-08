import 'dart:async';
import 'dart:convert';

import 'package:drift/drift.dart';

import 'database.dart';
import 'place.dart';

/// Something in Saved: a list or a place.
sealed class SavedItem {
  const SavedItem();

  /// The list it's in; null at the top level.
  int? get parent;

  /// The user's note on it, if any.
  String? get note;

  /// Identifies it among all of Saved.
  Object get key;
}

/// A list of saved places and lists.
class SavedList extends SavedItem {
  const SavedList({
    required this.id,
    required this.parent,
    required this.name,
    this.note,
    this.shownOnMap = false,
  });

  final int id;

  @override
  final int? parent;

  final String name;

  @override
  final String? note;

  /// Whether it's on in the map's layer while it's empty: see [SavedTree.shownState].
  final bool shownOnMap;

  @override
  Object get key => ('list', id);
}

/// A place in one list, or at the top level. A place can be in several, each its own
/// [SavedPlace] with its own name and note.
class SavedPlace extends SavedItem {
  const SavedPlace({
    required this.entryId,
    required this.place,
    required this.parent,
    this.customName,
    this.note,
    this.pinned = false,
    this.emoji,
    this.shownOnMap = false,
  });

  /// Identifies this copy of the place.
  final int entryId;

  /// What's known about the place, without its shape, bounds or tags: open it by ID
  /// for all of that.
  final Place place;

  @override
  final int? parent;

  /// The name the user gave it, if any.
  final String? customName;

  @override
  final String? note;

  /// Whether this copy has a chip on the home sheet, under its name.
  final bool pinned;

  /// The emoji it's marked with on the map, if any.
  final String? emoji;

  /// Whether this copy is on in the map's Saved Places layer.
  final bool shownOnMap;

  /// The place's ID, the same for all its copies.
  String get id => place.id;

  @override
  Object get key => ('place', entryId);
}

/// Everything in Saved, in order.
class SavedTree {
  SavedTree({required List<SavedList> lists, required List<SavedPlace> places})
    : _lists = {for (final list in lists) list.id: list},
      _places = places,
      _copies = _copiesOf(places),
      _childLists = _group(lists),
      _childPlaces = _group(places);

  static Map<String, List<SavedPlace>> _copiesOf(List<SavedPlace> places) {
    final copies = <String, List<SavedPlace>>{};
    for (final place in places) {
      (copies[place.id] ??= []).add(place);
    }
    return copies;
  }

  static Map<int?, List<T>> _group<T extends SavedItem>(List<T> items) {
    final grouped = <int?, List<T>>{};
    for (final item in items) {
      (grouped[item.parent] ??= []).add(item);
    }
    return grouped;
  }

  final Map<int, SavedList> _lists;
  final List<SavedPlace> _places;
  final Map<String, List<SavedPlace>> _copies;
  final Map<int?, List<SavedList>> _childLists;
  final Map<int?, List<SavedPlace>> _childPlaces;

  SavedList? list(int id) => _lists[id];

  /// Every copy of the place [id]; empty if it isn't saved.
  List<SavedPlace> copiesOf(String id) => _copies[id] ?? const [];

  /// The lists directly in [parent]; null for the top level.
  List<SavedList> listsIn(int? parent) => _childLists[parent] ?? const [];

  /// The places directly in [parent]; null for the top level.
  List<SavedPlace> placesIn(int? parent) => _childPlaces[parent] ?? const [];

  /// How many lists and places are directly in the list [id].
  int count(int id) => listsIn(id).length + placesIn(id).length;

  /// The pinned copies, in the order of their positions.
  List<SavedPlace> get pinned => [
    for (final place in _places)
      if (place.pinned) place,
  ];

  /// Every copy of every place, in order.
  List<SavedPlace> get allPlaces => _places;

  /// How much of list [id] is on in the map's layer: true for all of what's in it, at
  /// every level, false for none, null for some. An empty list has a state of its own.
  bool? shownState(int id) {
    final states = [
      for (final list in listsIn(id)) shownState(list.id),
      for (final place in placesIn(id)) place.shownOnMap,
    ];
    if (states.isEmpty) return _lists[id]?.shownOnMap ?? false;
    if (states.every((state) => state == true)) return true;
    if (states.every((state) => state == false)) return false;
    return null;
  }

  /// Every copy in the order Saved shows them, every level opened: at each level its
  /// lists, each followed by what's in it, and then its places.
  List<SavedPlace> get inOrder {
    final places = <SavedPlace>[];
    void visit(int? parent) {
      listsIn(parent).map((list) => list.id).forEach(visit);
      places.addAll(placesIn(parent));
    }

    visit(null);
    return places;
  }

  /// What the map's layer shows, in the order Saved shows them ([inOrder]): each place
  /// with a copy on, once, where its first copy that's on is, as its first such copy
  /// with an emoji or else that first one.
  List<SavedPlace> get shownPlaces {
    final chosen = <String, SavedPlace>{};
    for (final place in inOrder) {
      if (!place.shownOnMap) continue;
      final current = chosen[place.id];
      if (current == null || (current.emoji == null && place.emoji != null)) {
        chosen[place.id] = place;
      }
    }
    return chosen.values.toList();
  }

  /// The lists containing list [id], from the top level down, without itself.
  List<SavedList> ancestors(int id) {
    final chain = <SavedList>[];
    var parent = _lists[id]?.parent;
    while (parent != null) {
      final list = _lists[parent]!;
      chain.insert(0, list);
      parent = list.parent;
    }
    return chain;
  }

  /// The lists anywhere inside list [id], without itself.
  Set<int> descendants(int id) => {
    for (final child in listsIn(id)) ...{child.id, ...descendants(child.id)},
  };

  /// Every list as a depth-first walk from the top level, for showing as a tree.
  List<({SavedList list, int depth})> get walk {
    final walked = <({SavedList list, int depth})>[];
    void visit(int? parent, int depth) {
      for (final list in listsIn(parent)) {
        walked.add((list: list, depth: depth));
        visit(list.id, depth + 1);
      }
    }

    visit(null, 0);
    return walked;
  }
}

/// How to undo a change: rows as they were, and copies it added.
class SavedSnapshot {
  const SavedSnapshot({
    required this.lists,
    required this.places,
    this.added = const [],
  });

  /// Parents before children.
  final List<SavedListRow> lists;
  final List<SavedPlaceRow> places;

  /// The [SavedPlace.entryId]s of copies to take away again.
  final List<int> added;
}

/// What's in Saved: nested lists of places.
///
/// A newly saved place goes at the top of the top level; anything else goes at the end
/// of where it's put, keeping its order. A list's lists and places are ordered
/// separately. A place can be in several lists, once each, and a list in one: the database refuses a
/// list inside itself (see `saved.drift`).
class SavedStore {
  SavedStore(this._db, {DateTime Function()? now}) : _now = now ?? DateTime.now;

  final CardinalDatabase _db;
  final DateTime Function() _now;

  /// All of Saved, and again whenever it changes.
  Stream<SavedTree> watch() {
    late final StreamController<SavedTree> controller;
    StreamSubscription<void>? updates;
    Future<void> send() async {
      final tree = await _load();
      if (!controller.isClosed) controller.add(tree);
    }

    controller = StreamController(
      onListen: () {
        updates = _db
            .tableUpdates(
              TableUpdateQuery.onAllTables([_db.savedLists, _db.savedPlaces]),
            )
            .listen((_) => send());
        unawaited(send());
      },
      onCancel: () async {
        await updates?.cancel();
        await controller.close();
      },
    );
    return controller.stream;
  }

  Future<SavedTree> _load() async {
    final listRows = await (_db.select(
      _db.savedLists,
    )..orderBy([(l) => OrderingTerm.asc(l.position)])).get();
    final p = _db.places;
    final s = _db.savedPlaces;
    final query =
        _db.select(s).join([
            innerJoin(p, p.id.equalsExp(s.placeId), useColumns: false),
          ])
          ..addColumns([
            p.id,
            p.kind,
            p.name,
            p.countryCode,
            p.centerLon,
            p.centerLat,
            p.address,
            p.admins,
          ])
          ..orderBy([OrderingTerm.asc(s.position)]);
    final rows = await query.get();
    return SavedTree(
      lists: [for (final row in listRows) savedListFromRow(row)],
      places: [
        for (final row in rows)
          savedPlaceFromRow(row: row.readTable(s), place: _placeOf(row)),
      ],
    );
  }

  Place _placeOf(TypedResult row) {
    final p = _db.places;
    final address = row.read(p.address);
    final admins = row.read(p.admins);
    return Place(
      id: row.read(p.id)!,
      kind: row.read(p.kind)!,
      name: row.read(p.name),
      countryCode: row.read(p.countryCode),
      center: LngLat(row.read(p.centerLon)!, row.read(p.centerLat)!),
      address: address == null
          ? null
          : Address.fromJson(jsonDecode(address) as Map<String, Object?>),
      admins: admins == null
          ? null
          : [
              for (final a in jsonDecode(admins) as List<Object?>)
                AdminRef.fromJson(a as Map<String, Object?>),
            ],
    );
  }

  /// Makes a list at the end of [parent]'s lists, on in the map's layer if all of
  /// [parent] is, and returns its ID.
  Future<int> createList(String name, {int? parent}) =>
      _db.transaction(() async {
        final position = await _endList(parent);
        final shown = await _startsShown(parent);
        return _db
            .into(_db.savedLists)
            .insert(
              SavedListsCompanion.insert(
                name: name,
                parentId: Value(parent),
                position: position + 1,
                shownOnMap: Value(shown),
              ),
            );
      });

  /// Saves the stored place [placeId] at the top of the top level, unless it's there.
  Future<void> savePlace(String placeId) => _db.transaction(() async {
    final position = await _topPlace(null);
    await _db
        .into(_db.savedPlaces)
        .insert(
          SavedPlacesCompanion.insert(
            placeId: placeId,
            position: position - 1,
            savedAt: _now(),
          ),
          mode: InsertMode.insertOrIgnore,
        );
  });

  /// Sets what the user calls [item], their note on it, and for a place its [emoji]: a blank [name] for a place
  /// goes back to its own name, and a blank [note] clears it. Lists keep their name if
  /// [name] is blank.
  Future<void> edit(
    SavedItem item, {
    required String name,
    String? note,
    String? emoji,
  }) async {
    final blankNote = note == null || note.trim().isEmpty;
    switch (item) {
      case SavedList(:final id):
        if (name.trim().isEmpty) return;
        await (_db.update(_db.savedLists)..where((l) => l.id.equals(id))).write(
          SavedListsCompanion(
            name: Value(name.trim()),
            note: Value(blankNote ? null : note.trim()),
          ),
        );
      case SavedPlace(:final entryId):
        await (_db.update(
          _db.savedPlaces,
        )..where((p) => p.id.equals(entryId))).write(
          SavedPlacesCompanion(
            name: Value(name.trim().isEmpty ? null : name.trim()),
            note: Value(blankNote ? null : note.trim()),
            emoji: Value(emoji),
          ),
        );
    }
  }

  /// Pins or unpins the copies [entryIds].
  Future<void> setPinned(Iterable<int> entryIds, bool pinned) =>
      (_db.update(_db.savedPlaces)..where((p) => p.id.isIn(entryIds))).write(
        SavedPlacesCompanion(pinned: Value(pinned)),
      );

  /// Turns [item] on or off in the map's layer, and for a list everything in it.
  Future<void> setShown(SavedItem item, bool shown) =>
      _db.transaction(() async {
        switch (item) {
          case SavedPlace(:final entryId):
            await (_db.update(_db.savedPlaces)
                  ..where((p) => p.id.equals(entryId)))
                .write(SavedPlacesCompanion(shownOnMap: Value(shown)));
          case SavedList(:final id):
            final lists = [id, ...(await _load()).descendants(id)];
            await (_db.update(_db.savedLists)..where((l) => l.id.isIn(lists)))
                .write(SavedListsCompanion(shownOnMap: Value(shown)));
            await (_db.update(_db.savedPlaces)
                  ..where((p) => p.listId.isIn(lists)))
                .write(SavedPlacesCompanion(shownOnMap: Value(shown)));
        }
      });

  /// Whether something new in [list] starts on in the map's layer: if all of the list
  /// is. Never at the top level, which isn't a list to be all on.
  Future<bool> _startsShown(int? list) async =>
      list != null && (await _load()).shownState(list) == true;

  /// Copies [items] to the end of [to], null for the top level, with their names and
  /// notes but unpinned, except any already there. Returns how to undo it, for [restore].
  Future<SavedSnapshot> copy(List<SavedPlace> items, int? to) =>
      _db.transaction(() async {
        final there = await _placeIdsIn(to);
        final shown = await _startsShown(to);
        var position = await _endPlace(to);
        final added = <int>[];
        for (final item in items) {
          if (!there.add(item.id)) continue;
          added.add(
            await _db
                .into(_db.savedPlaces)
                .insert(
                  SavedPlacesCompanion.insert(
                    placeId: item.id,
                    listId: Value(to),
                    name: Value(item.customName),
                    note: Value(item.note),
                    emoji: Value(item.emoji),
                    shownOnMap: Value(shown),
                    position: ++position,
                    savedAt: _now(),
                  ),
                ),
          );
        }
        return SavedSnapshot(lists: const [], places: const [], added: added);
      });

  /// Makes the place [placeId] be in exactly [lists], null for the top level and empty
  /// for nowhere. New copies go at the end, with the name and note of a copy it had. Returns
  /// how to undo it, for [restore].
  Future<SavedSnapshot> setLocations(String placeId, Set<int?> lists) =>
      _db.transaction(() async {
        final rows =
            await (_db.select(_db.savedPlaces)
                  ..where((p) => p.placeId.equals(placeId))
                  ..orderBy([(p) => OrderingTerm.asc(p.position)]))
                .get();
        final removed = [
          for (final row in rows)
            if (!lists.contains(row.listId)) row,
        ];
        await (_db.delete(
          _db.savedPlaces,
        )..where((p) => p.id.isIn([for (final row in removed) row.id]))).go();
        final present = {for (final row in rows) row.listId};
        final source = rows.firstOrNull;
        final added = <int>[];
        for (final list in lists) {
          if (present.contains(list)) continue;
          added.add(
            await _db
                .into(_db.savedPlaces)
                .insert(
                  SavedPlacesCompanion.insert(
                    placeId: placeId,
                    listId: Value(list),
                    name: Value(source?.name),
                    note: Value(source?.note),
                    emoji: Value(source?.emoji),
                    shownOnMap: Value(await _startsShown(list)),
                    position: await _endPlace(list) + 1,
                    savedAt: _now(),
                  ),
                ),
          );
        }
        return SavedSnapshot(lists: const [], places: removed, added: added);
      });

  /// Moves [items] to the end of [to]; null for the top level. A place that already has
  /// a copy there joins it: the copy moved goes away. What moves into a list that's all
  /// on in the map's layer turns on, with anything inside it; otherwise it stays as it
  /// was.
  ///
  /// Throws if a list would end up inside itself, changing nothing. Returns how they
  /// were, for [restore].
  Future<SavedSnapshot> move(
    List<SavedItem> items,
    int? to,
  ) => _db.transaction(() async {
    final listIds = [
      for (final item in items)
        if (item is SavedList) item.id,
    ];
    final places = items.whereType<SavedPlace>().toList();
    final entryIds = [for (final place in places) place.entryId];
    final shown = await _startsShown(to);
    final tree = await _load();
    // Only what turning on will change, besides the moved items themselves.
    final inside = !shown
        ? const <int>[]
        : [for (final id in listIds) ...tree.descendants(id)];
    final turningOn = [...listIds, ...inside];
    final snapshot = SavedSnapshot(
      lists: await (_db.select(
        _db.savedLists,
      )..where((l) => l.id.isIn(turningOn))).get(),
      places:
          await (_db.select(_db.savedPlaces)..where(
                (p) =>
                    p.id.isIn(entryIds) |
                    (shown ? p.listId.isIn(turningOn) : const Constant(false)),
              ))
              .get(),
    );
    var position = await _endList(to);
    for (final id in listIds) {
      await (_db.update(_db.savedLists)..where((l) => l.id.equals(id))).write(
        SavedListsCompanion(parentId: Value(to), position: Value(++position)),
      );
    }
    if (shown) {
      await (_db.update(_db.savedLists)..where((l) => l.id.isIn(turningOn)))
          .write(const SavedListsCompanion(shownOnMap: Value(true)));
      await (_db.update(_db.savedPlaces)
            ..where((p) => p.listId.isIn(turningOn)))
          .write(const SavedPlacesCompanion(shownOnMap: Value(true)));
    }
    final there = await _placeIdsIn(to);
    position = await _endPlace(to);
    for (final place in places) {
      if (place.parent == to) continue;
      final copy = _db.savedPlaces;
      if (!there.add(place.id)) {
        await (_db.delete(copy)..where((p) => p.id.equals(place.entryId))).go();
        continue;
      }
      await (_db.update(copy)..where((p) => p.id.equals(place.entryId))).write(
        SavedPlacesCompanion(
          listId: Value(to),
          position: Value(++position),
          shownOnMap: shown ? const Value(true) : const Value.absent(),
        ),
      );
    }
    return snapshot;
  });

  /// Puts the lists directly in [parent] in the order of [ids].
  Future<void> reorderLists(List<int> ids) => _db.transaction(() async {
    for (final (position, id) in ids.indexed) {
      await (_db.update(_db.savedLists)..where((l) => l.id.equals(id))).write(
        SavedListsCompanion(position: Value(position)),
      );
    }
  });

  /// Puts the copies directly in a list in the order of [entryIds].
  Future<void> reorderPlaces(List<int> entryIds) => _db.transaction(() async {
    for (final (position, id) in entryIds.indexed) {
      await (_db.update(_db.savedPlaces)..where((p) => p.id.equals(id))).write(
        SavedPlacesCompanion(position: Value(position)),
      );
    }
  });

  /// Deletes [items], which for a place is that copy of it, and for a list everything
  /// directly in it and below. Returns what was deleted, for
  /// [restore].
  Future<SavedSnapshot> delete(
    List<SavedItem> items,
  ) => _db.transaction(() async {
    final all = await _db.select(_db.savedLists).get();
    final byParent = <int?, List<SavedListRow>>{};
    for (final row in all) {
      (byParent[row.parentId] ??= []).add(row);
    }
    final lists = <SavedListRow>[];
    void take(SavedListRow row) {
      lists.add(row);
      (byParent[row.id] ?? const <SavedListRow>[]).forEach(take);
    }

    final listIds = {
      for (final item in items)
        if (item is SavedList) item.id,
    };
    all.where((row) => listIds.contains(row.id)).forEach(take);
    final entryIds = [
      for (final item in items)
        if (item is SavedPlace) item.entryId,
    ];
    final allIds = [for (final row in lists) row.id];
    final places = await (_db.select(
      _db.savedPlaces,
    )..where((p) => p.id.isIn(entryIds) | p.listId.isIn(allIds))).get();
    await (_db.delete(_db.savedPlaces)..where((p) => p.id.isIn(entryIds))).go();
    await (_db.delete(_db.savedLists)..where((l) => l.id.isIn(allIds))).go();
    return SavedSnapshot(lists: lists, places: places);
  });

  /// Undoes a change, from what it returned.
  Future<void> restore(SavedSnapshot snapshot) => _db.transaction(() async {
    await (_db.delete(
      _db.savedPlaces,
    )..where((p) => p.id.isIn(snapshot.added))).go();
    for (final row in snapshot.lists) {
      // As a companion, so a null parent is written, not skipped. Not a replace, which
      // would take the list's contents with it.
      await _db
          .into(_db.savedLists)
          .insertOnConflictUpdate(row.toCompanion(false));
    }
    for (final row in snapshot.places) {
      await _db
          .into(_db.savedPlaces)
          .insert(row.toCompanion(false), mode: InsertMode.insertOrReplace);
    }
  });

  /// The places directly in [list], null for the top level.
  Future<Set<String>> _placeIdsIn(int? list) async {
    final rows =
        await (_db.select(_db.savedPlaces)..where(
              (p) => list == null ? p.listId.isNull() : p.listId.equals(list),
            ))
            .get();
    return {for (final row in rows) row.placeId};
  }

  /// The highest position in [parent]'s lists, or -1.
  Future<int> _endList(int? parent) async {
    final t = _db.savedLists;
    final highest = t.position.max();
    final query = _db.selectOnly(t)
      ..addColumns([highest])
      ..where(parent == null ? t.parentId.isNull() : t.parentId.equals(parent));
    return (await query.getSingle()).read(highest) ?? -1;
  }

  /// The highest position in [parent]'s places, or -1.
  Future<int> _endPlace(int? parent) async {
    final t = _db.savedPlaces;
    final highest = t.position.max();
    final query = _db.selectOnly(t)
      ..addColumns([highest])
      ..where(parent == null ? t.listId.isNull() : t.listId.equals(parent));
    return (await query.getSingle()).read(highest) ?? -1;
  }

  /// The lowest position in [parent]'s places, or 0.
  Future<int> _topPlace(int? parent) async {
    final t = _db.savedPlaces;
    final lowest = t.position.min();
    final query = _db.selectOnly(t)
      ..addColumns([lowest])
      ..where(parent == null ? t.listId.isNull() : t.listId.equals(parent));
    return (await query.getSingle()).read(lowest) ?? 0;
  }
}

/// A [SavedList] from its row.
SavedList savedListFromRow(SavedListRow row) => SavedList(
  id: row.id,
  parent: row.parentId,
  name: row.name,
  note: row.note,
  shownOnMap: row.shownOnMap,
);

/// A [SavedPlace] from its row and what's known about [place].
SavedPlace savedPlaceFromRow({
  required SavedPlaceRow row,
  required Place place,
}) => SavedPlace(
  entryId: row.id,
  place: place,
  parent: row.listId,
  customName: row.name,
  note: row.note,
  pinned: row.pinned,
  emoji: row.emoji,
  shownOnMap: row.shownOnMap,
);
