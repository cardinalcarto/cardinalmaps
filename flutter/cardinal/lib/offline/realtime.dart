/// GTFS-RT for the offline area's feeds, fetched from the public GTFS-RT cache when there's a
/// network. Plans work on the schedule without it.
library;

import 'dart:async';
import 'dart:convert';

import 'package:crypto/crypto.dart';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;
import 'package:routing/routing.dart';

import '../config.dart';
import 'offline_area.dart';

const realtimeExpiry = Duration(minutes: 3);

Uri cachedRealtimeUrl(RealtimeSource source, String cacheUrl) {
  final root = cacheUrl.endsWith('/') ? cacheUrl : '$cacheUrl/';
  final hash = sha256.convert(
    utf8.encode(source.upstreamUrl ?? source.url.toString()),
  );
  return Uri.parse('$root$hash.pb');
}

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
/// Each feed is retried on the same cadence, even after a failure.
class RealtimeFeeds {
  RealtimeFeeds(
    this._sources, {
    http.Client? client,
    this.cacheUrl = realtimeCacheUrl,
    this.maxAge = const Duration(seconds: 60),
    this.timeout = const Duration(seconds: 6),
    this._clock = DateTime.now,
  }) : _client = client ?? http.Client();

  /// The messages to fetch, asked for on each fetch: an area's feeds change as it's
  /// downloaded to.
  final Future<List<RealtimeSource>> Function() _sources;
  final http.Client _client;

  /// The configured public cache root; empty disables real-time downloads.
  final String cacheUrl;

  /// How old a message may be before it's fetched again.
  final Duration maxAge;

  /// How long a fetch may take, so a plan doesn't wait long on a slow network.
  final Duration timeout;

  /// What time it is: the device's clock, or a test's.
  final DateTime Function() _clock;

  final _fetchedAt = <String, DateTime>{};

  /// Expiry is the updater's successful fetch time, not the client's receipt time.
  final _expires = <(String, TransitFeedKind), int>{};
  final _arrived = <(String, TransitFeedKind), TransitFeed>{};

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
    final now = _clock().millisecondsSinceEpoch ~/ 1000;
    for (final key in _expires.keys.toList()) {
      if (now >= _expires[key]!) {
        _expires.remove(key);
        _arrived.remove(key);
      }
    }
    final arrived = _arrived.values.toList();
    _arrived.clear();
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
    if (cacheUrl.isEmpty) return 0;
    final sources = await _sources();
    final now = _clock();
    final due = [
      for (final source in sources)
        if (now.difference(
              _fetchedAt[source.upstreamUrl ?? source.url.toString()] ??
                  DateTime(0),
            ) >=
            maxAge)
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
    return feeds.length;
  }

  Future<TransitFeed?> _fetchOne(RealtimeSource source) async {
    _fetchedAt[source.upstreamUrl ?? source.url.toString()] = _clock();
    final http.Response response;
    try {
      response = await _client
          .get(cachedRealtimeUrl(source, cacheUrl))
          .timeout(timeout);
    } on Object {
      return null;
    }
    if (response.statusCode != 200 || response.bodyBytes.isEmpty) return null;
    final seconds = int.tryParse(
      response.headers['x-amz-meta-fetched-at'] ?? '',
    );
    final now = _clock().millisecondsSinceEpoch ~/ 1000;
    if (seconds == null ||
        seconds < 0 ||
        seconds > now ||
        now - seconds >= realtimeExpiry.inSeconds) {
      return null;
    }
    final expires = seconds + realtimeExpiry.inSeconds;
    final key = (source.feed, source.kind);
    // A delayed or repeatedly served object must not replace a newer update.
    final previous = _expires[key];
    if (previous != null && expires <= previous) return null;
    final feed = TransitFeed(
      feed: source.feed,
      kind: source.kind,
      bytes: response.bodyBytes,
      expiresAt: expires,
    );
    _expires[key] = expires;
    _arrived[key] = feed;
    return feed;
  }
}
