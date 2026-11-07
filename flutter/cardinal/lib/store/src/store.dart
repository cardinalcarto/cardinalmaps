import 'package:material_ui/material_ui.dart';

import 'database.dart';
import 'places.dart';
import 'settings.dart';

/// The app's persistent state: what screens are about, stored by ID so that routes
/// can name it, and the app's [settings].
///
/// To open a screen about something, put it here and then navigate to its ID; the screen
/// reads it back by ID. Routes then carry only IDs, so they can be deep-linked and
/// restored, and nothing is passed to a screen any other way.
class Store {
  Store(CardinalDatabase db) : _db = db, places = PlaceStore(db), settings = SettingsStore(db);

  /// The store in the app's database.
  Store.open() : this(CardinalDatabase.open());

  /// A store of [places] without a database, for widget tests to give fakes. Settings
  /// are kept in memory unless [settings] is given.
  @visibleForTesting
  Store.withoutDatabase({required this.places, SettingsStore? settings})
    : _db = null,
      settings = settings ?? SettingsStore.inMemory();

  final CardinalDatabase? _db;

  final PlaceStore places;

  final SettingsStore settings;

  /// Deletes what hasn't been used in a while.
  Future<void> prune() => places.prune();

  Future<void> close() async => _db?.close();
}

/// Provides a [Store] to the widgets below it.
class StoreScope extends InheritedWidget {
  const StoreScope({super.key, required this.store, required super.child});

  final Store store;

  /// The store from the closest [StoreScope] around [context].
  static Store of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<StoreScope>()!.store;

  @override
  bool updateShouldNotify(StoreScope oldWidget) => store != oldWidget.store;
}
