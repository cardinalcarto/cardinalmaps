import 'dart:async';

import 'package:cardinal/store/store.dart';
import 'package:drift/drift.dart' show Value;

import 'fake_place_store.dart';

/// A [SavedStore] in memory, for widget tests: see [FakePlaceStore]. It orders and
/// moves things as the real one does.
class FakeSavedStore implements SavedStore {
  FakeSavedStore(this.placeStore);

  final FakePlaceStore placeStore;

  final _lists = <SavedListRow>[];
  final _places = <SavedPlaceRow>[];
  final _changes = StreamController<void>.broadcast(sync: true);
  int _nextId = 1;
  int _nextEntry = 1;

  SavedTree get _tree {
    final lists = [..._lists]..sort((a, b) => a.position - b.position);
    final places = [..._places]..sort((a, b) => a.position - b.position);
    return SavedTree(
      lists: [for (final row in lists) savedListFromRow(row)],
      places: [
        for (final row in places)
          savedPlaceFromRow(row: row, place: placeStore.cached(row.placeId)!),
      ],
    );
  }

  void _changed() => _changes.add(null);

  bool _startsShown(int? list) =>
      list == null || _tree.shownState(list) == true;

  int _endList(int? parent) => _lists
      .where((r) => r.parentId == parent)
      .fold(-1, (highest, r) => r.position > highest ? r.position : highest);

  int _endPlace(int? parent) => _places
      .where((r) => r.listId == parent)
      .fold(-1, (highest, r) => r.position > highest ? r.position : highest);

  int _topPlace(int? parent) => _places
      .where((r) => r.listId == parent)
      .fold(0, (lowest, r) => r.position < lowest ? r.position : lowest);

  @override
  Stream<SavedTree> watch() async* {
    yield _tree;
    yield* _changes.stream.map((_) => _tree);
  }

  @override
  Future<int> createList(String name, {int? parent}) async {
    final id = _nextId++;
    _lists.add(
      SavedListRow(
        id: id,
        parentId: parent,
        name: name,
        position: _endList(parent) + 1,
        shownOnMap: _startsShown(parent),
      ),
    );
    _changed();
    return id;
  }

  @override
  Future<void> savePlace(String placeId) async {
    if (_places.any((r) => r.placeId == placeId && r.listId == null)) return;
    _places.add(
      SavedPlaceRow(
        id: _nextEntry++,
        placeId: placeId,
        pinned: false,
        shownOnMap: true,
        position: _topPlace(null) - 1,
        savedAt: DateTime(2026),
      ),
    );
    _changed();
  }

