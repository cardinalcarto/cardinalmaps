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

  /// What [planGeocoderModels] answers with; by default [fakeModelsPlan].
  Future<GeocoderModelsPlan?> Function(String? language)? onPlanModels;

  /// The languages the geocoder's models were planned for, in order.
  final plannedModels = <String?>[];

  /// The geocoder model plans started, in order.
  final startedModels = <GeocoderModelsPlan>[];

  @override
  Future<GeocoderModelsPlan?> planGeocoderModels(String? language) async {
    plannedModels.add(language);
    return (onPlanModels ?? (_) async => fakeModelsPlan())(language);
  }

  @override
  Future<int> startGeocoderModels(GeocoderModelsPlan plan) async {
    startedModels.add(plan);
    return 100;
  }

  @override
  Stream<RegionProgress?> watchGeocoderModels() => watch().map(
    (regions) =>
        regions.where((r) => r.kind == RegionKind.geocoderModels).lastOrNull,
  );

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

/// The geocoder's models: a tagger of 106 MB and English categories of 55 MB.
GeocoderModelsPlan fakeModelsPlan({bool categories = true}) =>
    GeocoderModelsPlan(
      release: '2026-11-16',
      tagger: const PlannedFile(
        kind: ReleaseKind.geocoder,
        release: '2026-11-16',
        path: 'tagger.model',
        installPath: 'geocoder/2026-11-16/tagger.model',
        bytes: 105736607,
        sha256: 'a',
      ),
      categories: categories
          ? const PlannedFile(
              kind: ReleaseKind.geocoder,
              release: '2026-11-16',
              path: 'categorical/eng.amcats',
              installPath: 'geocoder/2026-11-16/categorical/eng.amcats',
              bytes: 55242842,
              sha256: 'b',
            )
          : null,
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
  GeoPolygon? polygon,
}) => RegionProgress(
  id: id,
  kind: kind,
  name: name,
  polygon: polygon,
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
