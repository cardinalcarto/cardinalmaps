import 'dart:async';

import 'package:cardinal/offline/offline_area.dart';
import 'package:cardinal/offline/realtime.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:routing/routing.dart';

const cacheUrl = 'https://cache.example.org/gtfs-rt';

RealtimeSource source(String url) =>
    RealtimeSource(feed: 'f', url: Uri.parse(url));

Map<String, String> timestampHeaders(DateTime time) => {
  'x-amz-meta-fetched-at': '${time.millisecondsSinceEpoch ~/ 1000}',
};

http.Response reply(List<int> bytes, [DateTime? time]) => http.Response.bytes(
  bytes,
  200,
  headers: timestampHeaders(time ?? DateTime.now()),
);

void main() {
  test("a message's kind is read from its URL", () {
    expect(
      source('https://s3.amazonaws.com/kcm/alerts.pb').kind,
      TransitFeedKind.alerts,
    );
    expect(
      source('https://s3.amazonaws.com/kcm/tripupdates.pb').kind,
      TransitFeedKind.tripUpdates,
    );
    expect(
      source('https://s3.amazonaws.com/ct/vehiclepositions.pb').kind,
      TransitFeedKind.vehiclePositions,
    );
    expect(
      source(
        'https://birch.catenarymaps.org/gtfs_rt?feed_id=f-x~rt&feed_type=trip',
      ).kind,
      TransitFeedKind.tripUpdates,
    );
    expect(
      source('https://example.org/gtfs-rt').kind,
      TransitFeedKind.combined,
    );
    // The bucket's name isn't what's in it.
    expect(
      source('https://s3.amazonaws.com/kcm-alerts-realtime-prod/tripupdates.pb')
          .kind,
      TransitFeedKind.tripUpdates,
    );
    expect(
      source('https://s3.amazonaws.com/kcm-alerts-realtime-prod/alerts.pb')
          .kind,
      TransitFeedKind.alerts,
    );
  });

  test('cache keys match the updater and preserve exact manifest spelling', () {
    expect(
      cachedRealtimeUrl(
        source('https://example.org/trip.pb'),
        cacheUrl,
      ).toString(),
      'https://cache.example.org/gtfs-rt/a5dffcef4b30cc6eb129bdceefea1a4930bfd8371943d4143eb54779f3d9a6ad.pb',
    );
    expect(
      cachedRealtimeUrl(source('https://example.org/trip.pb'), '$cacheUrl/'),
      cachedRealtimeUrl(source('https://example.org/trip.pb'), cacheUrl),
    );
    final spelled = RealtimeSource(
      feed: 'f',
      url: Uri.parse('https://example.org/trip.pb'),
      upstreamUrl: 'https://EXAMPLE.org/trip.pb',
    );
    expect(
      cachedRealtimeUrl(spelled, cacheUrl),
      isNot(cachedRealtimeUrl(source('https://example.org/trip.pb'), cacheUrl)),
    );
  });

  test('an unconfigured cache does not fetch agency URLs', () async {
    var requests = 0;
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      cacheUrl: '',
      client: MockClient((request) async {
        requests++;
        return reply([1]);
      }),
    );
    expect(await feeds.fresh(), isEmpty);
    expect(requests, 0);
  });

  test('fetches each message, then only once it is old', () async {
    var fetches = 0;
    final feeds = RealtimeFeeds(
      () => Future.value([
        source('https://a.test/alerts.pb'),
        source('https://a.test/tripupdates.pb'),
      ]),
      cacheUrl: cacheUrl,
      client: MockClient((request) async {
        fetches++;
        return reply([1, 2, 3]);
      }),
    );
    final first = await feeds.fresh();
    expect(first.map((f) => f.kind), [
      TransitFeedKind.alerts,
      TransitFeedKind.tripUpdates,
    ]);
    expect(await feeds.fresh(), isEmpty);
    expect(fetches, 2);
  });

  test(
    'uses the cache, polls at 60 seconds, and expires at 5 minutes',
    () async {
      final sourceFeed = source('https://a.test/alerts.pb');
      var now = DateTime.utc(2026, 11, 13, 8);
      final published = now;
      var fetches = 0;
      final feeds = RealtimeFeeds(
        () => Future.value([sourceFeed]),
        cacheUrl: cacheUrl,
        clock: () => now,
        client: MockClient((request) async {
          expect(request.url, cachedRealtimeUrl(sourceFeed, cacheUrl));
          fetches++;
          return reply([1], published);
        }),
      );
      final initial = (await feeds.fresh()).single;
      expect(initial.bytes, [1]);
      expect(initial.expiresAt, published.millisecondsSinceEpoch ~/ 1000 + 300);
      now = now.add(const Duration(seconds: 59));
      await feeds.prefetch();
      expect(fetches, 1);
      now = now.add(const Duration(seconds: 1));
      await feeds.prefetch();
      expect(fetches, 2);
      expect(feeds.arrived().feeds, isEmpty);
      now = published.add(const Duration(seconds: 300));
      await feeds.prefetch();
      expect(feeds.arrived().feeds, isEmpty);
      expect(feeds.arrived().feeds, isEmpty);
    },
  );

  test('failed refresh does not extend the expiry', () async {
    var now = DateTime.utc(2026, 11, 13, 8);
    var up = true;
    var requests = 0;
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      cacheUrl: cacheUrl,
      clock: () => now,
      client: MockClient((request) async {
        requests++;
        if (!up) throw http.ClientException('no network');
        return reply([1], now);
      }),
    );
    expect(await feeds.fresh(), hasLength(1));
    up = false;
    now = now.add(const Duration(seconds: 60));
    await feeds.prefetch();
    expect(feeds.arrived().feeds, isEmpty);
    expect(requests, 2);
    now = now.add(const Duration(seconds: 14));
    await feeds.prefetch();
    expect(requests, 2);
    now = now.add(const Duration(seconds: 1));
    await feeds.prefetch();
    expect(requests, 3);
    now = now.add(const Duration(seconds: 225));
    await feeds.prefetch();
    expect(feeds.arrived().feeds, isEmpty);
    up = true;
    now = now.add(const Duration(seconds: 15));
    await feeds.prefetch();
    expect(feeds.arrived().feeds.single.bytes, [1]);
  });

  test('rejects stale or missing cache timestamps', () async {
    final now = DateTime.utc(2026, 11, 13, 8);
    for (final response in [
      http.Response.bytes([1], 200),
      http.Response.bytes(
        [1],
        200,
        headers: {'x-amz-meta-fetched-at': '8640000000000'},
      ),
      reply([1], now.subtract(realtimeExpiry)),
      reply([1], now.add(const Duration(seconds: 31))),
    ]) {
      final feeds = RealtimeFeeds(
        () => Future.value([source('https://a.test/alerts.pb')]),
        cacheUrl: cacheUrl,
        clock: () => now,
        client: MockClient((request) async => response),
      );
      expect(await feeds.fresh(), isEmpty);
    }
  });

  test(
    'accepts small future clock skew without changing absolute expiry',
    () async {
      final now = DateTime.utc(2026, 11, 13, 8);
      final published = now.add(const Duration(seconds: 30));
      final feeds = RealtimeFeeds(
        () => Future.value([source('https://a.test/tripupdates.pb')]),
        cacheUrl: cacheUrl,
        clock: () => now,
        client: MockClient((request) async => reply([1], published)),
      );
      expect(
        (await feeds.fresh()).single.expiresAt,
        published.millisecondsSinceEpoch ~/ 1000 + 300,
      );
    },
  );

  test('failed feeds retry independently of successful feeds', () async {
    var now = DateTime.utc(2026, 11, 13, 8);
    final good = source('https://a.test/tripupdates.pb');
    final bad = source('https://a.test/alerts.pb');
    final requests = <Uri>[];
    var recovered = false;
    final feeds = RealtimeFeeds(
      () => Future.value([good, bad]),
      cacheUrl: cacheUrl,
      clock: () => now,
      client: MockClient((request) async {
        requests.add(request.url);
        if (request.url == cachedRealtimeUrl(bad, cacheUrl) && !recovered) {
          return http.Response('', 503);
        }
        return reply([1], now);
      }),
    );
    expect(await feeds.fresh(), hasLength(1));
    requests.clear();
    now = now.add(const Duration(seconds: 15));
    recovered = true;
    await feeds.prefetch();
    expect(requests, [cachedRealtimeUrl(bad, cacheUrl)]);
    expect(feeds.arrived().feeds.single.kind, TransitFeedKind.alerts);
    requests.clear();
    now = now.add(const Duration(seconds: 15));
    await feeds.prefetch();
    expect(requests, isEmpty);
  });

  test('older cached objects cannot replace a newer pending message', () async {
    var now = DateTime.utc(2026, 11, 13, 8);
    final published = now;
    var timestamp = published;
    var bytes = [1];
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/tripupdates.pb')]),
      cacheUrl: cacheUrl,
      clock: () => now,
      client: MockClient((request) async => reply(bytes, timestamp)),
    );
    await feeds.prefetch();
    now = now.add(const Duration(seconds: 60));
    timestamp = published.subtract(const Duration(seconds: 1));
    bytes = [2];
    await feeds.prefetch();
    expect(feeds.arrived().feeds.single.bytes, [1]);
    now = now.add(const Duration(seconds: 15));
    timestamp = now;
    bytes = [3];
    await feeds.prefetch();
    expect(feeds.arrived().feeds.single.bytes, [3]);
  });

  test('pending messages expire at exactly five minutes', () async {
    final published = DateTime.utc(2026, 11, 13, 8);
    for (final age in [299, 300]) {
      var now = published;
      final feeds = RealtimeFeeds(
        () => Future.value([source('https://a.test/tripupdates.pb')]),
        cacheUrl: cacheUrl,
        clock: () => now,
        client: MockClient((request) async => reply([1], published)),
      );
      await feeds.prefetch();
      now = published.add(Duration(seconds: age));
      expect(feeds.arrived().feeds, hasLength(age == 299 ? 1 : 0));
      await feeds.prefetch();
    }
  });

  test('calls during the first fetch all wait for it', () async {
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      cacheUrl: cacheUrl,
      client: MockClient((request) async => reply([1])),
    );
    final (a, b) = await (feeds.fresh(), feeds.fresh()).wait;
    expect(a.length + b.length, 1);
  });

  test('a prefetch leaves what it fetched for the next plan', () async {
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      cacheUrl: cacheUrl,
      client: MockClient((request) async => reply([1])),
    );
    await feeds.prefetch();
    expect(await feeds.fresh(), hasLength(1));
    expect(await feeds.fresh(), isEmpty);
  });

  test(
    'arrived never waits, and reports every fetch while it is under way',
    () async {
      var now = DateTime.utc(2026, 11, 13, 8);
      var reply = Completer<http.Response>();
      var requests = 0;
      final realtime = RealtimeFeeds(
        () => Future.value([source('https://a.test/alerts.pb')]),
        cacheUrl: cacheUrl,
        clock: () => now,
        client: MockClient((request) {
          requests++;
          return reply.future;
        }),
      );
      final (:feeds, :pending) = realtime.arrived();
      expect(feeds, isEmpty);
      reply.complete(
        http.Response.bytes([1], 200, headers: timestampHeaders(now)),
      );
      expect(await pending, isTrue);
      final after = realtime.arrived();
      expect(after.feeds, hasLength(1));
      expect(await after.pending, isFalse);

      now = now.add(const Duration(seconds: 60));
      reply = Completer<http.Response>();
      final refresh = realtime.arrived();
      final concurrent = realtime.arrived();
      expect(refresh.feeds, isEmpty);
      expect(concurrent.feeds, isEmpty);
      reply.complete(
        http.Response.bytes([2], 200, headers: timestampHeaders(now)),
      );
      expect(await refresh.pending, isTrue);
      expect(await concurrent.pending, isTrue);
      expect(requests, 2);
      final refreshed = realtime.arrived();
      expect(refreshed.feeds.single.bytes, [2]);
      expect(await refreshed.pending, isFalse);

      // A duplicate (unchanged timestamp) brings no new data and must not trigger a requery.
      now = now.add(const Duration(seconds: 60));
      final duplicate = realtime.arrived();
      expect(await duplicate.pending, isFalse);
      expect(duplicate.feeds, isEmpty);
      expect(requests, 3);
    },
  );

  test('a failed first fetch can notify its subsequent recovery', () async {
    var now = DateTime.utc(2026, 11, 13, 8);
    var up = false;
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      cacheUrl: cacheUrl,
      clock: () => now,
      client: MockClient((request) async {
        if (!up) throw http.ClientException('no');
        return reply([1], now);
      }),
    );
    expect(await feeds.arrived().pending, isFalse);
    expect(feeds.arrived().feeds, isEmpty);
    await feeds.prefetch();
    up = true;
    now = now.add(const Duration(seconds: 15));
    expect(await feeds.arrived().pending, isTrue);
    final recovered = feeds.arrived();
    expect(recovered.feeds.single.bytes, [1]);
    expect(await recovered.pending, isFalse);
  });

  test('an area has one set of feeds, and a new area new ones', () {
    OfflineArea area() => OfflineArea(
      routing: '/nonexistent/routing/r',
      transitBuild: '/nonexistent/transit/t',
    );
    final shared = AreaRealtimeFeeds();
    final first = area();
    final feeds = shared.of(first);
    expect(shared.of(first), same(feeds));
    final second = area();
    expect(shared.of(second), isNot(same(feeds)));
    expect(shared.of(second), same(shared.of(second)));
  });
}
