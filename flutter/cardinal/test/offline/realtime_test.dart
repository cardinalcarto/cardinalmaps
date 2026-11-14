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
      final feeds = RealtimeFeeds(
        () => Future.value([source('https://a.test/alerts.pb')]),
        client: MockClient((request) async {
          if (!up) throw http.ClientException('no network');
          return http.Response.bytes([1], 200);
        }),
      );
      expect(await feeds.fresh(), isEmpty);
      up = true;
      // After a first fetch, a plan doesn't wait: what arrives goes to the next one.
      expect(await feeds.fresh(), isEmpty);
      await pumpEventQueue();
      expect(await feeds.fresh(), hasLength(1));
    },
  );

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