  @override
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
        final i = _lists.indexWhere((r) => r.id == id);
        _lists[i] = _lists[i].copyWith(
          name: name.trim(),
          note: Value(blankNote ? null : note.trim()),
        );
      case SavedPlace(:final entryId):
        final i = _places.indexWhere((r) => r.id == entryId);
        _places[i] = _places[i].copyWith(
          name: Value(name.trim().isEmpty ? null : name.trim()),
          note: Value(blankNote ? null : note.trim()),
          emoji: Value(emoji),
        );
    }
    _changed();
  }

  @override
  Future<void> setPinned(Iterable<int> entryIds, bool pinned) async {
    for (final (i, row) in _places.indexed) {
      if (entryIds.contains(row.id)) _places[i] = row.copyWith(pinned: pinned);
    }
    _changed();
  }

  @override
  Future<SavedSnapshot> setLocations(String placeId, Set<int?> lists) async {
    final rows = _places.where((r) => r.placeId == placeId).toList()
      ..sort((a, b) => a.position - b.position);
    final removed = rows.where((r) => !lists.contains(r.listId)).toList();
    _places.removeWhere(removed.contains);
    final present = {for (final row in rows) row.listId};
    final added = <int>[];
    for (final list in lists) {
      if (present.contains(list)) continue;
      final id = _nextEntry++;
      added.add(id);
      _places.add(
        SavedPlaceRow(
          id: id,
          placeId: placeId,
          listId: list,
          name: rows.firstOrNull?.name,
          note: rows.firstOrNull?.note,
          emoji: rows.firstOrNull?.emoji,
          pinned: false,
          shownOnMap: _startsShown(list),
          position: _endPlace(list) + 1,
          savedAt: DateTime(2026),
        ),
      );
    }
    _changed();
    return SavedSnapshot(lists: const [], places: removed, added: added);
  }

  @override
  Future<void> setShown(SavedItem item, bool shown) async {
    switch (item) {
      case SavedPlace(:final entryId):
        final i = _places.indexWhere((r) => r.id == entryId);
        _places[i] = _places[i].copyWith(shownOnMap: shown);
      case SavedList(:final id):
        final lists = {id, ..._tree.descendants(id)};
        for (final (i, row) in _lists.indexed) {
          if (lists.contains(row.id)) {
            _lists[i] = row.copyWith(shownOnMap: shown);
          }
        }
        for (final (i, row) in _places.indexed) {
          if (lists.contains(row.listId)) {
            _places[i] = row.copyWith(shownOnMap: shown);
          }
        }
    }
    _changed();
  }

  @override
  Future<SavedSnapshot> copy(List<SavedPlace> items, int? to) async {
    final added = <int>[];
    final shown = _startsShown(to);
    var position = _endPlace(to);
    for (final item in items) {
      if (_places.any((r) => r.placeId == item.id && r.listId == to)) continue;
      final id = _nextEntry++;
      added.add(id);
      _places.add(
        SavedPlaceRow(
          id: id,
          placeId: item.id,
          listId: to,
          name: item.customName,
          note: item.note,
          emoji: item.emoji,
          pinned: false,
          shownOnMap: shown,
          position: ++position,
          savedAt: DateTime(2026),
        ),
      );
    }
    _changed();
    return SavedSnapshot(lists: const [], places: const [], added: added);
  }

  @override
  Future<SavedSnapshot> move(List<SavedItem> items, int? to) async {
    final lists = items.whereType<SavedList>().map((l) => l.id).toList();
    final places = items.whereType<SavedPlace>().toList();
    final tree = _tree;
    for (final id in lists) {
      if (id == to || tree.descendants(id).contains(to)) {
        throw ArgumentError('cycle');
      }
    }
    final entries = places.map((p) => p.entryId).toSet();
    final shown = to != null && _startsShown(to);
    final turningOn = {
      ...lists,
      if (shown)
        for (final id in lists) ...tree.descendants(id),
    };
    final snapshot = SavedSnapshot(
      lists: _lists.where((r) => turningOn.contains(r.id)).toList(),
      places: _places
          .where(
            (r) =>
                entries.contains(r.id) ||
                (shown && turningOn.contains(r.listId)),
          )
          .toList(),
    );
    if (shown) {
      for (final (i, row) in _lists.indexed) {
        if (turningOn.contains(row.id)) {
          _lists[i] = row.copyWith(shownOnMap: true);
        }
      }
      for (final (i, row) in _places.indexed) {
        if (turningOn.contains(row.listId)) {
          _places[i] = row.copyWith(shownOnMap: true);
        }
      }
    }
    var position = _endList(to);
    for (final id in lists) {
      final i = _lists.indexWhere((r) => r.id == id);
      _lists[i] = _lists[i].copyWith(parentId: Value(to), position: ++position);
    }
    position = _endPlace(to);
    for (final place in places) {
      if (place.parent == to) continue;
      final i = _places.indexWhere((r) => r.id == place.entryId);
      if (_places.any((r) => r.placeId == place.id && r.listId == to)) {
        _places.removeAt(i);
      } else {
        _places[i] = _places[i].copyWith(
          listId: Value(to),
          position: ++position,
          shownOnMap: shown ? true : _places[i].shownOnMap,
        );
      }
    }
    _changed();
    return snapshot;
  }

  @override
  Future<void> reorderLists(List<int> ids) async {
    for (final (position, id) in ids.indexed) {
      final i = _lists.indexWhere((r) => r.id == id);
      _lists[i] = _lists[i].copyWith(position: position);
    }
    _changed();
  }

  @override
  Future<void> reorderPlaces(List<int> entryIds) async {
    for (final (position, id) in entryIds.indexed) {
      final i = _places.indexWhere((r) => r.id == id);
      _places[i] = _places[i].copyWith(position: position);
    }
    _changed();
  }

  @override
  Future<SavedSnapshot> delete(List<SavedItem> items) async {
    final tree = _tree;
    final listIds = <int>{};
    final entries = items.whereType<SavedPlace>().map((p) => p.entryId).toSet();
    for (final list in items.whereType<SavedList>()) {
      listIds
        ..add(list.id)
        ..addAll(tree.descendants(list.id));
    }
    final snapshot = SavedSnapshot(
      lists: _lists.where((r) => listIds.contains(r.id)).toList(),
      places: _places
          .where((r) => entries.contains(r.id) || listIds.contains(r.listId))
          .toList(),
    );
    _places.removeWhere(
      (r) => entries.contains(r.id) || listIds.contains(r.listId),
    );
    _lists.removeWhere((r) => listIds.contains(r.id));
    _changed();
    return snapshot;
  }

  @override
  Future<void> restore(SavedSnapshot snapshot) async {
    _places.removeWhere((r) => snapshot.added.contains(r.id));
    for (final row in snapshot.lists) {
      _lists
        ..removeWhere((r) => r.id == row.id)
        ..add(row);
    }
    for (final row in snapshot.places) {
      _places
        ..removeWhere((r) => r.id == row.id)
        ..add(row);
    }
    _changed();
  }
}
