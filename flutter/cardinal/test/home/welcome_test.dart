import 'package:cardinal/home/welcome.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart' show LocationPermission;
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_downloads.dart';
import '../fakes/fake_location.dart';

const _title = 'Welcome to Cardinal Maps';

void main() {
  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    testWidgets('requires agreement on ${platform.name}', (tester) async {
      final store = fakeStore();
      await pumpFakeApp(
        tester,
        store: store,
        offerWelcome: true,
        platform: platform,
      );
      expect(find.text(_title), findsOneWidget);
      expect(find.textContaining('bottom-right'), findsOneWidget);
      expect(find.textContaining('own judgment'), findsNothing);
      expect(await store.settings.get(welcomeAccepted), isFalse);

      await tester.tapAt(const Offset(10, 10));
      await tester.pumpAndSettle();
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.text(_title), findsOneWidget);
      expect(await store.settings.get(welcomeAccepted), isFalse);

      await tester.tap(find.text('Next'));
      await tester.pumpAndSettle();
      expect(find.text('Use maps safely'), findsOneWidget);
      expect(find.textContaining('own judgment'), findsOneWidget);
      expect(await store.settings.get(welcomeAccepted), isFalse);
      await tester.tapAt(const Offset(10, 10));
      await tester.binding.handlePopRoute();
      await tester.pumpAndSettle();
      expect(find.text('Use maps safely'), findsOneWidget);
      await tester.tap(find.text('Back'));
      await tester.pumpAndSettle();
      expect(find.text(_title), findsOneWidget);
      await tester.tap(find.text('Next'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('I agree'));
      await tester.pumpAndSettle();
      expect(find.text(_title), findsNothing);
      expect(await store.settings.get(welcomeAccepted), isTrue);
      await tester.tap(find.text('Offline'));
      await tester.pumpAndSettle();
      expect(find.text('Offline maps'), findsOneWidget);
    });
  }

  testWidgets('does not reappear after acceptance', (tester) async {
    final store = fakeStore();
    await store.settings.set(welcomeAccepted, true);
    await pumpFakeApp(tester, store: store, offerWelcome: true);
    expect(find.text(_title), findsNothing);
  });

  testWidgets('precedes location and offline search offers', (tester) async {
    await pumpFakeApp(
      tester,
      offerWelcome: true,
      offerModels: true,
      locationButton: true,
      locationSource: FakeLocation(permission: LocationPermission.denied),
      downloads: Future.value(FakeOfflineDownloads()),
    );
    expect(find.text(_title), findsOneWidget);
    expect(find.text('Enable location?'), findsNothing);
    expect(find.text('Search offline'), findsNothing);
    await tester.tap(find.text('Next'));
    await tester.pumpAndSettle();
    expect(find.text('Enable location?'), findsNothing);
    await tester.tap(find.text('I agree'));
    await tester.pumpAndSettle();
    expect(find.text('Enable location?'), findsOneWidget);
    expect(find.text('Search offline'), findsNothing);
    await tester.tap(find.text('No'));
    await tester.pumpAndSettle();
    expect(find.text('Search offline'), findsOneWidget);
  });
}
