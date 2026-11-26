import 'dart:async';

import 'package:cardinal/offline/offline_area.dart';
import 'package:cardinal/offline/realtime.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:routing/routing.dart';

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
      cachedRealtimeUrl(source('https://example.org/trip.pb')).toString(),
      '${realtimeCacheUrl}a5dffcef4b30cc6eb129bdceefea1a4930bfd8371943d4143eb54779f3d9a6ad.pb',
    );
    final spelled = RealtimeSource(
      feed: 'f',
      url: Uri.parse('https://example.org/trip.pb'),
      upstreamUrl: 'https://EXAMPLE.org/trip.pb',
    );
    expect(
      cachedRealtimeUrl(spelled),
      isNot(cachedRealtimeUrl(source('https://example.org/trip.pb'))),
    );
  });

  test('fetches each message, then only once it is old', () async {
    var fetches = 0;
    final feeds = RealtimeFeeds(
      () => Future.value([
        source('https://a.test/alerts.pb'),
        source('https://a.test/tripupdates.pb'),
      ]),
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
    'uses the cache, polls at 60 seconds, and expires at 3 minutes',
    () async {
      final sourceFeed = source('https://a.test/alerts.pb');
      var now = DateTime.utc(2026, 11, 13, 8);
      final published = now;
      var fetches = 0;
      final feeds = RealtimeFeeds(
        () => Future.value([sourceFeed]),
        clock: () => now,
        client: MockClient((request) async {
          expect(request.url, cachedRealtimeUrl(sourceFeed));
          fetches++;
          return reply([1], published);
        }),
      );
      final initial = (await feeds.fresh()).single;
      expect(initial.bytes, [1]);
      expect(initial.expiresAt, published.millisecondsSinceEpoch ~/ 1000 + 180);
      now = now.add(const Duration(seconds: 59));
      await feeds.prefetch();
      expect(fetches, 1);
      now = now.add(const Duration(seconds: 1));
      await feeds.prefetch();
      expect(fetches, 2);
      expect(feeds.arrived().feeds, isEmpty);
      now = published.add(const Duration(seconds: 180));
      await feeds.prefetch();
      expect(feeds.arrived().feeds, isEmpty);
      expect(feeds.arrived().feeds, isEmpty);
    },
  );

  test('failed refresh does not extend the expiry', () async {
    var now = DateTime.utc(2026, 11, 13, 8);
    var up = true;
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      clock: () => now,
      client: MockClient((request) async {
        if (!up) throw http.ClientException('no network');
        return reply([1], now);
      }),
    );
    expect(await feeds.fresh(), hasLength(1));
    up = false;
    now = now.add(const Duration(seconds: 60));
    await feeds.prefetch();
    expect(feeds.arrived().feeds, isEmpty);
    now = now.add(const Duration(seconds: 120));
    await feeds.prefetch();
    expect(feeds.arrived().feeds, isEmpty);
    up = true;
    now = now.add(const Duration(seconds: 60));
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
      reply([1], now.add(const Duration(seconds: 1))),
    ]) {
      final feeds = RealtimeFeeds(
        () => Future.value([source('https://a.test/alerts.pb')]),
        clock: () => now,
        client: MockClient((request) async => response),
      );
      expect(await feeds.fresh(), isEmpty);
    }
  });

  test('calls during the first fetch all wait for it', () async {
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      client: MockClient((request) async => reply([1])),
    );
    final (a, b) = await (feeds.fresh(), feeds.fresh()).wait;
    expect(a.length + b.length, 1);
  });

  test('a prefetch leaves what it fetched for the next plan', () async {
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      client: MockClient((request) async => reply([1])),
    );
    await feeds.prefetch();
    expect(await feeds.fresh(), hasLength(1));
    expect(await feeds.fresh(), isEmpty);
  });

  test(
    'arrived never waits, and gives the first fetch while it is under way',
    () async {
      final reply = Completer<http.Response>();
      final realtime = RealtimeFeeds(
        () => Future.value([source('https://a.test/alerts.pb')]),
        client: MockClient((request) => reply.future),
      );
      final (:feeds, :first) = realtime.arrived();
      expect(feeds, isEmpty);
      expect(first, isNotNull);
      reply.complete(
        http.Response.bytes(
          [1],
          200,
          headers: timestampHeaders(DateTime.now()),
        ),
      );
      expect(await first, isTrue);
      final after = realtime.arrived();
      expect(after.feeds, hasLength(1));
      expect(after.first, isNull);
    },
  );

  test('a first fetch that brings nothing says so', () async {
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      client: MockClient((request) async => throw http.ClientException('no')),
    );
    expect(await feeds.arrived().first, isFalse);
    expect(feeds.arrived().feeds, isEmpty);
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
