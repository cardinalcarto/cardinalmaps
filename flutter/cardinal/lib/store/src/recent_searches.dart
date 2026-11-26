import 'dart:async';

import 'package:drift/drift.dart';

import 'database.dart';
import 'place.dart';
import 'places.dart';
import 'settings.dart';

/// On-device history of opened search results, newest first. This is not Saved
/// and must never be synced. Opened places of all kinds are recorded; queries may be added later.
abstract class RecentSearchStore {
  factory RecentSearchStore(CardinalDatabase db, SettingsStore settings) =
      _DatabaseRecentSearches;
  factory RecentSearchStore.inMemory(SettingsStore settings) =
      _MemoryRecentSearches;

  static const limit = 50;

  Stream<List<Place>> watch();

  /// Records a place already stored in the place store.
  Future<void> record(Place place);
  Future<void> remove(String placeId);
  Future<void> close();
}

class _DatabaseRecentSearches implements RecentSearchStore {
  _DatabaseRecentSearches(this._db, this._settings);
  final CardinalDatabase _db;
  final SettingsStore _settings;

  @override
  Stream<List<Place>> watch() {
    final recent = _db.recentSearches;
    final places = _db.places;
    return (_db.select(recent).join([
      innerJoin(places, places.id.equalsExp(recent.placeId)),
    ])..orderBy([OrderingTerm.desc(recent.id)])).watch().map(
      (rows) => [for (final row in rows) placeFromRow(row.readTable(places))],
    );
  }

  @override
  Future<void> record(Place place) async {
    await _db.transaction(() async {
      if (!await _settings.get(saveSearchHistory)) return;
      await remove(place.id);
      await _db
          .into(_db.recentSearches)
          .insert(RecentSearchesCompanion.insert(placeId: place.id));
      final recent = _db.recentSearches;
      final overflow = _db.selectOnly(recent)
        ..addColumns([recent.id])
        ..orderBy([OrderingTerm.desc(recent.id)])
        ..limit(1, offset: RecentSearchStore.limit - 1);
      final cutoff = await overflow.getSingleOrNull();
      if (cutoff != null) {
        await (_db.delete(
          recent,
        )..where((r) => r.id.isSmallerThanValue(cutoff.read(recent.id)!))).go();
      }
    });
  }

  @override
  Future<void> remove(String placeId) async {
    await (_db.delete(
      _db.recentSearches,
    )..where((r) => r.placeId.equals(placeId))).go();
  }

  @override
  Future<void> close() async {}
}

class _MemoryRecentSearches implements RecentSearchStore {
  _MemoryRecentSearches(this._settings) {
    _subscription = _settings.watch(saveSearchHistory).listen((enabled) {
      if (!enabled) {
        _places.clear();
        _changes.add([]);
      }
    });
  }
  final SettingsStore _settings;
  final _places = <String, Place>{};
  final _changes = StreamController<List<Place>>.broadcast();
  late final StreamSubscription<bool> _subscription;
  List<Place> get _recent => _places.values.toList().reversed.toList();

  @override
  Stream<List<Place>> watch() => Stream.multi((controller) {
    final subscription = _changes.stream.listen(controller.add);
    controller.add(_recent);
    controller.onCancel = subscription.cancel;
  });

  @override
  Future<void> record(Place place) async {
    if (!await _settings.get(saveSearchHistory)) return;
    _places.remove(place.id);
    _places[place.id] = place;
    if (_places.length > RecentSearchStore.limit)
      _places.remove(_places.keys.first);
    _changes.add(_recent);
  }

  @override
  Future<void> remove(String placeId) async {
    _places.remove(placeId);
    _changes.add(_recent);
  }

  @override
  Future<void> close() async {
    await _subscription.cancel();
    await _changes.close();
  }
}
