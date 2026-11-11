import 'package:cardinal/ui/adaptive.dart';
import 'package:cardinal/ui/glass.dart';
import 'package:cardinal/ui/symbols.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoAlertDialog,
        CupertinoButton,
        CupertinoIcons,
        CupertinoListSection,
        CupertinoListTile,
        CupertinoSlidingSegmentedControl;
import 'package:flutter_test/flutter_test.dart';
import 'package:liquid_glass_renderer/liquid_glass_renderer.dart';
import 'package:material_ui/material_ui.dart';

Future<void> _pump(
  WidgetTester tester,
  TargetPlatform platform,
  Widget child, {
  bool highContrast = false,
}) => tester.pumpWidget(
  MaterialApp(
    theme: appTheme(Brightness.light, platform: platform),
    home: Builder(
      builder: (context) => MediaQuery(
        data: MediaQuery.of(context).copyWith(highContrast: highContrast),
        child: Scaffold(body: Center(child: child)),
      ),
    ),
  ),
);

const _ios = TargetPlatform.iOS;
const _android = TargetPlatform.android;

void main() {
  group('AppButton', () {
    for (final kind in AppButtonKind.values) {
      testWidgets('is a CupertinoButton on iOS ($kind)', (tester) async {
        var taps = 0;
        await _pump(
          tester,
          _ios,
          AppButton(label: 'Go', kind: kind, onPressed: () => taps++),
        );
        expect(find.byType(CupertinoButton), findsOneWidget);
        await tester.tap(find.text('Go'));
        expect(taps, 1);
      });
    }

    testWidgets('is Material on Android', (tester) async {
      await _pump(
        tester,
        _android,
        Column(
          children: [
            AppButton(label: 'A', icon: Icons.add, onPressed: () {}),
            AppButton(label: 'B', kind: AppButtonKind.tinted, onPressed: () {}),
            AppButton(label: 'C', kind: AppButtonKind.plain, onPressed: () {}),
          ],
        ),
      );
      expect(find.byType(CupertinoButton), findsNothing);
      expect(find.byType(FilledButton), findsNWidgets(2));
      expect(find.byType(TextButton), findsOneWidget);
      expect(find.byIcon(Icons.add), findsOneWidget);
    });
  });

  group('AppListSection', () {
    Widget section() => AppListSection(
      header: 'Lists',
      children: [
        AppListTile(title: const Text('One'), chevron: true, onTap: () {}),
        const AppListTile(title: Text('Two')),
      ],
    );

    testWidgets('is an inset grouped list on iOS', (tester) async {
      await _pump(tester, _ios, section());
      expect(find.byType(CupertinoListSection), findsOneWidget);
      expect(find.byType(CupertinoListTile), findsNWidgets(2));
      // iOS heads grouped rows in capitals.
      expect(find.text('LISTS'), findsOneWidget);
    });

    testWidgets('is a card of list tiles on Android', (tester) async {
      await _pump(tester, _android, section());
      expect(find.byType(Card), findsOneWidget);
      expect(find.byType(ListTile), findsNWidgets(2));
      expect(find.byType(Divider), findsOneWidget);
      expect(find.byIcon(Icons.chevron_right), findsOneWidget);
    });
  });

  group('AppSegmentedControl', () {
    Widget control(ValueChanged<int> onChanged) => AppSegmentedControl<int>(
      segments: const {1: Text('ft'), 2: Text('m')},
      selected: 1,
      onChanged: onChanged,
    );

    for (final (platform, type) in [
      (_ios, CupertinoSlidingSegmentedControl<int>),
      (_android, SegmentedButton<int>),
    ]) {
      testWidgets('is a $type on $platform, and changes', (tester) async {
        final changes = <int>[];
        await _pump(tester, platform, control(changes.add));
        expect(find.byType(type), findsOneWidget);
        await tester.tap(find.text('m'));
        await tester.pumpAndSettle();
        expect(changes, [2]);
      });
    }
  });

  group('showAppDialog', () {
    for (final (platform, cupertino) in [(_ios, true), (_android, false)]) {
      testWidgets('closes with the action tapped on $platform', (tester) async {
        Future<String?>? shown;
        await _pump(
          tester,
          platform,
          Builder(
            builder: (context) => TextButton(
              onPressed: () => shown = showAppDialog<String>(
                context: context,
                title: 'Delete?',
                message: 'It can’t be undone.',
                actions: const [
                  AppDialogAction(label: 'Cancel', value: 'cancel'),
                  AppDialogAction(
                    label: 'Delete',
                    value: 'delete',
                    isDestructive: true,
                  ),
                ],
              ),
              child: const Text('Open'),
            ),
          ),
        );
        await tester.tap(find.text('Open'));
        await tester.pumpAndSettle();
        expect(
          find.byType(CupertinoAlertDialog),
          cupertino ? findsOneWidget : findsNothing,
        );
        expect(find.text('It can’t be undone.'), findsOneWidget);
        await tester.tap(find.text('Delete'));
        await tester.pumpAndSettle();
        expect(await shown, 'delete');
      });
    }
  });

  group('GlassSurface', () {
    Widget surface() => const AppGlassLayer(
      child: GlassSurface(child: SizedBox.square(dimension: 48)),
    );

    testWidgets('is glass on iOS', (tester) async {
      await _pump(tester, _ios, surface());
      expect(find.byType(LiquidGlassLayer), findsOneWidget);
      expect(find.byType(LiquidGlass), findsOneWidget);
    });

    testWidgets('is an opaque surface on iOS with more contrast', (
      tester,
    ) async {
      await _pump(tester, _ios, surface(), highContrast: true);
      expect(find.byType(LiquidGlass), findsNothing);
      expect(find.byType(LiquidGlassLayer), findsNothing);
    });

    testWidgets('is an opaque surface on Android', (tester) async {
      await _pump(tester, _android, surface());
      expect(find.byType(LiquidGlass), findsNothing);
      expect(find.byType(LiquidGlassLayer), findsNothing);
    });
  });

  testWidgets('AppSymbols are SF-style on iOS and Material elsewhere', (
    tester,
  ) async {
    late AppSymbols ios, android;
    await _pump(
      tester,
      _ios,
      Builder(
        builder: (context) {
          ios = AppSymbols.of(context);
          return const SizedBox();
        },
      ),
    );
    await _pump(
      tester,
      _android,
      Builder(
        builder: (context) {
          android = AppSymbols.of(context);
          return const SizedBox();
        },
      ),
    );
    // The app animates from the iOS theme to this one.
    await tester.pumpAndSettle();
    expect(ios.close, CupertinoIcons.xmark);
    expect(android.close, Icons.close);
    expect(ios.swap, CupertinoIcons.arrow_up_arrow_down);
  });
}
