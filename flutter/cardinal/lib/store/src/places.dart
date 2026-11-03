import 'dart:convert';

import 'package:drift/drift.dart';

import 'database.dart';
import 'place.dart';

/// Stored places, by ID.
///
/// Whatever opens a place's screen [put]s what it knows about the place, then navigates
/// to the place's ID. The screen [watch]es the ID, starting from [cached] so its first
/// frame has the place.
class PlaceStore {
  PlaceStore(this._db, {DateTime Function()? now}) : _now = now ?? DateTime.now;

  final CardinalDatabase _db;
  final DateTime Function() _now;

  /// Places recently put or read, most recent last.
  final _cache = <String, Place>{};
  static const _cacheSize = 64;

  /// The place with [id] if it was put or read recently, without waiting on the
  /// database.
  Place? cached(String id) => _cache[id];

  /// Stores what's known about a place, merged with what's stored (see [Place.merge]),
  /// and returns the result.
  Future<Place> put(Place place) async {
    final merged = await _db.transaction(() async {
      final stored = await _select(place.id).getSingleOrNull();
      final merged = stored == null ? place : _fromRow(stored).merge(place);
      final now = _now();
      await _db.into(_db.places).insertOnConflictUpdate(_toRow(merged, now));
      return merged;
    });
    _remember(merged);
    return merged;
  }

  /// The stored place with [id], if any.
  Future<Place?> get(String id) async {
    final row = await _select(id).getSingleOrNull();
    if (row == null) return null;
    await _touch(id);
    return _remember(_fromRow(row));
  }

  /// The stored place with [id], and again whenever it changes. Null while there isn't
  /// one.
  Stream<Place?> watch(String id) async* {
    await _touch(id);
    yield* _select(id)
        .watchSingleOrNull()
        .map((row) => row == null ? null : _remember(_fromRow(row)))
        .distinct();
  }

  /// Deletes places that haven't been put or read in [age].
  Future<int> prune({Duration age = const Duration(days: 30)}) {
    final cutoff = _now().subtract(age);
    return (_db.delete(_db.places)..where((p) => p.accessedAt.isSmallerThanValue(cutoff))).go();
  }

  SimpleSelectStatement<$PlacesTable, PlaceRow> _select(String id) =>
      _db.select(_db.places)..where((p) => p.id.equals(id));

  Future<void> _touch(String id) => (_db.update(
    _db.places,
  )..where((p) => p.id.equals(id))).write(PlacesCompanion(accessedAt: Value(_now())));

  Place _remember(Place place) {
    _cache.remove(place.id);
    _cache[place.id] = place;
    if (_cache.length > _cacheSize) _cache.remove(_cache.keys.first);
    return place;
  }
}

PlacesCompanion _toRow(Place place, DateTime now) {
  final bbox = place.bbox;
  return PlacesCompanion.insert(
    id: place.id,
    kind: place.kind,
    version: Value(place.version),
    name: Value(place.name),
    countryCode: Value(place.countryCode),
    centerLon: place.center.lon,
    centerLat: place.center.lat,
    bboxWest: Value(bbox?.west),
    bboxSouth: Value(bbox?.south),
    bboxEast: Value(bbox?.east),
    bboxNorth: Value(bbox?.north),
    shape: Value(place.shape == null ? null : jsonEncode(place.shape)),
    address: Value(place.address == null ? null : jsonEncode(place.address!.toJson())),
    admins: Value(
      place.admins == null ? null : jsonEncode([for (final a in place.admins!) a.toJson()]),
    ),
    tags: jsonEncode(place.tags),
    updatedAt: now,
    accessedAt: now,
  );
}

Place _fromRow(PlaceRow row) {
  final (west, south, east, north) = (row.bboxWest, row.bboxSouth, row.bboxEast, row.bboxNorth);
  return Place(
    id: row.id,
    kind: row.kind,
    version: row.version,
    name: row.name,
    countryCode: row.countryCode,
    center: LngLat(row.centerLon, row.centerLat),
    bbox: west != null && south != null && east != null && north != null
        ? Bounds(west: west, south: south, east: east, north: north)
        : null,
    shape: row.shape == null ? null : jsonDecode(row.shape!) as Map<String, Object?>,
    address: row.address == null
        ? null
        : Address.fromJson(jsonDecode(row.address!) as Map<String, Object?>),
    admins: row.admins == null
        ? null
        : [
            for (final a in jsonDecode(row.admins!) as List<Object?>)
              AdminRef.fromJson(a as Map<String, Object?>),
          ],
    tags: (jsonDecode(row.tags) as Map<String, Object?>).cast<String, String>(),
  );
}
