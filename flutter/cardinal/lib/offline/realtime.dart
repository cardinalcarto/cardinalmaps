/// GTFS-RT for the offline area's feeds, fetched straight from each agency when there's a
/// network. Plans work on the schedule without it.
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;
import 'package:routing/routing.dart';

import 'offline_area.dart';

/// How long a feed is left alone after its first failure: an answer other than 2xx, no
/// network, or a fetch past [RealtimeFeeds.timeout]. Each failure after that doubles it,
/// up to [realtimeBackoffMax].
const realtimeBackoffInitial = Duration(seconds: 30);

/// The longest a failing feed is left alone between tries.
const realtimeBackoffMax = Duration(minutes: 10);

/// How far a backoff may be moved either way at random, as a fraction of it, so feeds
/// that failed together aren't all tried again together.
const realtimeBackoffJitter = 0.1;

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
///
/// A feed that fails is left alone for a while (see [realtimeBackoffInitial]), longer
/// each time it fails again, until it answers.
class RealtimeFeeds {
  RealtimeFeeds(
    this._sources, {
    http.Client? client,
    this.maxAge = const Duration(seconds: 30),
    this.timeout = const Duration(seconds: 6),
    this._clock = DateTime.now,
    math.Random? random,
  }) : _client = client ?? http.Client(),
       _random = random ?? math.Random();

  /// The messages to fetch, asked for on each fetch: an area's feeds change as it's
  /// downloaded to.
  final Future<List<RealtimeSource>> Function() _sources;
  final http.Client _client;

  /// How old a message may be before it's fetched again.
  final Duration maxAge;

  /// How long a fetch may take, so a plan doesn't wait long on a slow network.
  final Duration timeout;

  /// What time it is: the device's clock, or a test's.
  final DateTime Function() _clock;

  /// What a backoff's jitter is drawn from.
  final math.Random _random;

  final _fetchedAt = <Uri, DateTime>{};

  /// The feeds that failed, and how they're being left alone.
  final _backoffs = <Uri, _Backoff>{};

  /// Messages fetched and not yet handed to a plan.
  var _arrived = <TransitFeed>[];

  /// The fetch under way, completing to how many messages it fetched.
  Future<int>? _inFlight;

  /// The first fetch, until it has finished.
  Future<int>? _first;

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
    return _take();
  }

  /// The messages that arrived since the last call, as [fresh] gives them, but never
  /// waiting, not even for the first fetch: for an answer on the schedule first.
  ///
  /// With them comes the first fetch while it's under way, which completes to whether it
  /// fetched any messages, so the plan can be asked again with them; null once it has
  /// finished. After the first fetch, what's fetched goes to the next plan, as for
  /// [fresh].
  ({List<TransitFeed> feeds, Future<bool>? first}) arrived() {
    // Its failure is the plan's: messages that can't be fetched are left out.
    prefetch().ignore();
    final first = _firstDone
        ? null
        : _first?.then((fetched) => fetched > 0, onError: (Object _) => false);
    return (feeds: _take(), first: first);
  }

  List<TransitFeed> _take() {
    final arrived = _arrived;
    _arrived = [];
    return arrived;
  }

  /// Starts fetching the messages older than [maxAge], if that isn't under way, for the
  /// next plan to take with [fresh]. Completes when the fetch does.
  Future<void> prefetch() {
    final fetch = _inFlight ??= _fetch().whenComplete(() {
      _inFlight = null;
      _firstDone = true;
    });
    if (!_firstDone) _first ??= fetch;
    return fetch;
  }

  Future<int> _fetch() async {
    final sources = await _sources();
    final now = _clock();
    final due = [
      for (final source in sources)
        if (now.difference(_fetchedAt[source.url] ?? DateTime(0)) >= maxAge &&
            !(_backoffs[source.url]?.until.isAfter(now) ?? false))
          source,
    ];
    if (due.isEmpty) return 0;
    final watch = Stopwatch()..start();
    final fetched = await Future.wait(due.map(_fetchOne));
    final feeds = fetched.nonNulls.toList();
    debugPrint(
      'cardinal-timing: realtime fetch ${feeds.length}/${due.length} '
      'in ${watch.elapsedMilliseconds} ms',
    );
    _arrived.addAll(feeds);
    return feeds.length;
  }

  Future<TransitFeed?> _fetchOne(RealtimeSource source) async {
    final http.Response response;
    try {
      response = await _client.get(source.url).timeout(timeout);
    } on Object catch (e) {
      // No network, or the agency is down: the plan uses what it has.
      _failed(source.url, '$e');
      return null;
    }
    if (response.statusCode < 200 || response.statusCode >= 300) {
      _failed(source.url, 'answered ${response.statusCode}');
      return null;
    }
    if (_backoffs.remove(source.url) != null) {
      debugPrint('realtime: ${source.url} answered again');
    }
    if (response.statusCode != 200 || response.bodyBytes.isEmpty) return null;
    _fetchedAt[source.url] = _clock();
    return TransitFeed(
      feed: source.feed,
      kind: source.kind,
      bytes: response.bodyBytes,
    );
  }

  /// Leaves [url] alone for longer than last time, after it failed with [why].
  void _failed(Uri url, String why) {
    final failures = (_backoffs[url]?.failures ?? 0) + 1;
    final base = math.min(
      realtimeBackoffInitial.inMilliseconds * math.pow(2, failures - 1),
      realtimeBackoffMax.inMilliseconds.toDouble(),
    );
    final jitter = 1 + realtimeBackoffJitter * (2 * _random.nextDouble() - 1);
    final wait = Duration(milliseconds: (base * jitter).round());
    _backoffs[url] = _Backoff(failures, _clock().add(wait));
    if (failures == 1) {
      debugPrint('realtime: $url $why; backing off');
    }
  }
}

/// A failing feed: how many times in a row it has failed, and when to try it again.
class _Backoff {
  const _Backoff(this.failures, this.until);

  final int failures;
  final DateTime until;
}
