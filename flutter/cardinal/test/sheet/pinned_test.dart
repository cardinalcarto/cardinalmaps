import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import 'held_harness.dart' show handle, navigationBar, screen;

const _barHeight = 60.0;
const _bodyHeight = 100.0;
const _slotHeight = 40.0;

/// What covers the bottom of the window, and the handle over the content.
const _inset = navigationBar + handle;

class _Bar extends SliverPersistentHeaderDelegate {
  const _Bar();

  @override
  double get minExtent => _barHeight;

  @override
  double get maxExtent => _barHeight;

  @override
  Widget build(
    BuildContext context,
    double shrinkOffset,
    bool overlapsContent,
  ) => const SheetSection(
    id: #bar,
    child: SizedBox.expand(child: Text('bar')),
  );

  @override
  bool shouldRebuild(_Bar oldDelegate) => false;
}

/// A page with a bar pinned to the top of its content, and under it a section, then a
/// slot.
Future<SheetController> _pump(
  WidgetTester tester,
  List<SheetAnchor> anchors, {
  Object? held,
}) async {
  tester.view
    ..physicalSize = screen * 2
    ..devicePixelRatio = 2
    ..padding = const FakeViewPadding(top: 80, bottom: 40)
    ..viewPadding = const FakeViewPadding(top: 80, bottom: 40);
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: SheetScaffold(
          body: const SizedBox.expand(),
          sheet: Navigator(
            pages: [
              SheetPage<void>(
                key: const ValueKey('page'),
                child: SheetContent(
                  anchors: anchors,
                  held: held,
                  slivers: const [
                    SliverPersistentHeader(pinned: true, delegate: _Bar()),
                    SliverToBoxAdapter(
                      child: SheetSection(
                        id: #body,
                        child: SizedBox(
                          height: _bodyHeight,
                          child: Text('body'),
                        ),
                      ),
                    ),
                    SliverToBoxAdapter(
                      child: SheetSlot(
                        id: #slot,
                        child: SizedBox(
                          height: _slotHeight,
                          child: Text('slot'),
                        ),
                      ),
                    ),
                    SliverToBoxAdapter(child: SizedBox(height: 1000)),
                  ],
                ),
              ),
            ],
            onDidRemovePage: (_) {},
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return SheetScaffold.controllerOf(tester.element(find.text('bar')));
}

void main() {
  testWidgets('a section in a pinned sliver shows whole at its anchor', (
    tester,
  ) async {
    final sheet = await _pump(tester, const [SheetAnchor.section(#bar)]);
    expect(sheet.extent, _barHeight + _inset);
    expect(
      tester.getRect(find.text('bar')).bottom,
      screen.height - navigationBar,
    );
  });

  testWidgets('a section below a pinned sliver shows whole at its anchor', (
    tester,
  ) async {
    final sheet = await _pump(tester, const [
      SheetAnchor.section(#body),
      SheetAnchor.full(),
    ]);
    expect(sheet.extent, _barHeight + _bodyHeight + _inset);
    expect(
      tester.getRect(find.text('body')).bottom,
      screen.height - navigationBar,
    );

    // Wherever the content is scrolled to.
    sheet.animateTo(const SheetAnchor.full());
    await tester.pumpAndSettle();
    await tester.drag(find.text('body'), const Offset(0, -300));
    await tester.pumpAndSettle();
    expect(sheet.anchors.first.extent, _barHeight + _bodyHeight + _inset);
  });

  testWidgets('a slot below a pinned sliver is held under the section over '
      'it', (tester) async {
    final sheet = await _pump(tester, const [
      SheetAnchor.section(#body),
    ], held: #slot);
    expect(sheet.extent, _barHeight + _bodyHeight + _slotHeight + _inset);
    expect(
      tester.getRect(find.text('slot')).bottom,
      screen.height - navigationBar,
    );
  });
}
