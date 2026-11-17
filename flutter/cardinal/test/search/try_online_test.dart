import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_geocoder.dart';

const _needle = Place(
  id: 'poi:1',
  kind: 'poi',
  name: 'Space Needle',
  center: LngLat(-122.3493, 47.6205),
);

const _ask = 'Nothing offline. Try online?';

Future<void> _type(WidgetTester tester, String text) async {
  await tester.enterText(find.byType(TextField), text);
  await tester.pump(const Duration(milliseconds: 300));
  await tester.pumpAndSettle();
}

/// The app searching with nothing found offline and [_needle] online.
Future<(Store, FakeGeocoder, FakeGeocoder)> _pump(
  WidgetTester tester, {
  List<Place> offlineResults = const [],
  OnlineFallback? fallback,
  bool goOnline = false,
}) async {
  final store = fakeStore();
  if (fallback != null) await store.settings.set(onlineFallback, fallback);
  await store.settings.set(searchOnline, goOnline);
  final offline = FakeGeocoder(offlineResults);
  final online = FakeGeocoder([_needle]);
  await pumpFakeApp(tester, store: store, offline: offline, online: online);
  await tester.tap(find.byType(SearchBar));
  await tester.pumpAndSettle();
  return (store, offline, online);
}

void main() {
  testWidgets('asks when the device finds nothing', (tester) async {
    final (_, offline, online) = await _pump(tester);
    await _type(tester, 'space needle');
    expect(offline.asked, ['space needle']);
    expect(online.asked, isEmpty);
    expect(find.text(_ask), findsOneWidget);
    expect(find.text('Search online'), findsOneWidget);
    expect(find.text('Always'), findsOneWidget);
    expect(find.text('Never'), findsOneWidget);
    expect(find.text('No results'), findsNothing);
  });

  testWidgets("doesn't ask when the device finds something", (tester) async {
    await _pump(tester, offlineResults: [_needle]);
    await _type(tester, 'space needle');
    expect(find.text(_ask), findsNothing);
    expect(find.text('Space Needle'), findsOneWidget);
  });

  testWidgets("doesn't ask when search goes online", (tester) async {
    final (_, offline, _) = await _pump(tester, goOnline: true);
    await _type(tester, 'space needle');
    expect(offline.asked, isEmpty);
    expect(find.text(_ask), findsNothing);
    expect(find.text('Space Needle'), findsOneWidget);
  });

  testWidgets('Search online searches online once, choosing nothing', (
    tester,
  ) async {
    final (store, _, online) = await _pump(tester);
    await _type(tester, 'space needle');
    await tester.tap(find.text('Search online'));
    await tester.pumpAndSettle();
    expect(online.asked, ['space needle']);
    expect(find.text('Space Needle'), findsOneWidget);
    expect(find.text(_ask), findsNothing);
    expect(await store.settings.get(onlineFallback), isNull);
    expect(await store.settings.get(searchOnline), isFalse);

    // The next text goes to the device again, and asks again.
    await _type(tester, 'space needles');
    expect(online.asked, ['space needle']);
    expect(find.text(_ask), findsOneWidget);
  });

  testWidgets('Always searches online now and whenever the device finds '
      'nothing, without switching search online', (tester) async {
    final (store, _, online) = await _pump(tester);
    await _type(tester, 'space needle');
    await tester.tap(find.text('Always'));
    await tester.pumpAndSettle();
    expect(await store.settings.get(onlineFallback), OnlineFallback.always);
    expect(await store.settings.get(searchOnline), isFalse);
    expect(online.asked, ['space needle']);
    expect(find.text('Space Needle'), findsOneWidget);
    expect(find.text(_ask), findsNothing);

    await _type(tester, 'space');
    expect(online.asked, ['space needle', 'space']);
    expect(find.text(_ask), findsNothing);
  });

  testWidgets('with Always chosen and nothing online either, no results', (
    tester,
  ) async {
    final (_, _, online) = await _pump(tester, fallback: OnlineFallback.always);
    online.results = [];
    await _type(tester, 'zzz');
    expect(online.asked, ['zzz']);
    expect(find.text('No results'), findsOneWidget);
    expect(find.text(_ask), findsNothing);
  });

  testWidgets('Never stops asking', (tester) async {
    final (store, _, online) = await _pump(tester);
    await _type(tester, 'space needle');
    await tester.tap(find.text('Never'));
    await tester.pumpAndSettle();
    expect(await store.settings.get(onlineFallback), OnlineFallback.never);
    expect(find.text(_ask), findsNothing);
    expect(find.text('No results'), findsOneWidget);

    await _type(tester, 'space');
    expect(online.asked, isEmpty);
    expect(find.text(_ask), findsNothing);
    expect(find.text('No results'), findsOneWidget);
  });
}
