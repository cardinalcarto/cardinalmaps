import 'package:drift/drift.dart';
import 'package:drift_flutter/drift_flutter.dart';
import 'package:path_provider/path_provider.dart';

import 'database.steps.dart';

part 'database.g.dart';

/// Places shown on their own screen: POIs, addresses and administrative areas. See
/// `Place` for what the columns hold.
@DataClassName('PlaceRow')
class Places extends Table {
  /// The geocoder's identifier: `poi:<id>`, `wof:<id>`, `osm:<type>/<id>`, ... (see
  /// `Place.id`).
  TextColumn get id => text()();
  TextColumn get kind => text()();
  IntColumn get version => integer().nullable()();
  TextColumn get name => text().nullable()();

  /// ISO 3166-1 alpha-2, such as `US`.
  TextColumn get countryCode => text().nullable()();
  RealColumn get centerLon => real()();
  RealColumn get centerLat => real()();
  RealColumn get bboxWest => real().nullable()();
  RealColumn get bboxSouth => real().nullable()();
  RealColumn get bboxEast => real().nullable()();
  RealColumn get bboxNorth => real().nullable()();

  /// A GeoJSON geometry.
  TextColumn get shape => text().nullable()();

  /// JSON, as in [Place].
  TextColumn get address => text().nullable()();
  TextColumn get admins => text().nullable()();
  TextColumn get tags => text()();

  DateTimeColumn get updatedAt => dateTime()();
  DateTimeColumn get accessedAt => dateTime()();

  @override
  Set<Column> get primaryKey => {id};
}

/// The app's settings, one row each: see `SettingsStore`.
@DataClassName('SettingRow')
class Settings extends Table {
  TextColumn get key => text()();

  /// The value as JSON.
  TextColumn get value => text()();

  @override
  Set<Column> get primaryKey => {key};
}

/// A list of saved places, within another list or at the top level of Saved.
///
/// Its child lists and places go with it when it's deleted.
@DataClassName('SavedListRow')
class SavedLists extends Table {
  IntColumn get id => integer().autoIncrement()();

  /// The list it's in; null at the top level.
  IntColumn get parentId => integer().nullable().references(
    SavedLists,
    #id,
    onDelete: KeyAction.cascade,
  )();
  TextColumn get name => text()();
  TextColumn get note => text().nullable()();

  /// Its place among the lists with the same parent, lowest first.
  IntColumn get position => integer()();

  /// Whether it's on in the map's Saved Places layer, for as long as it's empty; after
  /// that, it's on as much as what's in it is.
  BoolColumn get shownOnMap => boolean().withDefault(const Constant(false))();
}

/// A place in a list, or at the top level of Saved: a place can be in several, once
/// each (see `saved.drift`). What's known about the place itself is in [Places];
/// `PlaceStore.prune` leaves saved places alone.
@DataClassName('SavedPlaceRow')
class SavedPlaces extends Table {
  IntColumn get id => integer().autoIncrement()();
  TextColumn get placeId => text().references(Places, #id)();

  /// The list it's in; null at the top level.
  IntColumn get listId => integer().nullable().references(
    SavedLists,
    #id,
    onDelete: KeyAction.cascade,
  )();

  /// The user's name for it, instead of the place's own.
  TextColumn get name => text().nullable()();
  TextColumn get note => text().nullable()();

  /// Whether this copy has a chip on the home sheet.
  BoolColumn get pinned => boolean().withDefault(const Constant(false))();

  /// Its place among the places in the same list, lowest first.
  IntColumn get position => integer()();
  DateTimeColumn get savedAt => dateTime()();

  /// The emoji it's marked with on the map, one grapheme.
  TextColumn get emoji => text().nullable()();

  /// Whether this copy is on in the map's Saved Places layer.
  BoolColumn get shownOnMap => boolean().withDefault(const Constant(false))();
}

/// The headsign a rider last settled on for a route at a stop, so its row in Nearby
/// transit opens on that headsign's page there: see `HeadsignStore`.
@DataClassName('RememberedHeadsignRow')
class RememberedHeadsigns extends Table {
  /// The GTFS feed, in which [routeId] and [stopId] are unique.
  TextColumn get feed => text()();
  TextColumn get routeId => text()();
  TextColumn get stopId => text()();

  /// The headsign; empty for the departures that have none.
  TextColumn get headsign => text()();

  /// When the rider settled on it.
  DateTimeColumn get chosenAt => dateTime()();

  @override
  Set<Column> get primaryKey => {feed, routeId, stopId};
}

/// Local-only search history. Never part of saved-place sync.
@DataClassName('RecentSearchRow')
class RecentSearches extends Table {
  IntColumn get id => integer().autoIncrement()();
  TextColumn get placeId => text().unique().references(Places, #id)();
}

@DriftDatabase(
  tables: [
    Places,
    Settings,
    SavedLists,
    SavedPlaces,
    RememberedHeadsigns,
    RecentSearches,
  ],
  include: {'saved.drift'},
)
class CardinalDatabase extends _$CardinalDatabase {
  CardinalDatabase(super.executor);

  /// The app's database, in the platform's application support directory.
  CardinalDatabase.open()
    : super(
        driftDatabase(
          name: 'cardinal',
          native: const DriftNativeOptions(
            databaseDirectory: getApplicationSupportDirectory,
          ),
        ),
      );

  @override
  int get schemaVersion => 8;

  @override
  MigrationStrategy get migration => MigrationStrategy(
    onUpgrade: stepByStep(
      from1To2: (m, schema) =>
          m.addColumn(schema.places, schema.places.countryCode),
      from2To3: (m, schema) => m.createTable(schema.settings),
      from3To4: (m, schema) async {
        await m.createTable(schema.savedLists);
        await m.createTable(schema.savedPlaces);
      },
      from4To5: (m, schema) async {
        // A place can now be in several lists: rows get an ID of their own.
        await m.alterTable(
          TableMigration(
            schema.savedPlaces,
            newColumns: [schema.savedPlaces.id],
          ),
        );
        await m.create(schema.savedPlacesOncePerList);
        await m.create(schema.savedPlacesOnceAtTop);
        await m.create(schema.savedListsNoCycleOnUpdate);
        await m.create(schema.savedListsNoCycleOnInsert);
      },
      from5To6: (m, schema) async {
        await m.addColumn(schema.savedLists, schema.savedLists.shownOnMap);
        await m.addColumn(schema.savedPlaces, schema.savedPlaces.emoji);
        await m.addColumn(schema.savedPlaces, schema.savedPlaces.shownOnMap);
      },
      from6To7: (m, schema) => m.createTable(schema.rememberedHeadsigns),
      from7To8: (m, schema) => m.createTable(schema.recentSearches),
    ),
    // Deleting a list takes what's in it along.
    beforeOpen: (details) => customStatement('PRAGMA foreign_keys = ON'),
  );
}
