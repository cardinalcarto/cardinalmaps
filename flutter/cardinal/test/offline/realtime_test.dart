import 'dart:async';
import 'dart:math' as math;

import 'package:cardinal/offline/offline_area.dart';
import 'package:cardinal/offline/realtime.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:routing/routing.dart';

RealtimeSource source(String url) =>
    RealtimeSource(feed: 'f', url: Uri.parse(url));

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

  test('fetches each message, then only once it is old', () async {
    var fetches = 0;
    final feeds = RealtimeFeeds(
      () => Future.value([
        source('https://a.test/alerts.pb'),
        source('https://a.test/tripupdates.pb'),
      ]),
      client: MockClient((request) async {
        fetches++;
        return http.Response.bytes([1, 2, 3], 200);
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
    'a message that cannot be fetched is left out, and tried again',
    () async {
      var up = false;
      var now = DateTime(2026, 11, 13, 8);
      final feeds = RealtimeFeeds(
        () => Future.value([source('https://a.test/alerts.pb')]),
        client: MockClient((request) async {
          if (!up) throw http.ClientException('no network');
          return http.Response.bytes([1], 200);
        }),
        clock: () => now,
        random: _Middle(),
      );
      expect(await feeds.fresh(), isEmpty);
      up = true;
      now = now.add(realtimeBackoffInitial);
      // After a first fetch, a plan doesn't wait: what arrives goes to the next one.
      expect(await feeds.fresh(), isEmpty);
      await pumpEventQueue();
      expect(await feeds.fresh(), hasLength(1));
    },
  );

  group('a failing feed', () {
    late DateTime now;
    late Map<String, int> fetches;
    late Map<String, int> status;
    late RealtimeFeeds feeds;

    setUp(() {
      now = DateTime(2026, 11, 13, 8);
      fetches = {};
      status = {};
      feeds = RealtimeFeeds(
        () => Future.value([
          source('https://bad.test/tripupdates.pb'),
          source('https://good.test/tripupdates.pb'),
        ]),
        // Old messages are always fetched again, so only the backoff skips one.
        maxAge: Duration.zero,
        client: MockClient((request) async {
          final host = request.url.host;
          fetches[host] = (fetches[host] ?? 0) + 1;
          return switch (status[host] ?? 200) {
            -1 => throw http.ClientException('no network'),
            final code => http.Response.bytes([1], code),
          };
        }),
        clock: () => now,
        random: _Middle(),
      );
    });

    /// Fetches once, returning how many messages came.
    Future<int> fetch() async {
      await feeds.prefetch();
      return feeds.arrived().feeds.length;
    }

    test('is left alone until its backoff has passed', () async {
      status['bad.test'] = 500;
      expect(await fetch(), 1);
      expect(fetches['bad.test'], 1);

      now = now.add(realtimeBackoffInitial - const Duration(seconds: 1));
      expect(await fetch(), 1);
      expect(fetches['bad.test'], 1, reason: 'skipped during the backoff');

      now = now.add(const Duration(seconds: 1));
      expect(await fetch(), 1);
      expect(fetches['bad.test'], 2, reason: 'tried again after it');
    });

    test('backs off on a network error too', () async {
      status['bad.test'] = -1;
      await fetch();
      await fetch();
      expect(fetches['bad.test'], 1);
    });

    test('backs off longer each time, up to the cap', () async {
      status['bad.test'] = 500;
      await fetch();
      final waits = <Duration>[];
      for (var i = 0; i < 8; i++) {
        // Steps a second at a time to find when it's tried again.
        final before = fetches['bad.test'];
        var waited = Duration.zero;
        while (fetches['bad.test'] == before) {
          now = now.add(const Duration(seconds: 1));
          waited += const Duration(seconds: 1);
          await fetch();
        }
        waits.add(waited);
      }
      expect(waits, [
        realtimeBackoffInitial,
        const Duration(minutes: 1),
        const Duration(minutes: 2),
        const Duration(minutes: 4),
        const Duration(minutes: 8),
        realtimeBackoffMax,
        realtimeBackoffMax,
        realtimeBackoffMax,
      ]);
    });

    test('is reset by a success', () async {
      status['bad.test'] = 500;
      await fetch();
      now = now.add(realtimeBackoffInitial);
      await fetch();
      expect(fetches['bad.test'], 2);

      status['bad.test'] = 200;
      now = now.add(const Duration(minutes: 1));
      expect(await fetch(), 2);
      expect(fetches['bad.test'], 3);

      // Failing again starts from the first backoff, not where it left off.
      status['bad.test'] = 500;
      await fetch();
      expect(fetches['bad.test'], 4);
      now = now.add(realtimeBackoffInitial);
      await fetch();
      expect(fetches['bad.test'], 5);
    });

    test('leaves the other feeds alone', () async {
      status['bad.test'] = 500;
      for (var i = 0; i < 5; i++) {
        expect(await fetch(), 1);
        now = now.add(const Duration(seconds: 1));
      }
      expect(fetches['bad.test'], 1);
      expect(fetches['good.test'], 5);
    });

    test('is moved a little at random', () async {
      final late = RealtimeFeeds(
        () => Future.value([source('https://bad.test/tripupdates.pb')]),
        client: MockClient((request) async {
          fetches['bad.test'] = (fetches['bad.test'] ?? 0) + 1;
          return http.Response('', 500);
        }),
        clock: () => now,
        random: _Fixed(1),
      );
      await late.prefetch();
      now = now.add(realtimeBackoffInitial);
      await late.prefetch();
      expect(fetches['bad.test'], 1, reason: 'jitter pushed it later');
      now = now.add(realtimeBackoffInitial * realtimeBackoffJitter);
      await late.prefetch();
      expect(fetches['bad.test'], 2);
    });
  });

  test('calls during the first fetch all wait for it', () async {
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      client: MockClient((request) async => http.Response.bytes([1], 200)),
    );
    final (a, b) = await (feeds.fresh(), feeds.fresh()).wait;
    expect(a.length + b.length, 1);
  });

  test('a prefetch leaves what it fetched for the next plan', () async {
    final feeds = RealtimeFeeds(
      () => Future.value([source('https://a.test/alerts.pb')]),
      client: MockClient((request) async => http.Response.bytes([1], 200)),
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
      reply.complete(http.Response.bytes([1], 200));
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

/// A random source that always gives [value], for a backoff with known jitter.
class _Fixed implements math.Random {
  _Fixed(this.value);

  final double value;

  @override
  double nextDouble() => value;

  @override
  bool nextBool() => value >= 0.5;

  @override
  int nextInt(int max) => (value * max).floor().clamp(0, max - 1);
}

/// No jitter at all.
class _Middle extends _Fixed {
  _Middle() : super(0.5);
}
