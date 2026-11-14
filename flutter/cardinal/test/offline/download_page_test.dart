import 'dart:async';
import 'dart:math' as math;

import 'package:cardinal/map/map_drawing.dart';
import 'package:cardinal/offline/download_page.dart';
import 'package:cardinal/offline/download_steps.dart';
import 'package:cardinal/offline/downloads/bucket.dart';
import 'package:cardinal/offline/downloads/geometry.dart';
import 'package:cardinal/offline/downloads/plan.dart';
import 'package:cardinal/offline/drawing/area_layer.dart';
import 'package:cardinal/offline/offline_page.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoIcons;
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_downloads.dart';
import '../fakes/fake_map.dart';

const _gb = 1000 * 1000 * 1000;
const _mb = 1000 * 1000;

/// [n] megabytes as the page writes them, with its no-break space.
String _mbText(String n) => '$n\u00a0MB';

/// Sixteen agencies, as Amtrak's feed has.
const _amtrakAgencies = [
  'Amtrak',
  'AVLN',
  'Arrow Trailways',
  'Bustang',
  'Cascades',
  'Greyhound',
  'Hoosier Ride',
  'Indian Trails',
  'Jefferson Lines',
  'Lamers',
  'Miller Transportation',
  'Northwestern',
  'Peter Pan',
  'San Joaquins',
  'Thruway',
  'Trailways',
];

/// Seattle's feeds, out of order: by index, Amtrak, Sound Transit, Metro, Kitsap.
/// Metro and Sound Transit share a transfer tile.
List<CandidateFeed> _seattle() => [
  fakeFeed(
    'us-Amtrak',
    index: 0,
    displayName: 'Amtrak',
    agencies: _amtrakAgencies,
    bytes: 2700 * 1000,
    cellsInside: 3,
  ),
  fakeFeed(
    'us-wa-Sound-Transit',
    index: 1,
    displayName: 'Sound-Transit',
    agencies: ['Sound Transit'],
    bytes: 3800 * 1000,
    transfers: {
      'walking': [(1, 1, 10 * _mb), (1, 2, 5 * _mb)],
    },
    cellsInside: 12,
  ),
  fakeFeed(
    'us-wa-Metro-Transit',
    index: 2,
    displayName: 'Metro-Transit',
    agencies: ['Metro', 'Link'],
    bytes: 15100 * 1000,
    transfers: {
      'walking': [(1, 1, 10 * _mb)],
    },
    cellsInside: 40,
  ),
  fakeFeed(
    'us-wa-Kitsap-Transit',
    index: 3,
    displayName: 'Kitsap-Transit',
    agencies: ['Kitsap Transit'],
    bytes: 1200 * 1000,
    cellsInside: 3,
  ),
];

/// The plan of the area with [_seattle]'s feeds, whose shared files are 2 MB.
OfflinePlan _seattlePlan(GeoPolygon polygon, {List<CandidateFeed>? feeds}) =>
    fakeAreaPlan(
      polygon,
      candidates: feeds ?? _seattle(),
      transitSharedBytes: 2 * _mb,
    );

/// The total line's text.
String _totalLine(WidgetTester tester) => tester
    .widget<RichText>(
      find.textContaining(RegExp('^(Total|Not enough)'), findRichText: true),
    )
    .text
    .toPlainText();

/// The titles of page 2's rows, top to bottom.
List<String> _rowTitles(WidgetTester tester) {
  final tiles = find.byType(CheckboxListTile);
  return [
    for (final e in tiles.evaluate())
      (((e.widget as CheckboxListTile).title!) as Text).data!,
  ];
}

