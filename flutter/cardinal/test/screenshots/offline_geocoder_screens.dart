// Screenshots of the offline geocoder's screens, for review: see `render.dart`.
import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/search/search_sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_downloads.dart';
import '../fakes/fake_geocoder.dart';
import 'render.dart';

const _needle = Place(
  id: 'poi:1',
  kind: 'poi',
  name: 'Space Needle',
  center: LngLat(-122.3493, 47.6205),
);

/// Types [text] into the search bar and waits for the results.
Future<void> _search(WidgetTester tester, String text) async {
  await tester.tap(find.byType(SearchBar));
  await tester.pumpAndSettle();
  await tester.enterText(find.byType(TextField), text);
  await tester.pump(const Duration(milliseconds: 300));
  await tester.pumpAndSettle();
}

void main() {
  setUpAll(loadRenderFonts);

  testWidgets('1 search, offline results', (tester) async {
    await pumpFakeApp(
      tester,
      platform: TargetPlatform.android,
      offline: FakeGeocoder([_needle]),
    );
    await tester.pump();
    expect(find.byType(SearchSheet), findsOneWidget);
    await _search(tester, 'space needle');
    await screenshot(tester, '1-search-offline-results');
  });

  testWidgets('2 offline tab, search toggle', (tester) async {
    final downloads = FakeOfflineDownloads()
      ..emit([
        fakeRegion(1, name: 'Seattle', createdAt: DateTime(2026, 11, 3)),
      ]);
    await pumpFakeApp(
      tester,
      platform: TargetPlatform.android,
      location: '/offline',
      downloads: Future.value(downloads),
    );
    await screenshot(tester, '2a-offline-tab-search-offline');

    await tester.tap(find.text('Online'));
    await screenshot(tester, '2b-offline-tab-search-online');
  });

  testWidgets('2 offline tab, a fallback chosen', (tester) async {
    final store = fakeStore();
    await store.settings.set(onlineFallback, OnlineFallback.always);
    await pumpFakeApp(
      tester,
      store: store,
      platform: TargetPlatform.android,
      location: '/offline',
      downloads: Future.value(FakeOfflineDownloads()),
    );
    await screenshot(tester, '2c-offline-tab-fallback-always');
  });
}
