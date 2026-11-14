/// GTFS-RT for the offline area's feeds, fetched straight from each agency when there's a
/// network. Plans work on the schedule without it.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;
import 'package:routing/routing.dart';

import 'offline_area.dart';

/// One [RealtimeFeeds] for the current offline area, shared by everything that asks its
/// transit planner (directions and nearby transit), so they fetch each message once.
///
/// The planner keeps what it's given whichever question it came with, so a message one
/// asker took serves the others too. A new area gets new feeds.
class AreaRealtimeFeeds {
  AreaRealtimeFeeds({this.client});

  /// What real-time messages are fetched with.
  final http.Client? client;

  /// The real-time feeds of the area they were made for.
  (OfflineArea, RealtimeFeeds)? _feeds;

  /// The real-time feeds of [area].
  RealtimeFeeds of(OfflineArea area) {
    if (_feeds case (final made, final feeds) when made == area) return feeds;
    final feeds = RealtimeFeeds(() => area.realtimeSources, client: client);
    _feeds = (area, feeds);
    return feeds;
  }
}

/// The latest message of each [RealtimeSource], fetched again once it's [maxAge] old.
///
/// The planner keeps what it was given until it ages out, so each plan only needs the
/// messages fetched since the last one. A message that can't be fetched is left out:
/// whatever the planner already holds for it stays until it's stale.
class RealtimeFeeds {
  RealtimeFeeds(
    this._sources, {
    http.Client? client,
    this.maxAge = const Duration(seconds: 30),
    this.timeout = const Duration(seconds: 6),
  }) : _client = client ?? http.Client();

  /// The messages to fetch, asked for on each fetch: an area's feeds change as it's
  /// downloaded to.
  final Future<List<RealtimeSource>> Function() _sources;
  final http.Client _client;

  /// How old a message may be before it's fetched again.
  final Duration maxAge;

  /// How long a fetch may take, so a plan doesn't wait long on a slow network.
  final Duration timeout;

  final _fetchedAt = <Uri, DateTime>{};

  /// Messages fetched and not yet handed to a plan.
  var _arrived = <TransitFeed>[];
  Future<void>? _inFlight;

  /// Whether a fetch has finished, so plans no longer wait for one.
  var _firstDone = false;

  /// The messages that arrived since the last call, for the next plan, after starting a
  /// fetch of those older than [maxAge].
  ///
  /// Until a first fetch has finished, this waits for it, so the first plan has live
  /// data if it's to be had. After that it never waits: a plan takes what has arrived,
  /// and what's being fetched goes to the next one.
  Future<List<TransitFeed>> fresh() async {
    final fetch = prefetch();
    if (!_firstDone) await fetch;
    final arrived = _arrived;
    _arrived = [];
    return arrived;
  }

  /// Starts fetching the messages older than [maxAge], if that isn't under way, for the
  /// next plan to take with [fresh]. Completes when the fetch does.
  Future<void> prefetch() => _inFlight ??= _fetch().whenComplete(() {
    _inFlight = null;
    _firstDone = true;
  });

  Future<void> _fetch() async {
    final sources = await _sources();
    final now = DateTime.now();
    final due = [
      for (final source in sources)
        if (now.difference(_fetchedAt[source.url] ?? DateTime(0)) >= maxAge)
          source,
    ];
    if (due.isEmpty) return;
    final watch = Stopwatch()..start();
    final fetched = await Future.wait(due.map(_fetchOne));
    final feeds = fetched.nonNulls.toList();
    debugPrint(
      'cardinal-timing: realtime fetch ${feeds.length}/${due.length} '
      'in ${watch.elapsedMilliseconds} ms',
    );
    _arrived.addAll(feeds);
  }

  Future<TransitFeed?> _fetchOne(RealtimeSource source) async {
    try {
      final response = await _client.get(source.url).timeout(timeout);
      if (response.statusCode != 200 || response.bodyBytes.isEmpty) {
        debugPrint('realtime: ${source.url} answered ${response.statusCode}');
        return null;
      }
      _fetchedAt[source.url] = DateTime.now();
      return TransitFeed(
        feed: source.feed,
        kind: source.kind,
        bytes: response.bodyBytes,
      );
    } on Object catch (e) {
      // No network, or the agency is down: the plan uses what it has.
      debugPrint('realtime: ${source.url}: $e');
      return null;
    }
  }
}
