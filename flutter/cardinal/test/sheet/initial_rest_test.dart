import 'dart:ui' show lerpDouble;

import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import 'held_harness.dart';

/// What covers the bottom of the harness's window, and the handle over the content.
const _inset = footerHeight + navigationBar + handle;

const _low = SheetAnchor.fraction(0.25);
const _top = SheetAnchor.section(#top);
const _range = SheetRange(_top, SheetAnchor.full());

/// A page that rests at a quarter, and one pushed over it that opens within a range,
/// showing down to a section that isn't there until [_PagesState.fill].
class _Pages extends StatefulWidget {
  const _Pages({super.key});

  @override
  State<_Pages> createState() => _PagesState();
}

class _PagesState extends State<_Pages> {
  bool _pushed = false;
  bool _filled = false;

  void push() => setState(() => _pushed = true);
  void fill() => setState(() => _filled = true);

  @override
  Widget build(BuildContext context) => MaterialApp(
    home: Scaffold(
      body: SheetScaffold(
        body: const SizedBox.expand(),
        sheet: Navigator(
          pages: [
            SheetPage<void>(
              key: const ValueKey('first'),
              child: SheetContent(
                anchors: const [_low],
                slivers: const [
                  SliverToBoxAdapter(
                    child: SizedBox(height: 50, child: Text('first')),
                  ),
                ],
              ),
            ),
            if (_pushed)
              SheetPage<void>(
                key: const ValueKey('second'),
                child: SheetContent(
                  anchors: const [_top],
                  freeRanges: const [_range],
                  initialSection: #body,
                  initialMaxFraction: 0.6,
                  slivers: [
                    const SliverToBoxAdapter(
                      child: SheetSection(
                        id: #top,
                        child: SizedBox(height: 80, child: Text('second')),
                      ),
                    ),
                    if (_filled)
                      const SliverToBoxAdapter(
                        child: SheetSection(
                          id: #body,
                          child: SizedBox(height: 300),
                        ),
                      ),
                  ],
                ),
              ),
          ],
          onDidRemovePage: (_) {},
        ),
      ),
    ),
  );
}

void main() {
  testWidgets('opens within a range, showing down to a section', (
    tester,
  ) async {
    final sheet = await pumpHarness(
      tester,
      (key) => Harness(key: key, initialSection: #three),
    );
    expect(sheet.restingIn, freeRange);
    expect(sheet.restingAt, isNull);
    expect(sheet.extent, headerHeight + 4 * rowHeight + _inset);
    expect(tester.getRect(row(3)).bottom, footerTop);

    // It rests there as anywhere else in the range: let go, it stays.
    await tester.timedDrag(
      find.text('header'),
      const Offset(0, -100),
      const Duration(seconds: 1),
    );
    await tester.pumpAndSettle();
    expect(sheet.restingIn, freeRange);
    expect(
      sheet.extent,
      greaterThan(headerHeight + 4 * rowHeight + _inset + 50),
    );
  });

  testWidgets('opens on its first frame', (tester) async {
    final sheet = await pumpHarness(
      tester,
      (key) => Harness(key: key, initialSection: #three),
      settle: false,
    );
    expect(tester.getRect(row(3)).bottom, footerTop);
    expect(tester.getRect(find.byKey(const ValueKey('footer'))).top, footerTop);
    await tester.pumpAndSettle();
    expect(sheet.extent, headerHeight + 4 * rowHeight + _inset);
  });

  testWidgets('opens no higher than the fraction it is given', (tester) async {
    final sheet = await pumpHarness(
      tester,
      (key) =>
          Harness(key: key, initialSection: #three, initialMaxFraction: 0.5),
    );
    expect(sheet.restingIn, freeRange);
    expect(sheet.extent, screen.height / 2);
  });

  testWidgets('counts a held slot below the section', (tester) async {
    final sheet = await pumpHarness(
      tester,
      (key) => Harness(key: key, initialSection: #three, held: 8),
    );
    expect(sheet.extent, headerHeight + 5 * rowHeight + _inset);
    expect(tester.getRect(row(3)).bottom, footerTop - rowHeight);
    expect(rowTop(tester, 8), footerTop - rowHeight);
  });

  testWidgets('opens at the initial anchor if no range has that extent', (
    tester,
  ) async {
    final sheet = await pumpHarness(
      tester,
      (key) => Harness(
        key: key,
        anchors: const [footerAnchor, headerAnchor, full],
        freeRanges: const [],
        initialSection: #three,
      ),
    );
    expect(sheet.restingAt, headerAnchor);
    expect(sheet.extent, headerHeight + _inset);
  });

  testWidgets('opens at the anchor at the end of the range, if it opens '
      'there', (tester) async {
    // All of the content down to the header is the range's lower end.
    final sheet = await pumpHarness(
      tester,
      (key) => Harness(key: key, initialAnchor: 0, initialSection: #header),
    );
    expect(sheet.restingAt, headerAnchor);
    expect(sheet.extent, headerHeight + _inset);
  });

  testWidgets('waits for a section laid out late, then opens there at once', (
    tester,
  ) async {
    final sheet = await pumpHarness(
      tester,
      (key) => Harness(key: key, initialSection: #three, sectionThree: false),
    );
    expect(sheet.hasExtent, isFalse);

    final extents = <double>[];
    sheet.addListener(() => extents.add(sheet.extent));
    harness.currentState!.change(
      () => harness.currentState!.sectionThree = true,
    );
    await tester.pumpAndSettle();
    expect(extents, [headerHeight + 4 * rowHeight + _inset]);
    expect(sheet.restingIn, freeRange);
  });

  testWidgets('a page pushed opens towards a section laid out a frame late, '
      'without a jump', (tester) async {
    tester.view
      ..physicalSize = screen * 2
      ..devicePixelRatio = 2;
    addTearDown(tester.view.reset);
    final pages = GlobalKey<_PagesState>();
    await tester.pumpWidget(_Pages(key: pages));
    await tester.pumpAndSettle();
    final sheet = SheetScaffold.controllerOf(
      tester.element(find.text('first')),
    );
    const from = 200.0;
    const to = 80.0 + 300 + handle;
    expect(sheet.extent, from);

    pages.currentState!.push();
    await tester.pump();
    // Nowhere to go yet.
    expect(sheet.extent, from);
    pages.currentState!.fill();

    var last = from;
    for (var ms = 16; ms < 300; ms += 16) {
      await tester.pump(const Duration(milliseconds: 16));
      // On its way from the first frame it can be, as far as the transition has got.
      expect(
        sheet.extent,
        moreOrLessEquals(
          lerpDouble(from, to, Curves.easeOutCubic.transform(ms / 300))!,
          epsilon: 0.5,
        ),
      );
      expect(sheet.extent, greaterThan(last));
      last = sheet.extent;
    }
    await tester.pumpAndSettle();
    expect(sheet.extent, moreOrLessEquals(to));
    expect(sheet.restingIn, _range);
  });

  testWidgets('a collapsed sheet expands to where it first rested', (
    tester,
  ) async {
    final sheet = await pumpHarness(
      tester,
      (key) => Harness(key: key, initialSection: #three, collapsible: true),
    );
    final opened = sheet.extent;
    expect(sheet.restingIn, freeRange);
    await moveTo(tester, sheet, const SheetAnchor.collapsed());
    expect(sheet.isCollapsed, isTrue);
    sheet.expand();
    await tester.pumpAndSettle();
    expect(sheet.restingIn, freeRange);
    expect(sheet.extent, moreOrLessEquals(opened, epsilon: 0.5));
  });
}
