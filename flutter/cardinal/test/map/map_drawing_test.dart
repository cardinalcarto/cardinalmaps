import 'package:cardinal/map/map.dart';
import 'package:cardinal/map/map_drawing.dart';
import 'package:cardinal/map/overlays.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../fakes/fake_map.dart';

/// The layer over a stand-in map that records the touches it gets.
Future<List<PointerEvent>> _pumpLayer(
  WidgetTester tester,
  MapDrawing drawing,
  FakeMap map,
) async {
  final reached = <PointerEvent>[];
  await tester.pumpWidget(
    MapScope(
      controller: map,
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
              onPointerUp: reached.add,
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
  testWidgets('a page holding the map stops touches reaching it, but for two '
      'fingers moving it while it takes strokes', (tester) async {
    final drawing = MapDrawing();
    final map = FakeMap();
    final reached = await _pumpLayer(tester, drawing, map);
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
    // Taking strokes, the map only pinches, and every touch reaches it.
    expect(map.gestures, MapGestures.pinch);
    // A finger alone draws.
    await tester.dragFrom(const Offset(100, 100), const Offset(40, 30));
    expect(strokes, hasLength(1));
    expect(strokes.single.first, const Offset(100, 100));
    expect(strokes.single.last, const Offset(140, 130));
    expect(reached.whereType<PointerDownEvent>(), hasLength(1));

    // A second finger landing mid-stroke throws it away; both reach the map, from
    // their downs, to pinch it.
    reached.clear();
    final one = await tester.startGesture(const Offset(100, 100));
    await one.moveTo(const Offset(120, 110));
    final two = await tester.startGesture(const Offset(200, 200));
    await one.moveTo(const Offset(140, 130));
    await two.moveTo(const Offset(150, 160));
    await two.up();
    // The finger left down doesn't draw again, nor does one landing beside it.
    await one.moveTo(const Offset(160, 150));
    final three = await tester.startGesture(const Offset(300, 300));
    await three.moveTo(const Offset(320, 340));
    await three.up();
    await one.up();
    await tester.pumpAndSettle();
    expect(strokes, hasLength(1));
    expect(reached.whereType<PointerDownEvent>().map((e) => e.position), const [
      Offset(100, 100),
      Offset(200, 200),
      Offset(300, 300),
    ]);
    expect(reached.whereType<PointerUpEvent>(), hasLength(3));

    // Then a finger alone draws again.
    await tester.dragFrom(const Offset(100, 100), const Offset(40, 30));
    expect(strokes, hasLength(2));

    // Held without taking strokes: nothing reaches the map.
    reached.clear();
    drawing.hold(#test);
    await tester.pump();
    expect(map.gestures, MapGestures.all);
    await tester.dragFrom(const Offset(50, 50), const Offset(40, 0));
    expect(reached, isEmpty);
    expect(strokes, hasLength(2));

    // Only its holder lets it go.
    drawing.release(#other);
    expect(drawing.frozen, isTrue);
    drawing.release(#test);
    await tester.pumpAndSettle();
    await tester.dragFrom(const Offset(50, 50), const Offset(40, 0));
    expect(reached, isNotEmpty);
    expect(map.gestures, MapGestures.all);
  });
}
