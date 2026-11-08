import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

const _screen = Size(400, 800);
const _low = SheetAnchor.fraction(0.25);
const _high = SheetAnchor.fraction(0.75);

/// A sheet with a page of [rows] rows, resting at its lower anchor.
Future<SheetController> _pump(
  WidgetTester tester, {
  required bool draggable,
  int rows = 100,
}) async {
  tester.view
    ..physicalSize = _screen * 2
    ..devicePixelRatio = 2;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: SheetScaffold(
          body: const SizedBox.expand(),
          sheet: Navigator(
            pages: [
              SheetPage(
                child: SheetContent(
                  anchors: const [_low, _high],
                  draggable: draggable,
                  slivers: [
                    SliverList.builder(
                      itemCount: rows,
                      itemBuilder: (context, i) =>
                          SizedBox(height: 50, child: Text('row $i')),
                    ),
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
  return SheetScaffold.controllerOf(tester.element(find.text('row 0')));
}

double _rowTop(WidgetTester tester) => tester.getTopLeft(find.text('row 0')).dy;

void main() {
  testWidgets('a draggable page moves with a finger', (tester) async {
    final sheet = await _pump(tester, draggable: true);
    final before = sheet.extent;
    await tester.drag(find.text('row 1'), const Offset(0, -100));
    expect(sheet.extent, greaterThan(before));
  });

  for (final (name, delta) in [
    ('up', const Offset(0, -100)),
    ('down', const Offset(0, 100)),
  ]) {
    testWidgets('otherwise a finger dragging $name leaves the sheet', (
      tester,
    ) async {
      final sheet = await _pump(tester, draggable: false);
      final before = sheet.extent;
      final gesture = await tester.startGesture(
        tester.getCenter(find.text('row 1')),
      );
      await tester.pump();
      for (var i = 0; i < 5; i++) {
        await gesture.moveBy(delta / 5);
        await tester.pump(const Duration(milliseconds: 16));
        expect(sheet.extent, before);
      }
      await gesture.up();
      await tester.pumpAndSettle();
      expect(sheet.extent, before);
    });
  }

  testWidgets('or the drag handle', (tester) async {
    final sheet = await _pump(tester, draggable: false);
    final before = sheet.extent;
    final handle = Offset(_screen.width / 2, _screen.height - before + 12);
    final gesture = await tester.startGesture(handle);
    await tester.pump();
    await gesture.moveBy(const Offset(0, 60));
    await tester.pump(const Duration(milliseconds: 16));
    expect(sheet.extent, before);
    await gesture.up();
    await tester.pumpAndSettle();

    await tester.tapAt(handle);
    await tester.pumpAndSettle();
    expect(sheet.extent, before);
  });

  testWidgets('or a mouse wheel', (tester) async {
    final sheet = await _pump(tester, draggable: false);
    final before = sheet.extent;
    final mouse = TestPointer(1, PointerDeviceKind.mouse);
    await tester.sendEventToBinding(
      mouse.hover(tester.getCenter(find.text('row 1'))),
    );
    await tester.sendEventToBinding(mouse.scroll(const Offset(0, 40)));
    await tester.pumpAndSettle();
    expect(sheet.extent, before);
  });

  testWidgets('the content still scrolls', (tester) async {
    await _pump(tester, draggable: false);
    final before = _rowTop(tester);
    await tester.drag(find.text('row 1'), const Offset(0, -30));
    await tester.pumpAndSettle();
    expect(_rowTop(tester), lessThan(before));
  });

  testWidgets('and the app can still move it', (tester) async {
    final sheet = await _pump(tester, draggable: false);
    final before = sheet.extent;
    sheet.animateTo(_high);
    await tester.pumpAndSettle();
    expect(sheet.extent, greaterThan(before));
  });
}
