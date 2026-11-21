import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

/// The window, with a 40 pixel status bar and a 20 pixel navigation bar.
const screen = Size(400, 800);
const statusBar = 40.0;
const navigationBar = 20.0;
const handle = 24.0;

/// The content: a header, then rows of one height, each in a slot named by its index.
const headerHeight = 100.0;
const rowHeight = 60.0;
const footerHeight = 50.0;

const footerAnchor = SheetAnchor.footer();
const headerAnchor = SheetAnchor.section(#header);
const full = SheetAnchor.full();
const freeRange = SheetRange(headerAnchor, full);

/// A sheet with one page, whose configuration tests change through [HarnessState].
class Harness extends StatefulWidget {
  const Harness({
    super.key,
    this.held,
    this.footer = true,
    this.anchors = const [footerAnchor, headerAnchor],
    this.freeRanges = const [freeRange],
    this.initialAnchor = 1,
    this.initialSection,
    this.initialMaxFraction = 1,
    this.rows = 20,
    this.sectionThree = true,
    this.collapsible = false,
  });

  final Object? held;
  final bool footer;
  final List<SheetAnchor> anchors;
  final List<SheetRange> freeRanges;
  final int initialAnchor;
  final Object? initialSection;
  final double initialMaxFraction;
  final int rows;

  /// Whether the fourth row is a section from the first.
  final bool sectionThree;
  final bool collapsible;

  @override
  State<Harness> createState() => HarnessState();
}

class HarnessState extends State<Harness> {
  late Object? held = widget.held;
  late bool footer = widget.footer;
  late bool sectionThree = widget.sectionThree;
  double footerExtent = footerHeight;

  /// Rows that are taller than the rest, by their index.
  Map<int, double> heights = {};

  void change(VoidCallback change) => setState(change);

  @override
  Widget build(BuildContext context) => MaterialApp(
    home: Scaffold(
      // The sheet handles the keyboard itself.
      resizeToAvoidBottomInset: false,
      body: SheetScaffold(
        body: const SizedBox.expand(),
        sheet: Navigator(
          pages: [
            SheetPage<void>(
              key: const ValueKey('page'),
              child: SheetContent(
                anchors: widget.anchors,
                freeRanges: widget.freeRanges,
                initialAnchor: widget.initialAnchor,
                initialSection: widget.initialSection,
                initialMaxFraction: widget.initialMaxFraction,
                collapsible: widget.collapsible,
                held: held,
                footer: footer
                    ? ColoredBox(
                        key: const ValueKey('footer'),
                        color: Colors.blue,
                        child: SizedBox(
                          height: footerExtent,
                          child: const Center(child: Text('footer')),
                        ),
                      )
                    : null,
                slivers: [
                  const SliverToBoxAdapter(
                    child: SheetSection(
                      id: #header,
                      child: SizedBox(
                        key: ValueKey('header'),
                        height: headerHeight,
                        child: Text('header'),
                      ),
                    ),
                  ),
                  for (var i = 0; i < widget.rows; i++)
                    SliverToBoxAdapter(
                      // The fourth row is a section too, for anchors below a slot.
                      child: i == 3 && sectionThree
                          ? SheetSection(id: #three, child: _slot(i))
                          : _slot(i),
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

  Widget _slot(int i) => SheetSlot(
    id: i,
    child: TestRow(
      key: ValueKey('row$i'),
      index: i,
      height: heights[i] ?? rowHeight,
    ),
  );
}

/// A row with state of its own, that counts what the tests check never happens as the
/// sheet moves.
class TestRow extends StatefulWidget {
  const TestRow({super.key, required this.index, required this.height});

  final int index;
  final double height;

  @override
  State<TestRow> createState() => TestRowState();
}

class TestRowState extends State<TestRow> {
  int taps = 0;
  int builds = 0;
  int layouts = 0;
  int paints = 0;
  double swiped = 0;

  /// What [SheetSlot.displacementOf] has said, in order.
  final List<double> displacements = [];

  ValueListenable<double>? _displacement;

  double get displacement => _displacement!.value;

  void _record() => displacements.add(displacement);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _displacement?.removeListener(_record);
    _displacement = SheetSlot.displacementOf(context)..addListener(_record);
  }

  @override
  void dispose() {
    _displacement?.removeListener(_record);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    builds++;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => setState(() => taps++),
      onHorizontalDragUpdate: (details) => swiped += details.delta.dx,
      child: _Probe(
        onLayout: () => layouts++,
        onPaint: () => paints++,
        child: ColoredBox(
          color: Colors.white,
          child: SizedBox(
            height: widget.height,
            child: Text('row ${widget.index}: $taps'),
          ),
        ),
      ),
    );
  }
}

class _Probe extends SingleChildRenderObjectWidget {
  const _Probe({
    required this.onLayout,
    required this.onPaint,
    required super.child,
  });

  final VoidCallback onLayout;
  final VoidCallback onPaint;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderProbe(onLayout, onPaint);

  @override
  void updateRenderObject(BuildContext context, _RenderProbe renderObject) {
    renderObject
      ..onLayout = onLayout
      ..onPaint = onPaint;
  }
}

class _RenderProbe extends RenderProxyBox {
  _RenderProbe(this.onLayout, this.onPaint);

  VoidCallback onLayout;
  VoidCallback onPaint;

  @override
  void performLayout() {
    onLayout();
    super.performLayout();
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    onPaint();
    super.paint(context, offset);
  }
}

final harness = GlobalKey<HarnessState>();

Future<SheetController> pumpHarness(
  WidgetTester tester,
  Harness Function(Key key) build, {
  Size size = screen,
  bool settle = true,
}) async {
  tester.view
    ..physicalSize = size * 2
    ..devicePixelRatio = 2
    ..padding = const FakeViewPadding(top: 80, bottom: 40)
    ..viewPadding = const FakeViewPadding(top: 80, bottom: 40);
  addTearDown(tester.view.reset);
  await tester.pumpWidget(build(harness));
  if (settle) await tester.pumpAndSettle();
  return SheetScaffold.controllerOf(tester.element(find.text('header')));
}

Future<void> moveTo(
  WidgetTester tester,
  SheetController sheet,
  SheetAnchor anchor,
) async {
  sheet.animateTo(anchor);
  await tester.pumpAndSettle();
}

/// Row [i], whether or not its slot is within the viewport: it's painted wherever it's
/// held.
Finder row(int i) => find.byKey(ValueKey('row$i'), skipOffstage: false);

TestRowState rowState(WidgetTester tester, int i) =>
    tester.state<TestRowState>(row(i));

/// Where the content's top is on screen with the sheet [extent] up, below where it
/// starts turning into a full screen.
double contentTop(double extent) => screen.height - extent + handle;

/// Where row [i] sits on screen in the list, scrolled to its top, with the sheet
/// [extent] up.
double slotTop(double extent, int i) =>
    contentTop(extent) + headerHeight + rowHeight * i;

/// The top of the footer on screen, wherever the sheet is.
const footerTop = 800 - navigationBar - footerHeight;

/// Where row [i] is painted and how solid, read from the layer tree, or null if it
/// isn't painted at all. Its layer can only be in one place.
({Offset offset, double opacity})? painted(WidgetTester tester, int i) {
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find
        .ancestor(
          of: row(i),
          matching: find.byType(RepaintBoundary, skipOffstage: false),
        )
        .first,
  );
  Layer? layer = boundary.debugLayer;
  if (layer == null || !layer.attached) return null;
  var offset = Offset.zero;
  var opacity = 1.0;
  while (layer != null) {
    if (layer is OpacityLayer) opacity *= layer.alpha! / 255;
    if (layer is OffsetLayer) offset += layer.offset;
    layer = layer.parent;
  }
  return (offset: offset, opacity: opacity);
}

/// The top of row [i] on screen, where it's painted, which must be where its render
/// object says it is.
double rowTop(WidgetTester tester, int i) {
  final top = tester.getRect(row(i)).top;
  expect(
    painted(tester, i)?.offset.dy,
    moreOrLessEquals(top, epsilon: 0.01),
    reason: 'row $i is painted where it says',
  );
  return top;
}
