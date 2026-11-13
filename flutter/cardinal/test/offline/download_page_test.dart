import 'dart:async';
import 'dart:math' as math;

import 'package:cardinal/map/map_drawing.dart';
import 'package:cardinal/offline/download_page.dart';
import 'package:cardinal/offline/downloads/bucket.dart';
import 'package:cardinal/offline/downloads/plan.dart';
import 'package:cardinal/offline/drawing/area_layer.dart';
import 'package:cardinal/offline/offline_page.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_downloads.dart';
import '../fakes/fake_map.dart';

const _gb = 1000 * 1000 * 1000;

/// Draws a loop of [radius] round [center] with one finger, over the map.
Future<void> _lasso(
  WidgetTester tester, {
  Offset center = const Offset(200, 300),
  double radius = 120,
}) async {
  final finger = await tester.startGesture(center + Offset(radius, 0));
  for (var a = 0.1; a <= 2 * math.pi + 0.3; a += 0.1) {
    await finger.moveTo(center + Offset(math.cos(a), math.sin(a)) * radius);
  }
  await finger.up();
  await tester.pump();
}

/// The features of the drawn area on [map].
List<Object?> _area(FakeMap map) =>
    (map.geoJson[areaSource] as Map?)?['features'] as List? ?? const [];

Finder _button(String label) => find.widgetWithText(FilledButton, label);

bool _enabled(WidgetTester tester, String label) =>
    tester.widget<FilledButton>(_button(label)).onPressed != null;

