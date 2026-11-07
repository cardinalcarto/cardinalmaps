import 'package:drift/drift.dart';
import 'package:drift_flutter/drift_flutter.dart';
import 'package:path_provider/path_provider.dart';

import 'database.steps.dart';

part 'database.g.dart';

/// Places shown on their own screen: POIs, addresses and administrative areas. See
/// `Place` for what the columns hold.
@DataClassName('PlaceRow')
class Places extends Table {
  /// The geocoder's identifier: `poi:<id>`, `wof:<id>` or `osm:<type>/<id>`.
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

@DriftDatabase(tables: [Places, Settings])
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
  int get schemaVersion => 3;

  @override
  MigrationStrategy get migration => MigrationStrategy(
    onUpgrade: stepByStep(
      from1To2: (m, schema) =>
          m.addColumn(schema.places, schema.places.countryCode),
      from2To3: (m, schema) => m.createTable(schema.settings),
    ),
  );
}
