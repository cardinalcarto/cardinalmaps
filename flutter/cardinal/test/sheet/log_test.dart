import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter_test/flutter_test.dart';

import 'held_harness.dart';

/// A page as directions has it: a low anchor, a header that holds lightly over it,
/// and a range from the header to the top.
const _waypoint = SheetAnchor.section(#header, gravity: 1200);
const _range = SheetRange(_waypoint, full, carriesIntoScroll: true);

const _physics = SheetPhysics(platform: TargetPlatform.android);
final _anchors = [
  ResolvedAnchor(footerAnchor, 94, _physics.gravity),
  const ResolvedAnchor(_waypoint, 194, 1200),
];
const _ranges = [ResolvedRange(_range, 194, 800)];

/// The line for a release from [extent] at [velocity] on that page.
String _line(double extent, double velocity) => SheetLog.release(
  extent: extent,
  velocity: velocity,
  anchors: _anchors,
  ranges: _ranges,
  decision: _physics.decide(_anchors, _ranges, extent, velocity),
);

/// What a line says, by key.
Map<String, String> _fields(String line) {
  final words = line.split(' ');
  expect(words.take(2), ['SHEET', 'release']);
  return {
    for (final word in words.skip(2))
      word.substring(0, word.indexOf('=')): word.substring(
        word.indexOf('=') + 1,
      ),
  };
}

void main() {
  test('logging is off unless the app is built with it', () {
    expect(SheetLog.enabled, const bool.fromEnvironment('CARDINAL_SHEET_LOG'));
  });

  group('the line for a release', () {
    test('is one line of key=value pairs after the prefix', () {
      final line = _line(94, 1500);
      expect(line, isNot(contains('\n')));
      expect(
        line,
        'SHEET release extent=94.0 velocity=1500.0 via=sheet finger=none '
        'carry=0 anchors=94.0@4800,194.0@1200 ranges=194.0..800.0 '
        'rule=glide-into-range at=194.0 arrival=1267 gravity=1200 '
        'landing=486.9 rest=range rest_extent=486.9 rest_range=194.0..800.0',
      );
      for (final word in line.split(' ').skip(2)) {
        expect(word, matches(RegExp(r'^[a-z_]+=[^=\s]+$')));
      }
    });

    test('says what let go, how fast the finger was, and whether the '
        'content can scroll on', () {
      final fields = _fields(
        SheetLog.release(
          extent: 94,
          velocity: 1500,
          anchors: _anchors,
          ranges: _ranges,
          decision: _physics.decide(_anchors, _ranges, 94, 1500),
          content: true,
          carries: true,
          // On screen, up is negative.
          finger: const VelocityEstimate(
            pixelsPerSecond: Offset(12, -1520.25),
            confidence: 1,
            duration: Duration(milliseconds: 83),
            offset: Offset(1, -96.5),
          ),
        ),
      );
      expect(fields['via'], 'content');
      expect(fields['carry'], '1');
      expect(fields['finger'], '1520.3');
      expect(fields['finger_px'], '96.5');
      expect(fields['finger_ms'], '83');
    });

    test('for a slow release outside a range', () {
      final fields = _fields(_line(130, 100));
      expect(fields['rule'], 'slow-to-nearest');
      expect(fields['rest'], 'anchor');
      expect(fields['rest_extent'], '94.0');
      expect(fields, isNot(contains('arrival')));
      expect(fields, isNot(contains('rest_range')));
    });

    test('for a slow release within a range', () {
      final fields = _fields(_line(500, -100));
      expect(fields['rule'], 'slow-in-range');
      expect(fields['rest'], 'range');
      expect(fields['rest_extent'], '500.0');
      expect(fields['rest_range'], '194.0..800.0');
    });

    test('for a glide within a range', () {
      final fields = _fields(_line(500, -1000));
      expect(fields['rule'], 'glide-within-range');
      expect(fields['rest'], 'range');
      expect(fields['landing'], fields['rest_extent']);
      expect(
        double.parse(fields['rest_extent']!),
        moreOrLessEquals(_physics.landing(500, -1000), epsilon: 0.05),
      );
    });

    test('for a glide into a range', () {
      final fields = _fields(_line(94, 3000));
      expect(fields['rule'], 'glide-into-range');
      expect(fields['at'], '194.0');
      expect(
        fields['arrival'],
        _physics.speedAt(94, 3000, 194).toStringAsFixed(0),
      );
      expect(fields['gravity'], '1200');
      // It would have landed past the top, and stops there.
      expect(double.parse(fields['landing']!), greaterThan(800));
      expect(fields['rest_extent'], '800.0');
    });

    test('for a fling caught at an anchor', () {
      final fields = _fields(_line(500, -1500));
      expect(fields['rule'], 'caught-at-anchor');
      expect(fields['at'], '194.0');
      expect(
        fields['arrival'],
        _physics.speedAt(500, -1500, 194).toStringAsFixed(0),
      );
      expect(double.parse(fields['arrival']!), lessThan(1200));
      expect(fields['gravity'], '1200');
      expect(fields['rest'], 'anchor');
      expect(fields['rest_extent'], '194.0');
    });

    test('for a fling that passes every anchor', () {
      final fields = _fields(_line(500, -6000));
      expect(fields['rule'], 'passed-all-anchors');
      // Weighed last at the lowest anchor, which is where it stops.
      expect(fields['at'], '94.0');
      expect(double.parse(fields['arrival']!), greaterThan(4800));
      expect(fields['gravity'], '4800');
      expect(fields['rest'], 'anchor');
      expect(fields['rest_extent'], '94.0');
    });

    test('for a fling that runs out before an anchor', () {
      final fields = _fields(_line(94, 600));
      expect(fields['rule'], 'ran-out-before-anchor');
      expect(double.parse(fields['landing']!), lessThan(194));
      expect(fields['rest'], 'anchor');
      expect(fields['rest_extent'], '194.0');
      expect(fields, isNot(contains('arrival')));
    });

    test('for a fling with no anchor ahead', () {
      final fields = _fields(_line(94, -600));
      expect(fields['rule'], 'no-anchor-ahead');
      expect(fields['rest_extent'], '94.0');
    });

    test('for a fling out of a range past an end with no anchor', () {
      const range = SheetRange(SheetAnchor.fraction(0.5), full);
      const ranges = [ResolvedRange(range, 400, 800)];
      final fields = _fields(
        SheetLog.release(
          extent: 500,
          velocity: 3000,
          anchors: [_anchors.first],
          ranges: ranges,
          decision: _physics.decide([_anchors.first], ranges, 500, 3000),
        ),
      );
      expect(fields['rule'], 'range-end');
      expect(fields['anchors'], '94.0@4800');
      expect(fields['rest'], 'range');
      expect(fields['rest_extent'], '800.0');
    });

    test('for a fling carried into scroll', () {
      final fields = _fields(
        SheetLog.release(
          extent: 500,
          velocity: 3000,
          anchors: _anchors,
          ranges: _ranges,
          decision: SheetDecision.range(
            SheetRule.carriedIntoScroll,
            _ranges.single,
            800,
            landing: _physics.landing(500, 3000),
            at: 800,
            arrival: _physics.speedAt(500, 3000, 800),
          ),
          content: true,
          carries: true,
        ),
      );
      expect(fields['rule'], 'carried-into-scroll');
      expect(fields['at'], '800.0');
      expect(double.parse(fields['arrival']!), inExclusiveRange(2000, 3000));
      expect(fields, isNot(contains('gravity')));
      expect(fields['rest_extent'], '800.0');
    });

    test('never writes a zero as negative', () {
      final fields = _fields(
        SheetLog.release(
          extent: 500,
          velocity: -0.0,
          anchors: _anchors,
          ranges: _ranges,
          decision: _physics.decide(_anchors, _ranges, 500, -0.0),
          finger: const VelocityEstimate(
            pixelsPerSecond: Offset(0, 0.01),
            confidence: 1,
            duration: Duration.zero,
            offset: Offset.zero,
          ),
        ),
      );
      expect(fields['velocity'], '0.0');
      expect(fields['finger'], '0.0');
      expect(fields['finger_px'], '0.0');
    });

    test('on a page with nowhere to rest', () {
      expect(
        SheetLog.release(
          extent: 0,
          velocity: 0,
          anchors: const [],
          ranges: const [],
          decision: null,
        ),
        'SHEET release extent=0.0 velocity=0.0 via=sheet finger=none carry=0 '
        'anchors=none ranges=none rule=no-anchors',
      );
    });

    test('names an anchor that holds every fling', () {
      final anchors = [
        _anchors.first,
        const ResolvedAnchor(_waypoint, 194, double.infinity),
      ];
      final fields = _fields(
        SheetLog.release(
          extent: 94,
          velocity: 8000,
          anchors: anchors,
          ranges: _ranges,
          decision: _physics.decide(anchors, _ranges, 94, 8000),
        ),
      );
      expect(fields['anchors'], '94.0@4800,194.0@inf');
      expect(fields['rule'], 'caught-at-anchor');
      expect(fields['gravity'], 'inf');
    });

    test('has a name for every rule', () {
      final names = SheetRule.values.map(SheetLog.ruleName).toList();
      expect(names.toSet(), hasLength(SheetRule.values.length));
      for (final name in names) {
        expect(name, matches(RegExp(r'^[a-z-]+$')));
      }
    });
  });

  test('the line for a drag the sheet was let go of at another speed', () {
    expect(
      SheetLog.dragEnd(reported: 2400, released: 0),
      'SHEET drag-end via=content reported=2400.0 released=0.0',
    );
  });

  // With `--dart-define=CARDINAL_SHEET_LOG=true`, what the sheet itself prints.
  group('a sheet built with logging', skip: !SheetLog.enabled, () {
    late List<String> lines;
    late DebugPrintCallback printed;

    setUp(() {
      lines = [];
      printed = debugPrint;
      debugPrint = (message, {wrapWidth}) {
        if (message != null && message.startsWith(SheetLog.prefix)) {
          lines.add(message);
        }
      };
    });

    tearDown(() => debugPrint = printed);

    Future<SheetController> pump(WidgetTester tester) => pumpHarness(
      tester,
      (key) => Harness(
        key: key,
        anchors: const [footerAnchor, _waypoint],
        freeRanges: const [_range],
        initialAnchor: 0,
      ),
    );

    testWidgets('prints one line for each release, with the finger in it', (
      tester,
    ) async {
      final sheet = await pump(tester);
      await tester.flingFrom(
        const Offset(200, footerTop + footerHeight / 2),
        const Offset(0, -60),
        1800,
      );
      debugPrint = printed;
      await tester.pumpAndSettle();
      expect(lines, hasLength(1));
      final fields = _fields(lines.single);
      expect(fields['via'], 'content');
      expect(fields['carry'], '1');
      expect(fields['rule'], 'glide-into-range');
      expect(fields['anchors'], '94.0@4800,194.0@1200');
      expect(fields['ranges'], '194.0..800.0');
      expect(
        double.parse(fields['finger']!),
        moreOrLessEquals(double.parse(fields['velocity']!), epsilon: 1),
      );
      expect(sheet.restingIn, _range);
      expect(
        sheet.extent,
        moreOrLessEquals(double.parse(fields['rest_extent']!), epsilon: 0.5),
      );
    });

    testWidgets('says a fling carried into scroll, and one the content took '
        'at the top', (tester) async {
      final sheet = await pump(tester);
      await tester.flingFrom(
        const Offset(200, footerTop + footerHeight / 2),
        const Offset(0, -60),
        3000,
      );
      await tester.pumpAndSettle();
      expect(sheet.extent, screen.height);
      expect(_fields(lines.single)['rule'], 'carried-into-scroll');

      lines.clear();
      await tester.flingFrom(
        const Offset(200, 400),
        const Offset(0, -60),
        2000,
      );
      debugPrint = printed;
      await tester.pumpAndSettle();
      expect(lines, hasLength(2));
      expect(lines.first, startsWith('SHEET drag-end via=content reported='));
      expect(lines.first, endsWith('released=0.0'));
      expect(_fields(lines.last)['velocity'], '0.0');
    });
  });
}
