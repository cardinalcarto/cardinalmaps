import 'package:cardinal/map/compass.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

Widget _app(double bearing, VoidCallback onPressed) => MaterialApp(
  home: Scaffold(
    body: Center(
      child: Compass(bearing: bearing, onPressed: onPressed),
    ),
  ),
);

double _opacity(WidgetTester tester) =>
    tester.widget<AnimatedOpacity>(find.byType(AnimatedOpacity)).opacity;

void main() {
  testWidgets('hides while the map is north up', (tester) async {
    var taps = 0;
    await tester.pumpWidget(_app(0, () => taps++));
    await tester.pumpAndSettle();
    expect(_opacity(tester), 0);
    expect(
      find.byTooltip('Turn north up', skipOffstage: false),
      findsOneWidget,
    );
    await tester.tap(find.byType(Compass), warnIfMissed: false);
    expect(taps, 0);
  });

  testWidgets(
    'slides in from the end once the map turns, and a tap turns it back',
    (tester) async {
      var taps = 0;
      await tester.pumpWidget(_app(0, () => taps++));
      await tester.pumpWidget(_app(42, () => taps++));
      // On its way in from the right.
      await tester.pump(const Duration(milliseconds: 50));
      final slide = tester.widget<AnimatedSlide>(find.byType(AnimatedSlide));
      expect(slide.offset, Offset.zero);
      await tester.pumpAndSettle();
      expect(_opacity(tester), 1);
      expect(find.byTooltip('Turn north up'), findsOneWidget);
      await tester.tap(find.byType(FloatingActionButton));
      expect(taps, 1);
    },
  );

  testWidgets('the needle points to north on screen', (tester) async {
    await tester.pumpWidget(_app(90, () {}));
    final turn = tester.widget<Transform>(
      find
          .descendant(
            of: find.byType(FloatingActionButton),
            matching: find.byType(Transform),
          )
          .first,
    );
    // The map turned to face east, so north is to the left: a quarter turn back.
    expect(turn.transform.getRotation().entry(0, 1), closeTo(1, 1e-9));
  });
}