bool _checked(WidgetTester tester, String title) => tester
    .widget<CheckboxListTile>(find.widgetWithText(CheckboxListTile, title))
    .value!;

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
  Future<void> startDrawing(
    WidgetTester tester, {
    int free = 64 * _gb,
    TargetPlatform? platform,
  }) async {
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
      platform: platform,
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

  /// Page 1 of [_seattlePlan], or of [feeds], gone on to page 2.
  Future<void> openTransit(
    WidgetTester tester, {
    int free = 64 * _gb,
    List<CandidateFeed>? feeds,
    TargetPlatform? platform,
  }) async {
    await startDrawing(tester, free: free, platform: platform);
    downloads.onPlan = (p) async => _seattlePlan(p, feeds: feeds);
    await _lasso(tester);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Next'));
    await tester.pumpAndSettle();
    expect(find.text('Transit'), findsOneWidget);
  }

  testWidgets('page 2 lists the feeds most in the area first, each with its '
      'title, agencies and size, none chosen', (tester) async {
    await openTransit(tester);
    expect(find.text('Choose the agencies you ride.'), findsOneWidget);
    // Most stop cells first; Amtrak and Kitsap tie, by index.
    expect(_rowTitles(tester), [
      'Metro-Transit',
      'Sound Transit',
      'Amtrak',
      'Kitsap Transit',
    ]);
    // One agency: its name, not the feed's, and no agencies line.
    expect(find.text('Sound-Transit'), findsNothing);
    expect(find.text('Kitsap-Transit'), findsNothing);
    // Several: the feed's name as published, and its agencies.
    expect(find.text('Metro, Link'), findsOneWidget);
    final amtrak = tester
        .widgetList<Text>(
          find.descendant(
            of: find.byType(AgencyLine),
            matching: find.byType(Text),
          ),
        )
        .map((t) => t.data!)
        .firstWhere((t) => t.startsWith('Amtrak'));
    expect(amtrak, matches(RegExp(r'^Amtrak, AVLN(, [^,]+)* \+\d+$')));
    expect(find.text('Kitsap Transit, '), findsNothing);
    // Sizes: each feed's own files.
    expect(find.text(_mbText('15.1')), findsOneWidget);
    expect(find.text(_mbText('3.8')), findsOneWidget);
    expect(find.text(_mbText('2.7')), findsOneWidget);
    expect(find.text(_mbText('1.2')), findsOneWidget);
    // Nothing chosen: maps and directions only.
    for (final title in _rowTitles(tester)) {
      expect(_checked(tester, title), isFalse);
    }
    expect(_totalLine(tester), 'Total ${_mbText('136')} · 64\u00a0GB free');
    expect(_enabled(tester, 'Download'), isTrue);
    expect(find.text('Redraw'), findsNothing);
  });

  testWidgets('choosing feeds adds them to the total as it runs to the new '
      'size, their shared files and transfer tiles counted once', (
    tester,
  ) async {
    await openTransit(tester);
    // Anywhere on the row toggles it.
    await tester.tap(find.text(_mbText('15.1')));
    await tester.pump();
    expect(_checked(tester, 'Metro-Transit'), isTrue);
    await tester.pump(const Duration(milliseconds: 60));
    final between = _totalLine(tester);
    expect(between, isNot(contains(_mbText('136'))));
    expect(between, isNot(contains(_mbText('163'))));
    await tester.pumpAndSettle();
    // 136 MB, Metro's 15.1 MB, the shared 2 MB and its 10 MB transfer tile.
    expect(_totalLine(tester), 'Total ${_mbText('163')} · 64\u00a0GB free');

    // Sound Transit shares that tile: only its own 3.8 MB and its other 5 MB.
    await tester.tap(find.text('Sound Transit'));
    await tester.pumpAndSettle();
    expect(_totalLine(tester), 'Total ${_mbText('172')} · 64\u00a0GB free');

    await tester.tap(find.text('Metro-Transit'));
    await tester.pumpAndSettle();
    expect(_checked(tester, 'Metro-Transit'), isFalse);
    // 136 MB, 3.8 MB, 2 MB, and both its tiles.
    expect(_totalLine(tester), 'Total ${_mbText('157')} · 64\u00a0GB free');
  });

  testWidgets('without the space for the feeds chosen, page 2 says so as page '
      '1 does, and Download waits', (tester) async {
    // 136 MB fits in 250 MB with 100 MB to spare; 163 MB doesn't.
    await openTransit(tester, free: 250 * _mb);
    expect(
      _totalLine(tester),
      'Total ${_mbText('136')} · ${_mbText('250')} free',
    );
    await tester.tap(find.text('Metro-Transit'));
    await tester.pumpAndSettle();
    expect(
      _totalLine(tester),
      'Not enough space: this area needs ${_mbText('163')}, and '
      '${_mbText('250')} is free.',
    );
    expect(_enabled(tester, 'Download'), isFalse);
    expect(_enabled(tester, 'Back'), isTrue);

    await tester.tap(find.text('Metro-Transit'));
    await tester.pumpAndSettle();
    expect(_enabled(tester, 'Download'), isTrue);
  });

  testWidgets('Download with no feeds chosen downloads maps and directions, '
      'and goes back to the Offline tab', (tester) async {
    await openTransit(tester);
    await tester.tap(_button('Download'));
    await tester.pumpAndSettle();
    final started = downloads.started.single;
    expect(started.feeds, isEmpty);
    expect(started.name, matches(RegExp(r'\d:\d\d [AP]M$')));
    expect(started.plan.polygon, downloads.planned.single);
    expect(find.byType(OfflinePage), findsOneWidget);
    expect(find.byType(DownloadAreaPage), findsNothing);
    expect(router.state.uri.path, '/offline');
    expect(drawing.frozen, isFalse);
  });

  testWidgets('Download with feeds chosen downloads them, with the name from '
      'page 1', (tester) async {
    await startDrawing(tester);
    downloads.onPlan = (p) async => _seattlePlan(p);
    await _lasso(tester);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Seattle');
    await tester.tap(_button('Next'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Metro-Transit'));
    await tester.tap(find.text('Amtrak'));
    await tester.pumpAndSettle();
    await tester.tap(_button('Download'));
    await tester.pumpAndSettle();
    final started = downloads.started.single;
    expect(started.feeds, {'us-wa-Metro-Transit', 'us-Amtrak'});
    expect(started.name, 'Seattle');
    expect(find.byType(OfflinePage), findsOneWidget);
  });

  testWidgets('Back keeps page 1 as it was, and the feeds chosen for when it '
      'comes back', (tester) async {
    await startDrawing(tester, free: 250 * _mb);
    downloads.onPlan = (p) async => _seattlePlan(p);
    await _lasso(tester);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'Trip');
    await tester.tap(_button('Next'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Kitsap Transit'));
    await tester.pumpAndSettle();

    await tester.tap(_button('Back'));
    await tester.pumpAndSettle();
    expect(find.text('Download this area?'), findsOneWidget);
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller!.text,
      'Trip',
    );
    // Page 1 is about maps and directions alone, whatever page 2 had chosen.
    expect(
      find.textContaining('need about ${_mbText('136')}', findRichText: true),
      findsOneWidget,
    );
    expect(_enabled(tester, 'Next'), isTrue);

    await tester.tap(_button('Next'));
    await tester.pumpAndSettle();
    expect(_checked(tester, 'Kitsap Transit'), isTrue);
    expect(
      _totalLine(tester),
      'Total ${_mbText('139')} · ${_mbText('250')} free',
    );
  });

  testWidgets('a long list scrolls within the sheet, the total and buttons '
      'staying put', (tester) async {
    await openTransit(
      tester,
      feeds: [
        for (var i = 0; i < 20; i++)
          fakeFeed('f$i', index: i, agencies: ['Feed $i'], bytes: _mb),
      ],
    );
    final list = tester.getRect(
      find.ancestor(of: find.text('Feed 0'), matching: find.byType(ListView)),
    );
    expect(list.height, lessThanOrEqualTo(800 * 0.34 + 0.5));
    // The area's map stays in view above the sheet.
    expect(tester.getTopLeft(find.text('Transit')).dy, greaterThan(250));
    final download = tester.getRect(_button('Download'));
    expect(download.bottom, lessThanOrEqualTo(800 - appNavigationBar));
    expect(find.text('Feed 19').hitTestable(), findsNothing);

    await tester.drag(find.text('Feed 2'), const Offset(0, -2000));
    await tester.pumpAndSettle();
    expect(find.text('Feed 19').hitTestable(), findsOneWidget);
    expect(tester.getRect(_button('Download')), download);
    await tester.tap(find.text('Feed 19'));
    await tester.pumpAndSettle();
    expect(_checked(tester, 'Feed 19'), isTrue);
  });

  testWidgets('on iOS, rows are grouped and choosing one fills its circle', (
    tester,
  ) async {
    await openTransit(tester, platform: TargetPlatform.iOS);
    expect(find.byType(CheckboxListTile), findsNothing);
    expect(find.byIcon(CupertinoIcons.circle), findsNWidgets(4));
    await tester.tap(find.text('Sound Transit'));
    await tester.pumpAndSettle();
    expect(find.byIcon(CupertinoIcons.checkmark_circle_fill), findsOneWidget);
    expect(_totalLine(tester), 'Total ${_mbText('157')} · 64\u00a0GB free');
  });

  group('the agencies line', () {
    Future<String> line(
      WidgetTester tester,
      List<String> agencies, {
      double width = 20 * 14,
    }) async {
      await tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: Center(
            child: SizedBox(
              width: width,
              child: AgencyLine(
                agencies: agencies,
                style: const TextStyle(fontSize: 14),
              ),
            ),
          ),
        ),
      );
      return tester
          .widgetList<Text>(find.byType(Text))
          .map((t) => t.data!)
          .join();
    }

    // The test font's letters are as wide as they're high: 20 to a line.
    testWidgets('shows them all when they fit', (tester) async {
      expect(await line(tester, ['Aaaa', 'Bbbb', 'Cccc']), 'Aaaa, Bbbb, Cccc');
    });

    testWidgets('shows as many as fit, and how many more', (tester) async {
      expect(
        await line(tester, ['Aaaa', 'Bbbb', 'Cccc', 'Dddd', 'Eeee']),
        'Aaaa, Bbbb, Cccc +2',
      );
    });

    testWidgets('cuts the first short if even it does not fit', (tester) async {
      expect(
        await line(tester, ['A very long agency name', 'Bbbb']),
        'A very long agency name +1',
      );
      expect(
        tester.widget<Text>(find.text('A very long agency name')).overflow,
        TextOverflow.ellipsis,
      );
    });
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
