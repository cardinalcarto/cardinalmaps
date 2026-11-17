import 'package:cardinal/offline/downloads/database.dart';
import 'package:cardinal/offline/geocoder_models.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_downloads.dart';

const _title = 'Search offline';

void main() {
  group('the startup sheet', () {
    testWidgets('says what offline search needs, with sizes, and Not now puts '
        'it off for good', (tester) async {
      final store = fakeStore();
      final downloads = FakeOfflineDownloads();
      await pumpFakeApp(
        tester,
        store: store,
        downloads: Future.value(downloads),
        offerModels: true,
      );
      expect(find.text(_title), findsOneWidget);
      expect(find.textContaining('one-time download'), findsOneWidget);
      expect(find.text('Addresses and places'), findsOneWidget);
      expect(find.text('106 MB'), findsOneWidget);
      expect(find.text('Categories, like “coffee”'), findsOneWidget);
      expect(find.text('55.2 MB'), findsOneWidget);
      expect(find.text('Download · 161 MB'), findsOneWidget);
      // The device's language, as the release names the file.
      expect(downloads.plannedModels, ['eng']);

      await tester.tap(find.text('Not now'));
      await tester.pumpAndSettle();
      expect(find.text(_title), findsNothing);
      expect(downloads.startedModels, isEmpty);
      expect(await store.settings.get(geocoderModelsOffered), isTrue);
    });

    testWidgets('Download starts the download and closes', (tester) async {
      final store = fakeStore();
      final downloads = FakeOfflineDownloads();
      await pumpFakeApp(
        tester,
        store: store,
        downloads: Future.value(downloads),
        offerModels: true,
      );
      await tester.tap(find.text('Download · 161 MB'));
      await tester.pumpAndSettle();
      expect(find.text(_title), findsNothing);
      expect(downloads.startedModels, hasLength(1));
      expect(await store.settings.get(geocoderModelsOffered), isTrue);
    });

    testWidgets('dismissing it lets the map be used, and puts it off', (
      tester,
    ) async {
      final store = fakeStore();
      final downloads = FakeOfflineDownloads();
      await pumpFakeApp(
        tester,
        store: store,
        downloads: Future.value(downloads),
        offerModels: true,
      );
      // Tapping above it, on the map.
      await tester.tapAt(const Offset(200, 40));
      await tester.pumpAndSettle();
      expect(find.text(_title), findsNothing);
      expect(downloads.startedModels, isEmpty);
      expect(await store.settings.get(geocoderModelsOffered), isTrue);
    });

    testWidgets('without categories for the language, offers the tagger', (
      tester,
    ) async {
      final downloads = FakeOfflineDownloads()
        ..onPlanModels = (_) async => fakeModelsPlan(categories: false);
      await pumpFakeApp(
        tester,
        downloads: Future.value(downloads),
        offerModels: true,
      );
      expect(find.text('Categories, like “coffee”'), findsNothing);
      expect(find.text('Download · 106 MB'), findsOneWidget);
    });

    testWidgets('is shown once', (tester) async {
      final store = fakeStore();
      await store.settings.set(geocoderModelsOffered, true);
      final downloads = FakeOfflineDownloads();
      await pumpFakeApp(
        tester,
        store: store,
        downloads: Future.value(downloads),
        offerModels: true,
      );
      expect(find.text(_title), findsNothing);
      expect(downloads.plannedModels, isEmpty);
    });

    testWidgets("isn't shown once they're downloading", (tester) async {
      final downloads = FakeOfflineDownloads()
        ..emit([
          fakeRegion(
            7,
            kind: RegionKind.geocoderModels,
            state: RegionState.downloading,
          ),
        ]);
      await pumpFakeApp(
        tester,
        downloads: Future.value(downloads),
        offerModels: true,
      );
      expect(find.text(_title), findsNothing);
    });

    testWidgets("isn't shown, and comes back next time, if the release can't "
        'be read', (tester) async {
      final store = fakeStore();
      final downloads = FakeOfflineDownloads()
        ..onPlanModels = (_) async => throw Exception('offline');
      await pumpFakeApp(
        tester,
        store: store,
        downloads: Future.value(downloads),
        offerModels: true,
      );
      expect(find.text(_title), findsNothing);
      expect(await store.settings.get(geocoderModelsOffered), isFalse);
    });

    testWidgets("isn't shown in a build without downloads", (tester) async {
      await pumpFakeApp(tester, offerModels: true);
      expect(find.text(_title), findsNothing);
    });
  });

  group("the Offline tab's row", () {
    testWidgets('offers the download again', (tester) async {
      final store = fakeStore();
      await store.settings.set(geocoderModelsOffered, true);
      final downloads = FakeOfflineDownloads();
      await pumpFakeApp(
        tester,
        store: store,
        location: '/offline',
        downloads: Future.value(downloads),
      );
      expect(find.text('Offline search'), findsOneWidget);
      expect(find.text('One-time download'), findsOneWidget);

      await tester.tap(find.text('Download'));
      await tester.pumpAndSettle();
      expect(find.text(_title), findsOneWidget);
      await tester.tap(find.text('Download · 161 MB'));
      await tester.pumpAndSettle();
      expect(downloads.startedModels, hasLength(1));
    });

    testWidgets('shows progress, Retry when failed, and goes once ready', (
      tester,
    ) async {
      final downloads = FakeOfflineDownloads()
        ..emit([
          fakeRegion(
            7,
            kind: RegionKind.geocoderModels,
            state: RegionState.downloading,
            bytesDone: 80 * 1000 * 1000,
            bytesTotal: 161 * 1000 * 1000,
          ),
        ]);
      await pumpFakeApp(
        tester,
        location: '/offline',
        downloads: Future.value(downloads),
      );
      expect(find.text('Downloading · 80 MB of 161 MB'), findsOneWidget);
      // Not an area.
      expect(find.text('Download an area'), findsOneWidget);

      downloads.emit([
        fakeRegion(
          7,
          kind: RegionKind.geocoderModels,
          state: RegionState.failed,
        ),
      ]);
      await tester.pumpAndSettle();
      expect(find.text('Download failed'), findsOneWidget);
      await tester.tap(find.text('Retry'));
      await tester.pump();
      expect(downloads.retried, [7]);

      downloads.emit([fakeRegion(7, kind: RegionKind.geocoderModels)]);
      await tester.pumpAndSettle();
      expect(find.text('Offline search'), findsNothing);
    });

    testWidgets("says so when the release can't be read", (tester) async {
      final downloads = FakeOfflineDownloads()
        ..onPlanModels = (_) async => null;
      await pumpFakeApp(
        tester,
        location: '/offline',
        downloads: Future.value(downloads),
      );
      await tester.tap(find.text('Download'));
      await tester.pumpAndSettle();
      expect(find.text(_title), findsNothing);
      expect(find.text("Couldn't reach the download server"), findsOneWidget);
      // The toast fades by itself.
      await tester.pump(const Duration(seconds: 10));
      await tester.pumpAndSettle();
    });
  });
}
