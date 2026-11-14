import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/maneuver_banner.dart';
import 'package:cardinal/navigation/maneuver_glyphs.dart';
import 'package:cardinal/ui/theme.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_navigation.dart';

final turnLeft = fakeContent(
  'Turn left onto Pike Street',
  type: ManeuverType.turn,
  modifier: ManeuverModifier.left,
);
final thenRight = fakeContent(
  'Turn right onto 4th Avenue',
  type: ManeuverType.turn,
  modifier: ManeuverModifier.right,
);

void main() {
  Future<void> show(
    WidgetTester tester,
    NavVisualInstruction? instruction, {
    double? distance = 180,
    UnitSystem units = UnitSystem.metric,
    TargetPlatform platform = TargetPlatform.android,
  }) => tester.pumpWidget(
    MaterialApp(
      theme: appTheme(Brightness.light, platform: platform),
      localizationsDelegates: AppLocalizations.localizationsDelegates,
      home: Scaffold(
        body: Align(
          alignment: Alignment.topCenter,
          child: ManeuverBanner(
            instruction: instruction,
            distance: distance,
            units: units,
          ),
        ),
      ),
    ),
  );

  Finder icon(String name) => find.byIcon(maneuverGlyphs[name]!);

  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    group(platform.name, () {
      testWidgets('shows the maneuver, its distance and its icon', (
        tester,
      ) async {
        await show(tester, fakeVisual(turnLeft), platform: platform);
        expect(find.text('200 m'), findsOneWidget);
        expect(find.text('Turn left onto Pike Street'), findsOneWidget);
        expect(icon('turn_left'), findsOneWidget);
        expect(find.text('Turn right onto 4th Avenue'), findsNothing);
      });

      testWidgets(
        'shows the maneuver after it underneath, without a distance',
        (tester) async {
          await show(
            tester,
            fakeVisual(turnLeft, secondary: thenRight),
            platform: platform,
          );
          expect(find.text('200 m'), findsOneWidget);
          expect(find.text('Turn left onto Pike Street'), findsOneWidget);
          expect(find.text('Turn right onto 4th Avenue'), findsOneWidget);
          expect(icon('turn_left'), findsOneWidget);
          expect(icon('turn_right'), findsOneWidget);
          // Under the first.
          expect(
            tester.getTopLeft(find.text('Turn right onto 4th Avenue')).dy,
            greaterThan(
              tester.getBottomLeft(find.text('Turn left onto Pike Street')).dy,
            ),
          );
        },
      );

      testWidgets('shows nothing without an instruction', (tester) async {
        await show(tester, null, platform: platform);
        expect(find.byType(Text), findsNothing);
        expect(find.byType(Icon), findsNothing);
        expect(tester.getSize(find.byType(ManeuverBanner)), Size.zero);
      });
    });
  }

  testWidgets('measures in the units given', (tester) async {
    await show(
      tester,
      fakeVisual(turnLeft),
      distance: 2414,
      units: UnitSystem.imperial,
    );
    expect(find.text('1.5 mi'), findsOneWidget);
  });

  testWidgets('leaves the distance out without one', (tester) async {
    await show(tester, fakeVisual(turnLeft), distance: null);
    expect(find.text('Turn left onto Pike Street'), findsOneWidget);
    expect(find.textContaining(' m'), findsNothing);
  });

  testWidgets('keeps the icon column for a maneuver without an icon', (
    tester,
  ) async {
    await show(
      tester,
      fakeVisual(
        fakeContent('Exit the roundabout', type: ManeuverType.exitRoundabout),
      ),
    );
    expect(find.byType(Icon), findsNothing);
    expect(
      tester.getTopLeft(find.text('Exit the roundabout')).dx -
          tester.getTopLeft(find.byType(ManeuverBanner)).dx,
      greaterThanOrEqualTo(ManeuverBanner.iconSize),
    );
  });

  testWidgets('cuts a long instruction off after two lines', (tester) async {
    await show(
      tester,
      fakeVisual(
        fakeContent(
          'Take the ramp on the right toward Interstate 5 South, Portland, '
          'Tacoma, Seattle-Tacoma International Airport and Everett',
          type: ManeuverType.offRamp,
          modifier: ManeuverModifier.right,
        ),
      ),
    );
    final text = tester.widget<Text>(find.textContaining('Take the ramp'));
    expect(text.maxLines, 2);
    expect(text.overflow, TextOverflow.ellipsis);
  });
}
