// Screenshots of the offline geocoder's screens, for review: see `render.dart`.
import 'package:cardinal/search/search_sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
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
}
