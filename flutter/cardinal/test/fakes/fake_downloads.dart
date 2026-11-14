import 'dart:async';
import 'dart:typed_data';

import 'package:cardinal/offline/downloads/database.dart';
import 'package:cardinal/offline/downloads/engine.dart';
import 'package:cardinal/offline/downloads/geometry.dart';
import 'package:cardinal/offline/downloads/native.dart';
import 'package:cardinal/offline/downloads/plan.dart';

/// Offline downloads without a database, a bucket or files, for widget tests: regions
/// are whatever the test [emit]s, plans are what [onPlan] gives, and what the page asks
/// for is recorded.
class FakeOfflineDownloads implements OfflineDownloads {
  final _regions = StreamController<List<RegionProgress>>.broadcast();
  List<RegionProgress> _current = const [];

  /// What [plan] answers with; by default [fakeAreaPlan] of the polygon.
  Future<OfflinePlan> Function(GeoPolygon polygon)? onPlan;

  /// The polygons planned, in order.
  final planned = <GeoPolygon>[];

  /// What [start] was given, in order.
  final started = <({OfflinePlan plan, Set<String> feeds, String? name})>[];
  final retried = <int>[];
  final deleted = <int>[];

  /// Shows [regions] to whoever watches.
  void emit(List<RegionProgress> regions) {
    _current = regions;
    _regions.add(regions);
  }

  @override
  Stream<List<RegionProgress>> watch() async* {
    yield _current;
    yield* _regions.stream;
  }

  @override
  Future<List<RegionProgress>> regions() async => _current;

  @override
  Future<OfflinePlan> plan(GeoPolygon polygon) async {
    planned.add(polygon);
    return (onPlan ?? (p) async => fakeAreaPlan(p))(polygon);
  }

  @override
  Future<int> start(
    OfflinePlan plan, {
    Set<String> feeds = const {},
    String? name,
  }) async {
    started.add((plan: plan, feeds: feeds, name: name));
    return started.length;
  }

  @override
  Future<void> retry(int region) async => retried.add(region);

  @override
  Future<void> delete(int region) async => deleted.add(region);

  @override
  Object? noSuchMethod(Invocation invocation) => throw UnimplementedError(
    'FakeOfflineDownloads: ${invocation.memberName}',
  );
}

/// A transit feed to choose, of [bytes] of its own files, with transfer tiles
/// ([transfers], by mode) it may share with other feeds, and [cellsInside] stop cells
/// in the area.
CandidateFeed fakeFeed(
  String id, {
  int index = 0,
  String? displayName,
  List<String>? agencies,
  int bytes = 0,
  Map<String, List<(int, int, int)>> transfers = const {},
  int cellsInside = 1,
}) => CandidateFeed(
  id: id,
  index: index,
  displayName: displayName ?? id,
  agencies: agencies ?? [id],
  firstServiceDate: null,
  lastServiceDate: null,
  files: [
    PlannedFile(
      kind: ReleaseKind.transit,
      release: '2026-11-11',
      path: 'timetables/$id.timetable',
      installPath: 'transit/2026-11-11/timetables/$id.timetable',
      bytes: bytes,
    ),
  ],
  transferTiles: transfers,
  cellsInside: cellsInside,
);

/// A plan for [polygon] of [routingBytes] of routing tiles and [mapBytes] of map
/// ranges, with [feeds] to choose from (by ID; of no size), or [candidates], whose
/// shared files are [transitSharedBytes].
OfflinePlan fakeAreaPlan(
  GeoPolygon polygon, {
  int routingBytes = 90 * 1000 * 1000,
  int mapBytes = 46 * 1000 * 1000,
  List<String> feeds = const [],
  List<CandidateFeed>? candidates,
  int transitSharedBytes = 0,
}) => OfflinePlan(
  polygon: polygon,
  releases: const PlanReleases(
    map: '2026-11-11',
    routing: '2026-11-11',
    transit: '2026-11-11',
    valhallaBuildId: 1,
    mapEtag: 'etag',
    pinned: false,
  ),
  routingFiles: [
    PlannedFile(
      kind: ReleaseKind.routing,
      release: '2026-11-11',
      path: 'tiles/2/000/000/001.gph',
      installPath: 'routing/2026-11-11/tiles/2/000/000/001.gph',
      bytes: routingBytes,
    ),
  ],
  transitShared: [
    PlannedFile(
      kind: ReleaseKind.transit,
      release: '2026-11-11',
      path: 'manifest.json',
      installPath: 'transit/2026-11-11/manifest.json',
      bytes: transitSharedBytes,
    ),
  ],
  feeds:
      candidates ??
      [for (final (i, id) in feeds.indexed) fakeFeed(id, index: i)],
  transferArchives: {
    for (final mode in ['walking', 'cycling', 'wheelchair'])
      mode: TransferArchive(
        path: 'transit/2026-11-11/transfers/$mode.pmtiles',
        mode: mode,
        etag: 'etag',
        zoom: 14,
        planning: _NoPlanning(),
      ),
  },
  map: (
    coveringTiles: 1,
    archiveTiles: 1,
    presentTiles: 0,
    tileBytes: mapBytes,
    rangeBytes: mapBytes,
    ranges: const [],
    tileKeys: Uint8List(0),
  ),
  mapHeader: null,
  present: const {},
);

/// A region as the Offline tab sees it.
RegionProgress fakeRegion(
  int id, {
  String? name,
  RegionState state = RegionState.ready,
  RegionKind kind = RegionKind.explicit,
  int bytesDone = 0,
  int bytesTotal = 136 * 1000 * 1000,
  DateTime? createdAt,
}) => RegionProgress(
  id: id,
  kind: kind,
  name: name,
  polygon: null,
  state: state,
  error: null,
  createdAt: createdAt ?? DateTime(2026, 11, 12, 21, 41),
  bytesDone: bytesDone,
  bytesTotal: bytesTotal,
  filesDone: 0,
  filesTotal: 0,
  failed: 0,
);

/// A transfer archive's planning that's never asked anything: the fake plan's tiles
/// are sized already.
class _NoPlanning implements ArchivePlanning {
  @override
  Object? noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('_NoPlanning: ${invocation.memberName}');
}
