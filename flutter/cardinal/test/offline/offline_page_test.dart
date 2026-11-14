import 'package:cardinal/map/map.dart';
import 'package:cardinal/offline/download_page.dart';
import 'package:cardinal/offline/downloads/database.dart';
import 'package:cardinal/offline/downloads/geometry.dart';
import 'package:cardinal/offline/drawing/area_layer.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_downloads.dart';
import '../fakes/fake_map.dart';

/// This year's March 4: dated without a year.
final _march = DateTime(DateTime.now().year, 3, 4, 9, 30);

void main() {
  testWidgets('a build without downloads says so, calmly', (tester) async {
    await pumpFakeApp(tester, location: '/offline');
    expect(
      find.text("Offline downloads aren't available in this build."),
      findsOneWidget,
    );
    expect(find.text('Download an area'), findsNothing);
  });

  testWidgets('with no areas, it says what downloading is for, and Download an '
      'area starts drawing one', (tester) async {
    final downloads = FakeOfflineDownloads();
    await pumpFakeApp(
      tester,
      location: '/offline',
      downloads: Future.value(downloads),
    );
    expect(find.textContaining('without a connection'), findsOneWidget);
    expect(find.text('Download an area'), findsOneWidget);

    await tester.tap(find.text('Download an area'));
    await tester.pumpAndSettle();
    expect(find.byType(DownloadAreaPage), findsOneWidget);
    expect(find.text('Draw around the area you want'), findsOneWidget);
    final drawing = MapScope.drawingOf(
      tester.element(find.byType(DownloadAreaPage)),
    );
    expect(drawing.frozen, isTrue);
  });

  testWidgets('areas show their names, sizes, dates and states', (
    tester,
  ) async {
    final downloads = FakeOfflineDownloads()
      ..emit([
        fakeRegion(1, name: 'Home', createdAt: _march),
        fakeRegion(
          2,
          name: 'Trip',
          state: RegionState.downloading,
          bytesDone: 68 * 1000 * 1000,
        ),
        fakeRegion(3, state: RegionState.failed, createdAt: _march),
        // Routing tiles fetched as routes need them aren't an area.
        fakeRegion(4, name: 'On demand', kind: RegionKind.onDemand),
      ]);
    await pumpFakeApp(
      tester,
      location: '/offline',
      downloads: Future.value(downloads),
    );
    expect(find.text('Home'), findsOneWidget);
    expect(find.text('136 MB · Mar 4'), findsOneWidget);
    expect(find.text('Trip'), findsOneWidget);
    expect(find.text('Downloading · 68 MB of 136 MB'), findsOneWidget);
    final bar = tester.widget<LinearProgressIndicator>(
      find.byType(LinearProgressIndicator),
    );
    expect(bar.value, closeTo(0.5, 0.01));
    // Unnamed, it goes by when it was started.
    expect(find.text('Mar 4, 9:30 AM'), findsOneWidget);
    expect(find.text('Download failed'), findsOneWidget);
    expect(find.text('On demand'), findsNothing);
    // Still reachable with areas listed.
    expect(find.text('Download an area'), findsOneWidget);

    await tester.tap(find.text('Retry'));
    await tester.pump();
    expect(downloads.retried, [3]);

    // Progress comes as it downloads.
    downloads.emit([
      fakeRegion(1, name: 'Home', createdAt: _march),
      fakeRegion(2, name: 'Trip', createdAt: _march),
    ]);
    await tester.pumpAndSettle();
    expect(find.text('136 MB · Mar 4'), findsNWidgets(2));
    expect(find.byType(LinearProgressIndicator), findsNothing);
  });

  testWidgets(
    'tapping an area shows its bounds on the map, until tapped again',
    (tester) async {
      final home = GeoPolygon(const [
        GeoPoint(47.60, -122.36),
        GeoPoint(47.60, -122.30),
        GeoPoint(47.66, -122.33),
      ]);
      final trip = GeoPolygon(const [
        GeoPoint(45.50, -122.70),
        GeoPoint(45.50, -122.60),
        GeoPoint(45.55, -122.65),
      ]);
      final downloads = FakeOfflineDownloads()
        ..emit([
          fakeRegion(1, name: 'Home', polygon: home),
          fakeRegion(2, name: 'Trip', polygon: trip),
        ]);
      final map = FakeMap();
      await pumpFakeApp(
        tester,
        location: '/offline',
        downloads: Future.value(downloads),
        map: map,
      );
      List<Object?> shown() =>
          (map.geoJson[areaSource] as Map?)?['features'] as List? ?? const [];
      expect(shown(), isEmpty);

      await tester.tap(find.text('Home'));
      await tester.pump();
      expect(shown(), hasLength(1));
      expect(map.subject, isTrue);
      expect(map.subjectBounds, (
        west: -122.36,
        south: 47.60,
        east: -122.30,
        north: 47.66,
      ));

      // Another area takes its place.
      await tester.tap(find.text('Trip'));
      await tester.pump();
      expect(map.subjectBounds!.north, 45.55);

      // Tapped again, it goes, and the camera stays where it is.
      await tester.tap(find.text('Trip'));
      await tester.pump();
      expect(shown(), isEmpty);
      expect(map.subject, isFalse);

      // Drawing a new area uses the same layer: a shown one goes first.
      await tester.tap(find.text('Home'));
      await tester.pump();
      await tester.tap(find.text('Download an area'));
      await tester.pumpAndSettle();
      expect(find.byType(DownloadAreaPage), findsOneWidget);
      expect(shown(), isEmpty);
    },
  );

  testWidgets('a shown area goes from the map when it is deleted', (
    tester,
  ) async {
    final downloads = FakeOfflineDownloads()
      ..emit([
        fakeRegion(
          1,
          name: 'Home',
          polygon: GeoPolygon(const [
            GeoPoint(47.60, -122.36),
            GeoPoint(47.60, -122.30),
            GeoPoint(47.66, -122.33),
          ]),
        ),
      ]);
    final map = FakeMap();
    await pumpFakeApp(
      tester,
      location: '/offline',
      downloads: Future.value(downloads),
      map: map,
    );
    await tester.tap(find.text('Home'));
    await tester.pump();
    expect(map.subject, isTrue);
    downloads.emit(const []);
    await tester.pumpAndSettle();
    expect((map.geoJson[areaSource] as Map)['features'], isEmpty);
    expect(map.subject, isFalse);
  });

  testWidgets('an area is deleted from its menu, once confirmed', (
    tester,
  ) async {
    final downloads = FakeOfflineDownloads()
      ..emit([fakeRegion(1, name: 'Home'), fakeRegion(2, name: 'Trip')]);
    await pumpFakeApp(
      tester,
      location: '/offline',
      downloads: Future.value(downloads),
    );
    Future<void> deleteHome(String answer) async {
      await tester.tap(find.byTooltip('More').first);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Delete'));
      await tester.pumpAndSettle();
      expect(find.text('Delete “Home”?'), findsOneWidget);
      await tester.tap(find.widgetWithText(TextButton, answer));
      await tester.pumpAndSettle();
    }

    await deleteHome('Cancel');
    expect(downloads.deleted, isEmpty);
    expect(find.text('Home'), findsOneWidget);

    await deleteHome('Delete');
    expect(downloads.deleted, [1]);
    expect(find.text('Home'), findsNothing);
    expect(find.text('Trip'), findsOneWidget);
  });

  testWidgets('on iOS an area is deleted by swiping it, once confirmed', (
    tester,
  ) async {
    final downloads = FakeOfflineDownloads()
      ..emit([fakeRegion(1, name: 'Home'), fakeRegion(2, name: 'Trip')]);
    await pumpFakeApp(
      tester,
      location: '/offline',
      downloads: Future.value(downloads),
      platform: TargetPlatform.iOS,
    );
    expect(find.byTooltip('More'), findsNothing);
    await tester.drag(find.text('Home'), const Offset(-300, 0));
    await tester.pumpAndSettle();
    expect(find.text('Delete “Home”?'), findsOneWidget);
    await tester.tap(find.text('Delete').last);
    await tester.pumpAndSettle();
    expect(downloads.deleted, [1]);
    expect(find.text('Home'), findsNothing);
  });
}
