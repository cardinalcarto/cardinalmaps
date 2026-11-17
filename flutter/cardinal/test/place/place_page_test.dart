import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_geocoder.dart';

const _street = Place(
  id: 'local:7083948262915487',
  kind: 'street',
  name: 'Aurora Avenue North',
  center: LngLat(-122.3439, 47.623),
);

void main() {
  testWidgets(
    "a place the store doesn't have comes from the device for its own IDs",
    (tester) async {
      final store = fakeStore();
      // Even with search set to go online: only the device knows `local:` IDs.
      await store.settings.set(searchOnline, true);
      final online = FakeGeocoder();
      final offline = FakeGeocoder([_street]);
      await pumpFakeApp(
        tester,
        store: store,
        online: online,
        offline: offline,
        location: placeLocation(_street.id),
      );
      expect(find.text('Aurora Avenue North'), findsOneWidget);
    },
  );

  testWidgets('a saved place with a local ID shows what was stored of it', (
    tester,
  ) async {
    final store = fakeStore();
    await store.places.put(_street);
    await store.saved.savePlace(_street.id);
    // The index was rebuilt without it: the device finds nothing by its ID.
    await pumpFakeApp(
      tester,
      store: store,
      offline: FakeGeocoder(),
      location: placeLocation(_street.id),
    );
    expect(find.text('Aurora Avenue North'), findsOneWidget);
    expect(find.text('Place not found'), findsNothing);
  });

  testWidgets('a local ID the device no longer has is not found', (
    tester,
  ) async {
    await pumpFakeApp(
      tester,
      offline: FakeGeocoder(),
      location: placeLocation(_street.id),
    );
    expect(find.text('Place not found'), findsOneWidget);
  });
}
