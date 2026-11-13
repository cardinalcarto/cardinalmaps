import 'package:cardinal/map/map.dart';
import 'package:cardinal/map/map_drawing.dart';
import 'package:cardinal/map/overlays.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_map.dart';

/// The layer over a stand-in map that records the touches it gets.
Future<List<PointerEvent>> _pumpLayer(
  WidgetTester tester,
  MapDrawing drawing,
) async {
  final reached = <PointerEvent>[];
  await tester.pumpWidget(
    MapScope(
      controller: FakeMap(),
      overlays: MapOverlays(),
      drawing: drawing,
      child: MaterialApp(
        home: Stack(
          fit: StackFit.expand,
          children: [
            Listener(
              behavior: HitTestBehavior.opaque,
              onPointerDown: reached.add,
              onPointerMove: reached.add,
              onPointerSignal: reached.add,
            ),
            MapDrawingLayer(
              child: Align(
                child: TextButton(onPressed: () {}, child: const Text('B')),
              ),
            ),
          ],
        ),
      ),
    ),
  );
  return reached;
}

void main() {
  testWidgets('touches reach the map until a page holds it', (tester) async {
    final drawing = MapDrawing();
    final reached = await _pumpLayer(tester, drawing);
    await tester.dragFrom(const Offset(50, 50), const Offset(40, 0));
    expect(reached, isNotEmpty);

    reached.clear();
    final strokes = <List<Offset>>[];
    drawing.hold(#test, onStroke: strokes.add);
    await tester.pumpAndSettle();
    // The map's buttons fade away, and take no taps.
    expect(
      tester.widget<AnimatedOpacity>(find.byType(AnimatedOpacity)).opacity,
      0,
    );
    // Two fingers pinching: nothing reaches the map, and only the first draws.
    final one = await tester.startGesture(const Offset(100, 100));
    final two = await tester.startGesture(const Offset(200, 200));
    await one.moveTo(const Offset(120, 110));
    await two.moveTo(const Offset(150, 160));
    await one.moveTo(const Offset(140, 130));
    await two.up();
    await one.up();
    expect(reached, isEmpty);
    expect(strokes.single, const [
      Offset(100, 100),
      Offset(120, 110),
      Offset(140, 130),
    ]);

    // Held without taking strokes: still nothing reaches the map.
    drawing.hold(#test);
    await tester.pump();
    await tester.dragFrom(const Offset(50, 50), const Offset(40, 0));
    expect(reached, isEmpty);
    expect(strokes, hasLength(1));

    // Only its holder lets it go.
    drawing.release(#other);
    expect(drawing.frozen, isTrue);
    drawing.release(#test);
    await tester.pumpAndSettle();
    await tester.dragFrom(const Offset(50, 50), const Offset(40, 0));
    expect(reached, isNotEmpty);
  });
}
