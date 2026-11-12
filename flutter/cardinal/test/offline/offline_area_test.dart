import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:cardinal/offline/offline_area.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

void main() {
  group('after downloads remove files', () {
    late List<_FakeRouter> opened;
    late OfflineArea area;
    setUp(() {
      opened = [];
      area = OfflineArea(
        routing: '/nonexistent/routing/r',
        transitBuild: '/nonexistent/transit/t',
        openRouter: () async => (opened..add(_FakeRouter())).last,
      );
    });
    Future<RouterResponse> route() =>
        area.useRouter((r) => r.route(body: const []));
    final ok = RouterResponse(status: 200, body: Uint8List(0));

    test('new files are found by refreshing the open router', () async {
      await area.router;
      await area.offlineChanged(feedsChanged: false);
      expect(opened.single.refreshed, 1);
      expect(opened.single.isDisposed, isFalse);
    });

    test(
      'the router is dropped once its running routes finish, and reopened',
      () async {
        final running = route();
        await pumpEventQueue();
        final old = opened.single;
        await area.offlineChanged(feedsChanged: false, filesRemoved: true);
        // The route still running keeps it.
        expect(old.isDisposed, isFalse);
        expect(old.refreshed, 0);
        // A new route gets a router opened afresh.
        final next = route();
        await pumpEventQueue();
        expect(opened, hasLength(2));
        expect(opened.last.routes, hasLength(1));
        old.routes.single.complete(ok);
        expect((await running).status, 200);
        await pumpEventQueue();
        expect(old.isDisposed, isTrue);
        opened.last.routes.single.complete(ok);
        await next;
        expect(opened.last.isDisposed, isFalse);
      },
    );

    test('an idle router is dropped at once', () async {
      await area.router;
      await area.offlineChanged(feedsChanged: false, filesRemoved: true);
      expect(opened.single.isDisposed, isTrue);
      await area.router;
      expect(opened, hasLength(2));
    });
  });

  test(
    "an area covers the quarter degrees whose level 2 tiles it has",
    () async {
      final root = await Directory.systemTemp.createTemp('offline_area_test');
      addTearDown(() => root.delete(recursive: true));
      // Seattle's square, 47.6, -122.33: row 550, column 230.
      await File('${root.path}/routing/r/tiles/2/000/792/230.gph')
          .create(recursive: true);
      final area = OfflineArea(
        routing: '${root.path}/routing/r',
        transitBuild: '${root.path}/transit/t',
      );
      expect(await area.covers(47.6062, -122.3321), isTrue);
      expect(await area.covers(47.6062, -122.0), isFalse);
      expect(await area.covers(45.5, -122.6), isFalse);
    },
  );
}

/// A router whose routes finish when told to.
class _FakeRouter implements OfflineRouter {
  final routes = <Completer<RouterResponse>>[];
  var refreshed = 0;

  @override
  bool isDisposed = false;

  @override
  void dispose() => isDisposed = true;

  @override
  Future<void> refreshTiles() async => refreshed++;

  @override
  Future<RouterResponse> route({required List<int> body}) {
    expect(isDisposed, isFalse, reason: 'routed on a disposed router');
    final done = Completer<RouterResponse>();
    routes.add(done);
    return done.future;
  }
}
