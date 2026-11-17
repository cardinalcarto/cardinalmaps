import 'package:drift/drift.dart';
import 'package:drift_flutter/drift_flutter.dart';
import 'package:path_provider/path_provider.dart';

part 'database.g.dart';

/// The kinds of release, as `latest.json` names them.
///
/// `geocoder` is the geocoder's models (`tagger.model`, `categorical/<lang>.amcats`),
/// pinned on their own when they're first downloaded ([RegionKind.geocoderModels]).
enum ReleaseKind { map, routing, transit, geocoder }

/// What a region is for.
enum RegionKind {
  /// A polygon the user downloaded. Its files are never evicted.
  explicit,

  /// The built-in region of routing tiles fetched as routes need them (no polygon). Files
  /// only it holds are evicted, least recently used first, when space is needed.
  onDemand,

  /// The geocoder's models for offline search (no polygon), downloaded once. Never
  /// evicted.
  geocoderModels,
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
/// small JSON skips this) → `downloaded` (whole, in `.incoming/`) → `verified` (checked)
/// → `installed` (renamed to its place).
/// A failed attempt counts in `attempts` and goes back to `pending`, or to `failed` after
/// the last.
///
/// A file that comes out of an archive (a transfer tile) goes from `pending` straight to
/// `verified` when a range holding it is checked and unpacked; its range has the states.
enum FileState { pending, queued, downloaded, verified, installed, failed }

/// Where a range of an archive is:
///
/// `pending` → `queued` → `downloaded` → `unpacked` (its tiles are in the basemap, or
/// checked and written loose) → `deleted` (its file is gone). Failures go back to
/// `pending`, or to `failed`.
enum ChunkState { pending, queued, downloaded, unpacked, deleted, failed }

/// What a range's tiles are for.
enum ChunkKind {
  /// Map tiles, for the shared basemap.
  map,

  /// Transfer tiles, written loose as the files ([ChunkFiles]) they are.
  transfers,
}

/// The releases downloads are pinned to, one per kind: the first download pins those
/// `latest.json` names, and later ones use them.
@DataClassName('ReleaseRow')
class Releases extends Table {
  TextColumn get kind => textEnum<ReleaseKind>()();

  /// As in the bucket's paths: `YYYY-MM-DD`, or `YYYY-MM-DD.N`.
  TextColumn get date => text()();

  /// Routing and transit: the Valhalla build their tiles and walks are from.
  IntColumn get valhallaBuildId => integer().nullable()();

  /// Transit: the build's manifest ID, which its transfer tiles carry.
  TextColumn get manifestId => text().nullable()();

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

  /// Its size, from the index or the archive's directory.
  IntColumn get bytes => integer().nullable()();

  /// The archive it comes out of (`transit/<date>/transfers/<mode>.pmtiles`), for tiles
  /// downloaded in ranges ([ChunkFiles]) rather than one by one.
  TextColumn get archive => text().nullable()();

  /// The SHA-256 to check it against, from the index (none for JSON, which is checked by
  /// reading it, or transfer tiles, which are checked by decoding them).
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

/// The archives ranges are downloaded from, with the ETag each range must match
/// (`If-Match`), pinned with the releases.
@DataClassName('ArchiveRow')
class Archives extends Table {
  /// Under `releases/`, such as `map/<date>/planet.pmtiles`.
  TextColumn get path => text()();
  TextColumn get etag => text()();

  @override
  Set<Column> get primaryKey => {path};
}

/// A byte range of an archive a region downloads: map tiles go into the shared basemap,
/// transfer tiles are written loose; the range itself goes once they're out of it.
@DataClassName('ChunkRow')
class Chunks extends Table {
  IntColumn get id => integer().autoIncrement()();
  IntColumn get regionId =>
      integer().references(Regions, #id, onDelete: KeyAction.cascade)();
  TextColumn get kind => textEnum<ChunkKind>()();

  /// The archive, as in [Archives].
  TextColumn get archive => text()();

  /// From the start of the archive.
  IntColumn get start => integer()();
  IntColumn get length => integer()();
  IntColumn get tileCount => integer()();

  /// Map tiles: those in it (opaque bytes from the planner).
  BlobColumn get tiles => blob().nullable()();
  TextColumn get state => textEnum<ChunkState>()();
  IntColumn get attempts => integer().withDefault(const Constant(0))();
  TextColumn get error => text().nullable()();
}

/// The files in a range of transfer tiles, at their offsets from its start.
@DataClassName('ChunkFileRow')
@TableIndex(name: 'chunk_files_file', columns: {#fileId})
class ChunkFiles extends Table {
  IntColumn get chunkId =>
      integer().references(Chunks, #id, onDelete: KeyAction.cascade)();
  IntColumn get fileId =>
      integer().references(Files, #id, onDelete: KeyAction.cascade)();
  IntColumn get start => integer()();
  IntColumn get length => integer()();

  @override
  Set<Column> get primaryKey => {chunkId, fileId};
}

/// What offline downloads keep: `offline.sqlite`, apart from the app's own database.
@DriftDatabase(
  tables: [
    Releases,
    Regions,
    Files,
    RegionFiles,
    RegionFeeds,
    Archives,
    Chunks,
    ChunkFiles,
  ],
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
