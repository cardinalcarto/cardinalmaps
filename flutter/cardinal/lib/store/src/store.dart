import 'package:material_ui/material_ui.dart';

import 'database.dart';
import 'headsigns.dart';
import 'places.dart';
import 'saved.dart';
import 'settings.dart';
import 'recent_searches.dart';

/// The app's persistent state: what screens are about, stored by ID so that routes
/// can name it, the app's [settings], and the [headsigns] nearby transit remembers.
///
/// To open a screen about something, put it here and then navigate to its ID; the screen
/// reads it back by ID. Routes then carry only IDs, so they can be deep-linked and
/// restored, and nothing is passed to a screen any other way.
class Store {
  Store(CardinalDatabase db)
    : _db = db,
      places = PlaceStore(db),
      saved = SavedStore(db),
      settings = SettingsStore(db),
      headsigns = HeadsignStore(db) {
    recentSearches = RecentSearchStore(db, settings);
  }

  /// The store in the app's database.
  Store.open() : this(CardinalDatabase.open());

  /// A store of [places] and [saved] without a database, for widget tests to give
  /// fakes. Settings and headsigns are kept in memory unless [settings] or [headsigns]
  /// is given.
  @visibleForTesting
  Store.withoutDatabase({
    required this.places,
    required this.saved,
    SettingsStore? settings,
    HeadsignStore? headsigns,
  }) : _db = null,
       settings = settings ?? SettingsStore.inMemory(),
       headsigns = headsigns ?? HeadsignStore.inMemory() {
    recentSearches = RecentSearchStore.inMemory(this.settings);
  }

  final CardinalDatabase? _db;

  final PlaceStore places;

  final SavedStore saved;

  final SettingsStore settings;

  final HeadsignStore headsigns;

  late final RecentSearchStore recentSearches;

  /// Deletes what hasn't been used in a while.
  Future<void> prune() => places.prune();

  Future<void> close() async {
    await recentSearches.close();
    await _db?.close();
  }
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