void main() {
  late FakeOfflineDownloads downloads;
  late FakeMap map;
  late MapDrawing drawing;
  late GoRouter router;

  /// The Offline tab, gone on to drawing an area.
  Future<void> startDrawing(WidgetTester tester, {int free = 64 * _gb}) async {
    downloads = FakeOfflineDownloads();
    map = FakeMap()..hasGround = true;
    drawing = MapDrawing();
    router = await pumpFakeApp(
      tester,
      location: '/offline',
      map: map,
      drawing: drawing,
      downloads: Future.value(downloads),
      freeBytes: () async => free,
    );
    await tester.tap(find.text('Download an area'));
    await tester.pumpAndSettle();
  }

  testWidgets('drawing holds the map still, and a loop is measured, then '
      'page 1 says what it needs', (tester) async {
    await startDrawing(tester);
    expect(drawing.frozen, isTrue);
    expect(find.text('Draw around the area you want'), findsOneWidget);
    expect(find.text('Cancel'), findsOneWidget);
    final plan = Completer<OfflinePlan>();
    downloads.onPlan = (_) => plan.future;

    await _lasso(tester);
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Measuring this area…'), findsOneWidget);
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(_area(map), hasLength(1));
    // Around the loop drawn: FakeMap's ground is hundredths of a degree per pixel.
    final polygon = downloads.planned.single;
    expect(polygon.bounds.west, closeTo(0.8, 0.05));
    expect(polygon.bounds.east, closeTo(3.2, 0.05));
    expect(polygon.points.length, lessThanOrEqualTo(48));

    plan.complete(fakeAreaPlan(polygon, feeds: ['kcm']));
    await tester.pumpAndSettle();
    expect(find.text('Download this area?'), findsOneWidget);
    expect(
      find.text(
        'Maps and directions for this area need about 136 MB. You have '
        '64 GB free.',
        findRichText: true,
      ),
      findsOneWidget,
    );
    final name = tester.widget<TextField>(find.byType(TextField));
    expect(
      name.controller!.text,
      matches(RegExp(r'^[A-Z][a-z]{2} \d{1,2}, \d{1,2}:\d\d [AP]M$')),
    );
    expect(find.text("You'll choose transit next."), findsOneWidget);
    expect(_enabled(tester, 'Next'), isTrue);
    expect(find.text('Redraw'), findsOneWidget);
    expect(drawing.frozen, isTrue);
  });

  testWidgets('page 1 keeps the area in view above it, and Redraw lets it go', (
    tester,
  ) async {
    await startDrawing(tester);
    final plan = Completer<OfflinePlan>();
    downloads.onPlan = (_) => plan.future;
    await _lasso(tester);
    await tester.pump(const Duration(milliseconds: 400));
    // Measuring: the map stays where it was drawn.
    expect(map.subjectsSet, 0);

    final polygon = downloads.planned.single;
    plan.complete(fakeAreaPlan(polygon));
    await tester.pump();
    expect(map.subject, isTrue);
    expect(map.subjectsSet, 1);
    final bounds = map.subjectBounds!;
    expect(bounds.west, polygon.bounds.west);
    expect(bounds.south, polygon.bounds.south);
    expect(bounds.east, polygon.bounds.east);
    expect(bounds.north, polygon.bounds.north);
    await tester.pumpAndSettle();
    expect(find.text('Download this area?'), findsOneWidget);

    await tester.tap(find.text('Redraw'));
    await tester.pumpAndSettle();
    expect(map.subject, isFalse);
  });

  testWidgets('a second finger landing mid-stroke throws the stroke away, and '
      'two fingers move the map', (tester) async {
    await startDrawing(tester);
    expect(map.gestures, MapGestures.pinch);
    const center = Offset(200, 300);
    final one = await tester.startGesture(center + const Offset(120, 0));
    for (var a = 0.1; a <= 1.5 * math.pi; a += 0.1) {
      await one.moveTo(center + Offset(math.cos(a), math.sin(a)) * 120);
    }
    final two = await tester.startGesture(const Offset(300, 200));
    await two.moveTo(const Offset(330, 180));
    for (var a = 1.5 * math.pi; a <= 2 * math.pi + 0.3; a += 0.1) {
      await one.moveTo(center + Offset(math.cos(a), math.sin(a)) * 120);
    }
    await two.up();
    await one.up();
    await tester.pumpAndSettle();
    expect(downloads.planned, isEmpty);
    expect(_area(map), isEmpty);
    expect(find.text('Draw around the area you want'), findsOneWidget);

    // Then one finger draws as before, and the map is held still on page 1.
    await _lasso(tester);
    await tester.pumpAndSettle();
    expect(downloads.planned, hasLength(1));
    expect(find.text('Download this area?'), findsOneWidget);
    expect(map.gestures, MapGestures.all);
    expect(drawing.frozen, isTrue);
  });

  testWidgets('a lasso too small to download asks to draw again', (
    tester,
  ) async {
    await startDrawing(tester);
    await _lasso(tester, radius: 15);
    await tester.pump(const Duration(milliseconds: 300));
    expect(
      find.text("That's too small to download. Draw around a larger area."),
      findsOneWidget,
    );
    expect(downloads.planned, isEmpty);
    expect(_area(map), isEmpty);

    await _lasso(tester);
    await tester.pumpAndSettle();
    expect(downloads.planned, hasLength(1));
    expect(find.text('Download this area?'), findsOneWidget);
  });

  testWidgets('a lasso off the ground asks to keep to the map', (tester) async {
    await startDrawing(tester);
    map.hasGround = false;
    await _lasso(tester);
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Keep the outline on the map.'), findsOneWidget);
    expect(downloads.planned, isEmpty);
  });

  testWidgets('a plan that fails says why, with Try again and Redraw', (
    tester,
  ) async {
    await startDrawing(tester);
    var fail = true;
    downloads.onPlan = (polygon) async {
      if (fail) throw const BucketError('latest.json', null, 'timed out');
      return fakeAreaPlan(polygon);
    };
    await _lasso(tester);
    await tester.pumpAndSettle();
    expect(find.text("Couldn't measure this area"), findsOneWidget);
    expect(find.textContaining("can't be reached"), findsOneWidget);

    fail = false;
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(downloads.planned, hasLength(2));
    expect(find.text('Download this area?'), findsOneWidget);

    // And failing again, Redraw goes back to drawing, without the area.
    fail = true;
    await tester.tap(find.text('Redraw'));
    await tester.pumpAndSettle();
    await _lasso(tester);
    await tester.pumpAndSettle();
    expect(find.text("Couldn't measure this area"), findsOneWidget);
    await tester.tap(find.text('Redraw'));
    await tester.pumpAndSettle();
    expect(find.text('Draw around the area you want'), findsOneWidget);
    expect(_area(map), isEmpty);
    expect(drawing.frozen, isTrue);
  });

  testWidgets('without the space for it, page 1 says so and Download waits', (
    tester,
  ) async {
    // 136 MB needs 100 MB to spare.
    await startDrawing(tester, free: 200 * 1000 * 1000);
    await _lasso(tester);
    await tester.pumpAndSettle();
    expect(
      find.text(
        'Not enough space: this area needs 136 MB, and 200 MB is '
        'free.',
        findRichText: true,
      ),
      findsOneWidget,
    );
    expect(_enabled(tester, 'Download'), isFalse);
  });

  testWidgets('with room to spare, page 1 lets it download', (tester) async {
    await startDrawing(tester, free: 237 * 1000 * 1000);
    await _lasso(tester);
    await tester.pumpAndSettle();
    expect(
      find.textContaining('Not enough space', findRichText: true),
      findsNothing,
    );
    expect(_enabled(tester, 'Download'), isTrue);
  });

  testWidgets('with no transit to choose, Download starts the area with its '
      'name, and the Offline tab shows it', (tester) async {
    await startDrawing(tester);
    await _lasso(tester);
    await tester.pumpAndSettle();
    expect(find.text("You'll choose transit next."), findsNothing);
    expect(find.text('Next'), findsNothing);

    await tester.enterText(find.byType(TextField), 'Seattle');
    await tester.tap(_button('Download'));
    await tester.pumpAndSettle();
    final started = downloads.started.single;
    expect(started.name, 'Seattle');
    expect(started.feeds, isEmpty);
    expect(started.plan.polygon, downloads.planned.single);
    expect(find.byType(OfflinePage), findsOneWidget);
    expect(find.byType(DownloadAreaPage), findsNothing);
    expect(router.state.uri.path, '/offline');
    expect(drawing.frozen, isFalse);
    expect(_area(map), isEmpty);
  });

  testWidgets('a blank name is the date and time again', (tester) async {
    await startDrawing(tester);
    await _lasso(tester);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), '  ');
    await tester.tap(_button('Download'));
    await tester.pumpAndSettle();
    expect(downloads.started.single.name, matches(RegExp(r'\d:\d\d [AP]M$')));
  });

  testWidgets('Next goes to page 2, whose Back returns to page 1 as it was', (
    tester,
  ) async {
    await startDrawing(tester);
    downloads.onPlan = (p) async => fakeAreaPlan(p, feeds: ['kcm']);
    await _lasso(tester);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Trip');

    await tester.tap(_button('Next'));
    await tester.pump(const Duration(milliseconds: 150));
    // Partway, both pages show, sliding.
    expect(find.text('Download this area?'), findsOneWidget);
    expect(find.text('Transit'), findsOneWidget);
    await tester.pumpAndSettle();
    expect(find.text('Download this area?'), findsNothing);
    expect(find.text('Transit'), findsOneWidget);
    // Redraw is only on page 1.
    expect(find.text('Redraw'), findsNothing);

    await tester.tap(find.text('Back'));
    await tester.pumpAndSettle();
    expect(find.text('Download this area?'), findsOneWidget);
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller!.text,
      'Trip',
    );

    // The system's back does the same from page 2, then Redraws from page 1.
    await tester.tap(_button('Next'));
    await tester.pumpAndSettle();
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(find.text('Download this area?'), findsOneWidget);
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(find.text('Draw around the area you want'), findsOneWidget);
    expect(downloads.started, isEmpty);
  });

  testWidgets('Redraw on page 1 draws again, the map still held', (
    tester,
  ) async {
    await startDrawing(tester);
    await _lasso(tester);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Redraw'));
    await tester.pumpAndSettle();
    expect(find.text('Draw around the area you want'), findsOneWidget);
    expect(_area(map), isEmpty);
    expect(drawing.frozen, isTrue);
    await _lasso(tester);
    await tester.pumpAndSettle();
    expect(downloads.planned, hasLength(2));
  });

  testWidgets('Cancel goes back to the Offline tab and lets the map go', (
    tester,
  ) async {
    await startDrawing(tester);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(find.byType(OfflinePage), findsOneWidget);
    expect(drawing.frozen, isFalse);
  });
}
