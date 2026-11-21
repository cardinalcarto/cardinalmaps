import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import 'held_harness.dart' show full, handle, navigationBar, screen;

const _low = SheetAnchor.fraction(0.25);
const _footer = SheetAnchor.footer();
const _header = SheetAnchor.section(#header);
const _summary = SheetAnchor.section(#summary);
const _tripRange = SheetRange(_header, full);
const _summaryRange = SheetRange(_summary, full);

/// Where the sheet rests showing a trip whose header is [height] tall.
double _headerExtent([double height = 100]) => height + navigationBar + handle;

/// What the page shows: each with anchors, ranges and sections of its own.
enum _Showing {
  /// Rests only at full, as a page being typed into.
  editing,

  /// Rests at a quarter or at full.
  browsing,

  /// Rests at the footer or showing its header, and anywhere from there up to full.
  trip,

  /// Rests showing its summary, and anywhere from there up to full.
  summary,
}

/// When in the frame that changes the content the sheet is asked to move.
enum _Asked {
  /// Before the page is marked to build again.
  before,

  /// After it's marked, before the frame begins.
  after,

  /// While the page builds.
  building,

  /// After the frame, in a callback added before it began.
  afterFrame,
}

/// A sheet with one page, whose content changes between frames.
class _Page extends StatefulWidget {
  const _Page({super.key, required this.showing});

  final _Showing showing;

  @override
  State<_Page> createState() => _PageState();
}

class _PageState extends State<_Page> {
  late _Showing showing = widget.showing;
  double headerHeight = 100;

  /// Whether the trip has its range up to full.
  bool ranged = true;
  VoidCallback? _whileBuilding;

  /// Changes the content with [change], asking [ask] of the sheet in the same frame.
  void change(VoidCallback change, _Asked when, VoidCallback ask) {
    switch (when) {
      case _Asked.before:
        ask();
        setState(change);
      case _Asked.after:
        setState(change);
        ask();
      case _Asked.building:
        setState(() {
          change();
          _whileBuilding = ask;
        });
      case _Asked.afterFrame:
        setState(change);
        SchedulerBinding.instance.addPostFrameCallback((_) => ask());
    }
  }

  @override
  Widget build(BuildContext context) {
    _whileBuilding?.call();
    _whileBuilding = null;
    return MaterialApp(
      home: Scaffold(
        body: SheetScaffold(
          body: const SizedBox.expand(),
          sheet: Navigator(
            pages: [
              SheetPage<void>(
                key: const ValueKey('page'),
                child: SheetContent(
                  anchors: switch (showing) {
                    _Showing.editing => const [full],
                    _Showing.browsing => const [_low, full],
                    _Showing.trip => const [_footer, _header],
                    _Showing.summary => const [_summary],
                  },
                  freeRanges: switch (showing) {
                    _Showing.editing || _Showing.browsing => const [],
                    _Showing.trip => ranged ? const [_tripRange] : const [],
                    _Showing.summary => const [_summaryRange],
                  },
                  slivers: switch (showing) {
                    _Showing.editing || _Showing.browsing => const [
                      SliverToBoxAdapter(
                        child: SizedBox(height: 300, child: Text('content')),
                      ),
                    ],
                    _Showing.trip => [
                      SliverToBoxAdapter(
                        child: SheetSection(
                          id: #header,
                          child: SizedBox(
                            height: headerHeight,
                            child: const Text('content'),
                          ),
                        ),
                      ),
                      const SliverToBoxAdapter(child: SizedBox(height: 1000)),
                    ],
                    _Showing.summary => const [
                      SliverToBoxAdapter(
                        child: SheetSection(
                          id: #summary,
                          child: SizedBox(height: 200, child: Text('content')),
                        ),
                      ),
                      SliverToBoxAdapter(child: SizedBox(height: 1000)),
                    ],
                  },
                ),
              ),
            ],
            onDidRemovePage: (_) {},
          ),
        ),
      ),
    );
  }
}

final _page = GlobalKey<_PageState>();

Future<SheetController> _pump(WidgetTester tester, _Showing showing) async {
  tester.view
    ..physicalSize = screen * 2
    ..devicePixelRatio = 2
    ..padding = const FakeViewPadding(top: 80, bottom: 40)
    ..viewPadding = const FakeViewPadding(top: 80, bottom: 40);
  addTearDown(tester.view.reset);
  await tester.pumpWidget(_Page(key: _page, showing: showing));
  await tester.pumpAndSettle();
  return SheetScaffold.controllerOf(tester.element(find.text('content')));
}

/// Pumps frames until the sheet rests, and returns its extent at each frame it moved.
Future<List<double>> _settle(WidgetTester tester, SheetController sheet) async {
  final extents = <double>[];
  void record() => extents.add(sheet.extent);
  sheet.addListener(record);
  for (var i = 0; i < 200; i++) {
    await tester.pump(const Duration(milliseconds: 16));
    if (sheet.isResting && !tester.binding.hasScheduledFrame) break;
  }
  sheet.removeListener(record);
  expect(sheet.isResting, isTrue);
  return extents;
}

/// That the sheet went from [from] to [to] in one motion: every frame further the same
/// way, none of them a jump.
void _expectOneMotion(List<double> extents, double from, double to) {
  expect(extents, isNotEmpty);
  final direction = (to - from).sign;
  var last = from;
  for (final extent in extents) {
    // A spring's last frames can come back from a pixel past its target.
    expect(
      (extent - last) * direction,
      greaterThan(-1),
      reason: 'from $from to $to, never back: $extents',
    );
    expect(
      (extent - last).abs(),
      lessThan((to - from).abs() / 3),
      reason: 'from $from to $to, no jump: $extents',
    );
    last = extent;
  }
  expect(last, moreOrLessEquals(to, epsilon: 0.5));
}

void main() {
  for (final when in _Asked.values) {
    group('asked ${when.name} the content changes,', () {
      testWidgets('stays at full when full becomes the end of a range', (
        tester,
      ) async {
        final sheet = await _pump(tester, _Showing.editing);
        expect(sheet.restingAt, full);
        _page.currentState!.change(
          () => _page.currentState!.showing = _Showing.trip,
          when,
          () => sheet.animateTo(full),
        );
        final extents = await _settle(tester, sheet);
        expect(extents, isEmpty, reason: 'it was there already');
        expect(sheet.extent, screen.height);
        expect(sheet.restingIn, _tripRange);
        expect(sheet.ranges.single.range, _tripRange);
      });

      testWidgets('rises to the end of a range the new content has', (
        tester,
      ) async {
        final sheet = await _pump(tester, _Showing.browsing);
        expect(sheet.restingAt, _low);
        _page.currentState!.change(
          () => _page.currentState!.showing = _Showing.trip,
          when,
          () => sheet.animateTo(full),
        );
        final extents = await _settle(tester, sheet);
        _expectOneMotion(extents, screen.height / 4, screen.height);
        expect(sheet.restingIn, _tripRange);
      });

      testWidgets('goes to the end of a different range', (tester) async {
        final sheet = await _pump(tester, _Showing.trip);
        sheet.animateTo(_header);
        await tester.pumpAndSettle();
        expect(sheet.extent, _headerExtent());
        _page.currentState!.change(
          () => _page.currentState!.showing = _Showing.summary,
          when,
          () => sheet.animateTo(full),
        );
        final extents = await _settle(tester, sheet);
        _expectOneMotion(extents, _headerExtent(), screen.height);
        expect(sheet.restingIn, _summaryRange);
      });

      testWidgets('goes to an anchor only the new content has', (tester) async {
        final sheet = await _pump(tester, _Showing.editing);
        _page.currentState!.change(
          () => _page.currentState!.showing = _Showing.trip,
          when,
          () => sheet.animateTo(_header),
        );
        final extents = await _settle(tester, sheet);
        _expectOneMotion(extents, screen.height, _headerExtent());
        expect(sheet.restingAt, _header);
      });

      testWidgets('goes to where the new content has an anchor both have, from '
          'elsewhere', (tester) async {
        final sheet = await _pump(tester, _Showing.trip);
        sheet.animateTo(full);
        await tester.pumpAndSettle();
        expect(sheet.extent, screen.height);
        _page.currentState!.change(
          () => _page.currentState!.headerHeight = 300,
          when,
          () => sheet.animateTo(_header),
        );
        final extents = await _settle(tester, sheet);
        _expectOneMotion(extents, screen.height, _headerExtent(300));
        expect(sheet.restingAt, _header);
      });

      testWidgets('springs to where the new content has the anchor it rests '
          'at', (tester) async {
        final sheet = await _pump(tester, _Showing.trip);
        sheet.animateTo(_header);
        await tester.pumpAndSettle();
        expect(sheet.extent, _headerExtent());
        _page.currentState!.change(
          () => _page.currentState!.headerHeight = 300,
          when,
          () => sheet.animateTo(_header),
        );
        final extents = await _settle(tester, sheet);
        _expectOneMotion(extents, _headerExtent(), _headerExtent(300));
        expect(sheet.restingAt, _header);
      });

      testWidgets('waits for an anchor the new content has not got', (
        tester,
      ) async {
        final sheet = await _pump(tester, _Showing.browsing);
        expect(sheet.extent, screen.height / 4);
        final page = _page.currentState!;
        page.change(
          () => page
            ..showing = _Showing.trip
            ..ranged = false,
          when,
          () => sheet.animateTo(full),
        );
        var extents = await _settle(tester, sheet);
        // Where it rested is gone, so to the nearest anchor, as ever.
        _expectOneMotion(extents, screen.height / 4, _headerExtent());
        expect(sheet.restingAt, _header);

        // And on to it once the page has it, though the quarter is nearer.
        page.change(
          () => page.showing = _Showing.browsing,
          _Asked.before,
          () {},
        );
        extents = await _settle(tester, sheet);
        _expectOneMotion(extents, _headerExtent(), screen.height);
        expect(sheet.restingAt, full);
      });
    });
  }

  testWidgets('without a change, an anchor asked for is sprung to at once', (
    tester,
  ) async {
    final sheet = await _pump(tester, _Showing.browsing);
    sheet.animateTo(full);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 16));
    final moved = sheet.extent;
    expect(moved, greaterThan(screen.height / 4));
    final extents = await _settle(tester, sheet);
    _expectOneMotion(extents, moved, screen.height);
    expect(sheet.restingAt, full);
  });

  testWidgets('an anchor that moves in a later frame is followed, not sprung '
      'to', (tester) async {
    final sheet = await _pump(tester, _Showing.trip);
    sheet.animateTo(_header);
    await tester.pumpAndSettle();
    // Asked for where it is, with nothing changing: that's the end of it.
    sheet.animateTo(_header);
    await tester.pump();
    await tester.pump();
    _page.currentState!.change(
      () => _page.currentState!.headerHeight = 300,
      _Asked.before,
      () {},
    );
    final extents = await _settle(tester, sheet);
    expect(extents, [_headerExtent(300)]);
  });

  testWidgets('a finger that takes the sheet in the same frame keeps it', (
    tester,
  ) async {
    final sheet = await _pump(tester, _Showing.trip);
    sheet.animateTo(_header);
    await tester.pumpAndSettle();
    _page.currentState!.change(
      () => _page.currentState!.headerHeight = 300,
      _Asked.before,
      () => sheet.animateTo(full),
    );
    sheet.hold();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 16));
    expect(sheet.isHeld, isTrue);
    expect(sheet.extent, _headerExtent());
    sheet.release(0);
    await tester.pumpAndSettle();
  });
}
