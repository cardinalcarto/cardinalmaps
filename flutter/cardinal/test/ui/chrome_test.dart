import 'package:cardinal/home/navigation_bar.dart';
import 'package:cardinal/map/map_control.dart';
import 'package:cardinal/saved/saved_feedback.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/ui/glass.dart';
import 'package:cardinal/ui/theme.dart';

import 'dart:ui' show Tristate;

import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoButton;
import 'package:flutter_test/flutter_test.dart';
import 'package:liquid_glass_renderer/liquid_glass_renderer.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';

const _ios = TargetPlatform.iOS;

/// The tab bar's [label]: the search field's hint is "Search" too.
Finder _tab(String label) => find.descendant(
  of: find.byType(AppNavigationBar),
  matching: find.bySemanticsLabel(label),
);

/// Whether the tab labeled [label] is marked as the current page.
bool _selected(WidgetTester tester, String label) =>
    tester.getSemantics(_tab(label)).flagsCollection.isSelected ==
    Tristate.isTrue;

void main() {
  group('the iOS tab bar', () {
    testWidgets('labels every page on glass, Search among them', (
      tester,
    ) async {
      await pumpFakeApp(tester, platform: _ios);
      for (final label in ['Saved', 'Search', 'Transit', 'Offline']) {
        expect(
          find.descendant(
            of: find.byType(AppNavigationBar),
            matching: find.text(label),
          ),
          findsOneWidget,
        );
      }
      expect(
        find.descendant(
          of: find.byType(AppNavigationBar),
          matching: find.text('Nearby'),
        ),
        findsNothing,
      );
      expect(find.byType(IconButton), findsNothing);
    });

    testWidgets('marks the current page, and goes to the one tapped', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      final router = await pumpFakeApp(tester, platform: _ios);
      expect(_selected(tester, 'Search'), isTrue);
      expect(_selected(tester, 'Saved'), isFalse);

      await tester.tap(_tab('Saved'));
      await tester.pumpAndSettle();
      expect(router.state.uri.path, '/saved');
      expect(_selected(tester, 'Saved'), isTrue);
      expect(_selected(tester, 'Search'), isFalse);
      semantics.dispose();
    });
  });

  group('the iOS sheet', () {
    testWidgets('is glass while it peeks', (tester) async {
      await pumpFakeApp(tester, platform: _ios);
      expect(
        find.descendant(
          of: find.byKey(sheetGlassKey),
          matching: find.byType(LiquidGlass),
        ),
        findsOneWidget,
      );
    });

    testWidgets('has glass at 40%, and none at full height', (tester) async {
      final router = await pumpFakeApp(tester, platform: _ios);
      expect(find.byKey(sheetGlassKey), findsOneWidget);
      router.go('/saved');
      await tester.pumpAndSettle();
      // Glass while it's at 40%...
      expect(find.byKey(sheetGlassKey), findsOneWidget);
      SheetScaffold.controllerOf(
        tester.element(find.text('Saved places').first),
      ).animateTo(const SheetAnchor.full());
      await tester.pumpAndSettle();
      // ...and none once it's full.
      expect(find.byKey(sheetGlassKey), findsNothing);
    });

    testWidgets('is solid on Android, with no glass at all', (tester) async {
      await pumpFakeApp(tester, platform: TargetPlatform.android);
      expect(find.byKey(sheetGlassKey), findsNothing);
      expect(find.byType(LiquidGlass), findsNothing);
    });

    test('turns solid from a third of the way up to halfway', () {
      const geometry = SheetGeometry(
        size: Size(400, 900),
        padding: EdgeInsets.zero,
        placement: SheetPlacement.bottom,
      );
      expect(geometry.solidity(200), 0);
      expect(geometry.solidity(300), 0);
      expect(geometry.solidity(375), closeTo(0.5, 1e-9));
      expect(geometry.solidity(450), 1);
      expect(geometry.solidity(900), 1);
    });
  });

  group('messages on iOS', () {
    testWidgets('show at the top, take their action, and go by themselves', (
      tester,
    ) async {
      await pumpFakeApp(tester, platform: _ios);
      var undone = 0;
      final context = tester.element(find.text('Search').first);
      showSheetMessage(
        context,
        'Removed from Saved',
        action: 'Undo',
        onAction: () => undone++,
      );
      await tester.pumpAndSettle();
      expect(find.byType(SnackBar), findsNothing);
      final message = find.text('Removed from Saved');
      expect(message, findsOneWidget);
      expect(tester.getTopLeft(message).dy, lessThan(appScreen.height / 4));

      await tester.tap(find.text('Undo'));
      await tester.pumpAndSettle();
      expect(undone, 1);
      expect(message, findsNothing);

      showSheetMessage(context, 'Saved');
      await tester.pumpAndSettle();
      expect(find.text('Saved'), findsWidgets);
      await tester.pump(const Duration(seconds: 4));
      await tester.pumpAndSettle();
      // Only the tab's label is left.
      expect(find.text('Saved'), findsOneWidget);
    });

    testWidgets('a new one takes the place of the last', (tester) async {
      await pumpFakeApp(tester, platform: _ios);
      final context = tester.element(find.text('Search').first);
      showSheetMessage(context, 'First');
      await tester.pumpAndSettle();
      showSheetMessage(context, 'Second');
      await tester.pumpAndSettle();
      expect(find.text('First'), findsNothing);
      expect(find.text('Second'), findsOneWidget);
      await tester.pump(const Duration(seconds: 4));
      await tester.pumpAndSettle();
    });
  });

  group('MapControl', () {
    Future<void> pump(WidgetTester tester, TargetPlatform platform) =>
        tester.pumpWidget(
          MaterialApp(
            theme: appTheme(Brightness.light, platform: platform),
            home: Scaffold(
              body: Center(
                child: MapControl(
                  tooltip: 'Map layers',
                  icon: Icons.layers_outlined,
                  onPressed: () {},
                ),
              ),
            ),
          ),
        );

    testWidgets('is a bare icon on iOS, for glass to group', (tester) async {
      await pump(tester, _ios);
      expect(find.byType(FloatingActionButton), findsNothing);
      expect(find.byType(CupertinoButton), findsOneWidget);
      expect(
        tester.getSize(find.byType(CupertinoButton)),
        const Size.square(MapControl.size),
      );
    });

    testWidgets('is a floating action button elsewhere', (tester) async {
      await pump(tester, TargetPlatform.android);
      expect(find.byType(FloatingActionButton), findsOneWidget);
      expect(find.byTooltip('Map layers'), findsOneWidget);
    });
  });
}
