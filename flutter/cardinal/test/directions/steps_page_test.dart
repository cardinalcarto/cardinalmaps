import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/steps_page.dart';
import 'package:cardinal/directions/trip_buttons.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/navigation/navigation_controller.dart';
import 'package:cardinal/routes.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoButton;
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import 'picker_harness.dart';

void main() {
  late Picker picker;

  setUp(() => picker = Picker());

  /// Selects the route at [index], then opens its steps.
  Future<GoRouter> openSteps(
    WidgetTester tester, {
    int index = 0,
    TargetPlatform? platform,
  }) async {
    final router = await picker.open(tester, platform: platform);
    await tester.tap(rows.at(index));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Steps'));
    await tester.pumpAndSettle();
    expect(router.state.uri.toString(), stepsLocation(destination.id));
    return router;
  }

  /// The steps page's Start.
  Finder start() => find.descendant(
    of: find.byType(StepsPage),
    matching: find.byType(TripButtons),
  );

  testWidgets('says how long the route takes and its name on one line, then '
      'how far it goes and when it arrives', (tester) async {
    await openSteps(tester);
    expect(find.text('22 min via I-5 South'), findsOneWidget);
    expect(find.text('14 mi · Arrive 3:42 PM'), findsOneWidget);
    final header = tester.widget<Text>(find.text('22 min via I-5 South'));
    expect(header.maxLines, 2);
    expect(header.overflow, TextOverflow.ellipsis);
  });

  testWidgets('lists each step, numbered, with how far it goes, its '
      'instruction never cut short', (tester) async {
    await openSteps(tester);
    expect(find.text('1'), findsOneWidget);
    expect(find.text('2'), findsOneWidget);
    expect(find.text('Drive south.'), findsOneWidget);
    expect(find.text('Take I-5.'), findsOneWidget);
    expect(find.text('0.4 mi'), findsOneWidget);
    expect(find.text('14 mi'), findsOneWidget);
    for (final instruction in ['Drive south.', 'Take I-5.']) {
      final text = tester.widget<Text>(find.text(instruction));
      expect(text.maxLines, isNull);
      expect(text.overflow, isNull);
    }
  });

  testWidgets('Start navigates the selected route, as the directions page\'s '
      'does', (tester) async {
    final router = await openSteps(tester, index: 1);
    expect(start(), findsOneWidget);
    expect(
      find.descendant(of: start(), matching: find.text('Steps')),
      findsNothing,
    );
    await tester.tap(
      find.descendant(of: start(), matching: find.text('Start')),
    );
    await tester.pumpAndSettle();
    expect(router.state.uri.toString(), navigationLocation(destination.id));
    expect(picker.navigation.state, NavigationState.acquiring);
    expect(picker.navigation.route, same(viaToll));
    expect(picker.navigation.mode, TravelMode.driving);
  });

  testWidgets('Start is a filled capsule on iOS', (tester) async {
    await openSteps(tester, platform: TargetPlatform.iOS);
    final button = tester.widget<CupertinoButton>(
      find.descendant(of: start(), matching: find.byType(CupertinoButton)),
    );
    expect(button.onPressed, isNotNull);
    expect(button.borderRadius, BorderRadius.circular(100));
  });

  testWidgets('follows the selection while it shows', (tester) async {
    await openSteps(tester);
    routeTaps.add(2);
    await tester.pumpAndSettle();
    expect(find.text('34 min via East Marginal Way South'), findsOneWidget);
    expect(find.text('13 mi · Arrive 3:54 PM'), findsOneWidget);
    // This one has no steps.
    expect(find.text('Take I-5.'), findsNothing);
  });

  testWidgets('the system\'s back returns to the sheet where it was', (
    tester,
  ) async {
    final router = await picker.open(tester);
    final sheet = sheetOf(tester);
    await tester.drag(tabs, const Offset(0, -90));
    await tester.pumpAndSettle();
    expect(sheet.restingIn, isNotNull);
    final extent = sheet.extent;

    await tester.tap(find.text('Steps'));
    await tester.pumpAndSettle();
    expect(sheet.restingAt, const SheetAnchor.fraction(0.4));
    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(router.state.uri.toString(), directionsLocation(destination.id));
    expect(sheet.extent, moreOrLessEquals(extent));
  });
}
