/// GTFS-RT for the offline area's feeds, fetched straight from each agency when there's a
/// network. Plans work on the schedule without it.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;
import 'package:routing/routing.dart';

import 'offline_area.dart';

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

  final Future<List<RealtimeSource>> _sources;
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
    final fetch = _inFlight ??= _fetch().whenComplete(() {
      _inFlight = null;
      _firstDone = true;
    });
    if (!_firstDone) await fetch;
    final arrived = _arrived;
    _arrived = [];
    return arrived;
  }

  Future<void> _fetch() async {
    final sources = await _sources;
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
