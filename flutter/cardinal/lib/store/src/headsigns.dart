import 'package:flutter/foundation.dart' show immutable;

import 'database.dart';

/// A GTFS route at one of its stops: where a headsign is remembered. `route_id` and
/// `stop_id` are only unique within their feed.
typedef HeadsignStop = ({String feed, String routeId, String stopId});

/// The headsign a rider last settled on at a [HeadsignStop], and when.
@immutable
class RememberedHeadsign {
  const RememberedHeadsign(this.headsign, this.chosenAt);

  /// The headsign; empty for the departures that have none.
  final String headsign;
  final DateTime chosenAt;

  @override
  bool operator ==(Object other) =>
      other is RememberedHeadsign &&
      other.headsign == headsign &&
      other.chosenAt == chosenAt;

  @override
  int get hashCode => Object.hash(headsign, chosenAt);
}

/// The headsign the rider last settled on for each route at each stop, as the Transit
/// app remembers it, so a nearby row opens on that headsign's page there. Kept in the
/// database (or in memory, for tests).
abstract class HeadsignStore {
  factory HeadsignStore(CardinalDatabase db) = _DatabaseHeadsigns;

  /// Headsigns that last only as long as the store, for tests.
  factory HeadsignStore.inMemory() = _MemoryHeadsigns;

  /// What's remembered at each of [stops] that has anything.
  Future<Map<HeadsignStop, RememberedHeadsign>> recall(
    Iterable<HeadsignStop> stops,
  );

  /// Remembers [headsign] at [stop], in place of what was, as chosen at [chosenAt].
  Future<void> remember(
    HeadsignStop stop,
    String headsign, {
    required DateTime chosenAt,
  });
}

class _DatabaseHeadsigns implements HeadsignStore {
  _DatabaseHeadsigns(this._db);

  final CardinalDatabase _db;

  @override
  Future<Map<HeadsignStop, RememberedHeadsign>> recall(
    Iterable<HeadsignStop> stops,
  ) async {
    final wanted = stops.toSet();
    if (wanted.isEmpty) return const {};
    final t = _db.rememberedHeadsigns;
    // By route, then the stops asked for among them: a route has few rows.
    final rows = await (_db.select(
      t,
    )..where((r) => r.routeId.isIn({for (final s in wanted) s.routeId}))).get();
    return {
      for (final row in rows)
        if (wanted.contains((
          feed: row.feed,
          routeId: row.routeId,
          stopId: row.stopId,
        )))
          (feed: row.feed, routeId: row.routeId, stopId: row.stopId):
              RememberedHeadsign(row.headsign, row.chosenAt),
    };
  }

  @override
  Future<void> remember(
    HeadsignStop stop,
    String headsign, {
    required DateTime chosenAt,
  }) => _db
      .into(_db.rememberedHeadsigns)
      .insertOnConflictUpdate(
        RememberedHeadsignRow(
          feed: stop.feed,
          routeId: stop.routeId,
          stopId: stop.stopId,
          headsign: headsign,
          chosenAt: chosenAt,
        ),
      );
}

class _MemoryHeadsigns implements HeadsignStore {
  final _remembered = <HeadsignStop, RememberedHeadsign>{};

  @override
  Future<Map<HeadsignStop, RememberedHeadsign>> recall(
    Iterable<HeadsignStop> stops,
  ) async => {for (final stop in stops) stop: ?_remembered[stop]};

  @override
  Future<void> remember(
    HeadsignStop stop,
    String headsign, {
    required DateTime chosenAt,
  }) async => _remembered[stop] = RememberedHeadsign(headsign, chosenAt);
}
