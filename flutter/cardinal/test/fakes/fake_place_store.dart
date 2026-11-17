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

  /// The place now, then again at each change. It hears changes from the moment it's
  /// listened to, as the database's query does: a put made before the first value
  /// arrives still comes through.
  @override
  Stream<Place?> watch(String id) {
    StreamSubscription<String>? changes;
    late final StreamController<Place?> controller;
    controller = StreamController<Place?>(
      onListen: () {
        controller.add(_places[id]);
        changes = _changes.stream
            .where((changed) => changed == id)
            .listen((_) => controller.add(_places[id]));
      },
      onCancel: () => changes?.cancel(),
    );
    return controller.stream;
  }

  @override
  Future<int> prune({Duration age = const Duration(days: 30)}) async => 0;
}
