import 'package:drift/drift.dart';
import 'package:drift_flutter/drift_flutter.dart';
import 'package:path_provider/path_provider.dart';

part 'database.g.dart';

/// The kinds of release, as `latest.json` names them.
enum ReleaseKind { map, routing, transit }

/// What a region is for.
enum RegionKind {
  /// A polygon the user downloaded. Its files are never evicted.
  explicit,

  /// The built-in region of routing tiles fetched as routes need them (no polygon). Files
  /// only it holds are evicted, least recently used first, when space is needed.
  onDemand,
}

/// Where a region is in its life:
///
/// `planned` → `downloading` → `installing` → `ready`; `downloading` → `failed` when a
/// file or range has failed too often (retrying goes back to `downloading`); any of them
/// → `deleting` → gone.
enum RegionState { planned, downloading, installing, ready, failed, deleting }

/// Where a file is:
///
/// `pending` → `queued` (handed to the downloader; fetched in the foreground instead,
/// small JSON skips this) → `downloaded` (whole, in `.incoming/`) → `verified` (its
/// SHA-256 matched, and is stored) → `installed` (renamed to its place).
/// A failed attempt counts in `attempts` and goes back to `pending`, or to `failed` after
/// the last.
enum FileState { pending, queued, downloaded, verified, installed, failed }

/// Where a map range is:
///
/// `pending` → `queued` → `downloaded` → `unpacked` (its tiles are in the basemap) →
/// `deleted` (its file is gone). Failures go back to `pending`, or to `failed`.
enum ChunkState { pending, queued, downloaded, unpacked, deleted, failed }

/// The releases downloads are pinned to, one per kind: the first download pins those
/// `latest.json` names, and later ones use them.
@DataClassName('ReleaseRow')
class Releases extends Table {
  TextColumn get kind => textEnum<ReleaseKind>()();

  /// As in the bucket's paths: `YYYY-MM-DD`, or `YYYY-MM-DD.N`.
  TextColumn get date => text()();

  /// Routing and transit: the Valhalla build their tiles and walks are from.
  IntColumn get valhallaBuildId => integer().nullable()();

  /// Map: the archive's ETag, which every range request must match (`If-Match`).
  TextColumn get etag => text().nullable()();

  DateTimeColumn get pinnedAt => dateTime()();

  @override
  Set<Column> get primaryKey => {kind};
}

/// A downloaded area: a polygon of routing tiles, transit feeds and map tiles.
@DataClassName('RegionRow')
class Regions extends Table {
  IntColumn get id => integer().autoIncrement()();
  TextColumn get kind => textEnum<RegionKind>()();
  TextColumn get name => text().nullable()();

  /// `[[lng, lat], ...]` as JSON; null for the on-demand region.
  TextColumn get polygon => text().nullable()();

  /// The releases its files are from (the pinned ones when it was planned).
  TextColumn get mapRelease => text().nullable()();
  TextColumn get routingRelease => text().nullable()();
  TextColumn get transitRelease => text().nullable()();
  TextColumn get state => textEnum<RegionState>()();

  /// Why it failed, for logs.
  TextColumn get error => text().nullable()();

  /// Every map tile of the region the archive has (opaque bytes from the planner), for
  /// claiming tiles another region brought.
  BlobColumn get mapTiles => blob().nullable()();

  /// The map's bytes and tile count, as planned.
  IntColumn get mapBytes => integer().withDefault(const Constant(0))();
  DateTimeColumn get createdAt => dateTime()();
  DateTimeColumn get updatedAt => dateTime()();
}

/// One downloaded file, shared by every region that needs it.
@DataClassName('FileRow')
@TableIndex(name: 'files_state', columns: {#state})
class Files extends Table {
  IntColumn get id => integer().autoIncrement()();
  TextColumn get kind => textEnum<ReleaseKind>()();
  TextColumn get release => text()();

  /// Where it is under the release in the bucket, such as `tiles/2/000/792/230.gph`.
  TextColumn get path => text()();

  /// Where it goes under the offline folder, such as `routing/<date>/tiles/...`.
  TextColumn get installPath => text()();

  /// Its size, when the index says (transfer tiles' sizes aren't published).
  IntColumn get bytes => integer().nullable()();

  /// The SHA-256 to check it against, from the index; for files whose index gives none,
  /// the bucket's `x-amz-meta-sha256` for the object, taken when it downloads.
  TextColumn get expectedSha256 => text().nullable()();

  /// What it hashed to, once it matched.
  TextColumn get verifiedSha256 => text().nullable()();
  TextColumn get state => textEnum<FileState>()();
  IntColumn get attempts => integer().withDefault(const Constant(0))();
  TextColumn get error => text().nullable()();

  /// When a route last needed it (on-demand eviction goes by this), or when it arrived.
  DateTimeColumn get lastUsed => dateTime().nullable()();

  @override
  List<Set<Column>> get uniqueKeys => [
    {kind, release, path},
  ];
}

/// Which regions need which files.
@DataClassName('RegionFileRow')
@TableIndex(name: 'region_files_file', columns: {#fileId})
class RegionFiles extends Table {
  IntColumn get regionId =>
      integer().references(Regions, #id, onDelete: KeyAction.cascade)();
  IntColumn get fileId =>
      integer().references(Files, #id, onDelete: KeyAction.cascade)();

  @override
  Set<Column> get primaryKey => {regionId, fileId};
}

/// The transit feeds a region holds.
@DataClassName('RegionFeedRow')
class RegionFeeds extends Table {
  IntColumn get regionId =>
      integer().references(Regions, #id, onDelete: KeyAction.cascade)();
  TextColumn get feedId => text()();

  /// Its index in the transit build, which the manifest orders feeds by.
  IntColumn get feedIndex => integer()();

  @override
  Set<Column> get primaryKey => {regionId, feedId};
}

/// A byte range of the map archive a region downloads; its tiles go into the shared
/// basemap, so the range itself goes once they're in.
@DataClassName('MapChunkRow')
class MapChunks extends Table {
  IntColumn get id => integer().autoIncrement()();
  IntColumn get regionId =>
      integer().references(Regions, #id, onDelete: KeyAction.cascade)();

  /// From the start of the archive.
  IntColumn get start => integer()();
  IntColumn get length => integer()();
  IntColumn get tileCount => integer()();

  /// The tiles in it (opaque bytes from the planner).
  BlobColumn get tiles => blob()();
  TextColumn get state => textEnum<ChunkState>()();
  IntColumn get attempts => integer().withDefault(const Constant(0))();
  TextColumn get error => text().nullable()();
}

/// What offline downloads keep: `offline.sqlite`, apart from the app's own database.
@DriftDatabase(
  tables: [Releases, Regions, Files, RegionFiles, RegionFeeds, MapChunks],
)
class OfflineDatabase extends _$OfflineDatabase {
  OfflineDatabase(super.executor);

  /// The database in the platform's application support directory.
  OfflineDatabase.open()
    : super(
        driftDatabase(
          name: 'offline',
          native: const DriftNativeOptions(
            databaseDirectory: getApplicationSupportDirectory,
          ),
        ),
      );

  @override
  int get schemaVersion => 1;

  @override
  MigrationStrategy get migration => MigrationStrategy(
    beforeOpen: (details) => customStatement('PRAGMA foreign_keys = ON'),
  );
}
