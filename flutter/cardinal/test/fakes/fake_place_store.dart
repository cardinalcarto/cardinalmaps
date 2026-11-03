import 'dart:async';

import 'package:cardinal/store/store.dart';

/// A [PlaceStore] in memory, for widget tests: no database, so nothing that runs off
/// the test's fake clock. Places merge as in the real one ([Place.merge]).
class FakePlaceStore implements PlaceStore {
  final _places = <String, Place>{};
  final _changes = StreamController<String>.broadcast(sync: true);

  @override
  Place? cached(String id) => _places[id];

  @override
  Future<Place> put(Place place) async {
    final stored = _places[place.id];
    final merged = stored == null ? place : stored.merge(place);
    _places[place.id] = merged;
    _changes.add(place.id);
    return merged;
  }

  @override
  Future<Place?> get(String id) async => _places[id];

  @override
  Stream<Place?> watch(String id) async* {
    yield _places[id];
    yield* _changes.stream.where((changed) => changed == id).map((_) => _places[id]);
  }

  @override
  Future<int> prune({Duration age = const Duration(days: 30)}) async => 0;
}
