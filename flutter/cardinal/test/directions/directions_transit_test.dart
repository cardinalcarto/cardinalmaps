import 'package:cardinal/directions/connections.dart';
import 'package:cardinal/directions/directions_page.dart';
import 'package:cardinal/directions/journey_list.dart';
import 'package:cardinal/directions/mode_tabs.dart';
import 'package:cardinal/directions/option_row.dart';
import 'package:cardinal/directions/route_source.dart';
import 'package:cardinal/directions/transit_details.dart';
import 'package:cardinal/directions/transit_plan.dart';
import 'package:cardinal/directions/transit_timeline.dart';
import 'package:cardinal/directions/trip_details_page.dart';
import 'package:cardinal/map/route_layer.dart';
import 'package:cardinal/settings/accessibility.dart';
import 'package:cardinal/sheet/sheet.dart';
import 'package:cardinal/store/store.dart';
import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoButton,
        CupertinoIcons,
        CupertinoListTile,
        CupertinoMenuAnchor,
        CupertinoSearchTextField;
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

import '../fakes/fake_app.dart';
import '../fakes/fake_transit.dart';
import 'picker_harness.dart';

/// A plan of one journey for each of [tags], which tell their trips apart, each
/// leaving [shifts] seconds after the fake plan's first would, or ten minutes after
/// the one before.
TransitPlan planOf(List<String> tags, {List<int>? shifts}) =>
    fakePlan(journeys: tags.length, tags: tags, shifts: shifts);

void main() {
  late Picker picker;

  setUp(() => picker = Picker());

  Finder journeys() => find.byType(JourneyRow);

  /// The tag of each row's journey, in the list's order: what [planOf] ended its
  /// trips' IDs with.
  List<String> shown(WidgetTester tester) => [
    for (final row in tester.widgetList<JourneyRow>(journeys()))
      row.journey.rides.first.tripId!.substring(3),
  ];

  List<bool> selected(WidgetTester tester) => [
    for (final row in tester.widgetList<JourneyRow>(journeys())) row.selected,
  ];

  /// Matches rects at [rect], give or take what adding the same heights up in
  /// another order comes to.
  Matcher rectNear(Rect rect) => predicate<Rect>(
    (actual) =>
        (actual.left - rect.left).abs() < 1e-6 &&
        (actual.top - rect.top).abs() < 1e-6 &&
        (actual.right - rect.right).abs() < 1e-6 &&
        (actual.bottom - rect.bottom).abs() < 1e-6,
    'within a millionth of a pixel of $rect',
  );

  /// How much of [finder]'s widget shows through everything that fades it.
  double opacityOf(WidgetTester tester, Finder finder) => tester
      .widgetList<FadeTransition>(
        find.ancestor(of: finder, matching: find.byType(FadeTransition)),
      )
      .fold(1, (shown, fade) => shown * fade.opacity.value);

  /// The rides the map draws.
  List<Map<String, Object?>> drawnRides() => [
    for (final feature
        in ((picker.map.geoJson[transitSource]! as Map)['features'] as List)
            .cast<Map<String, Object?>>())
      if ((feature['properties']! as Map)['role'] == 'ride') feature,
  ];

  /// Opens directions on transit's journeys, as [plan] has them.
  Future<void> openTransit(
    WidgetTester tester, {
    TransitPlan? plan,
    TargetPlatform? platform,
  }) async {
    if (plan != null) picker.transit.immediately = plan;
    await picker.open(tester, platform: platform);
    await showMode(tester, TravelMode.transit);
  }

  /// Lets the time to plan transit again pass, and returns the question asked.
  Future<TransitQuestion> refreshDue(WidgetTester tester) async {
    picker.transit.immediately = null;
    final asked = picker.transit.asked.length;
    await tester.pump(transitRefresh);
    expect(picker.transit.asked, hasLength(asked + 1));
    return picker.transit.asked.last;
  }

  /// Has an answer just given land, and what it changes built: it starts whatever
  /// it animates.
  Future<void> land(WidgetTester tester) async {
    await tester.pump();
    await tester.pump();
  }

  Finder detailsButton() => find.descendant(
    of: footer,
    matching: find.byWidgetPredicate(
      (widget) => widget is FilledButton || widget is CupertinoButton,
    ),
  );

  Future<void> openDetails(WidgetTester tester) async {
    await tester.tap(
      find.descendant(of: footer, matching: find.text('Trip details')),
    );
    await tester.pumpAndSettle();
    expect(find.byType(TripDetailsPage), findsOneWidget);
  }

  ConnectionChain openChain(WidgetTester tester) => tester
      .widget<JourneyHeader>(find.byType(JourneyHeader, skipOffstage: false))
      .chain;

  group('the transit tab', () {
    testWidgets('lists journeys as rows of the height every row has, the '
        'first selected and held, under its line, over its one button', (
      tester,
    ) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await moveTo(tester, const SheetAnchor.full());
      expect(shown(tester), ['-x', '-y']);
      expect(selected(tester), [true, false]);
      final height = OptionRowMetrics.heightOf(tester.element(tabs));
      for (final slot in slots.evaluate()) {
        expect(slot.size!.height, height);
      }
      expect(find.text('Live times from 1 of 2 feeds'), findsOneWidget);
      expect(find.text('Walking to stops'), findsOneWidget);
      expect(find.byType(TimelineTicks), findsOneWidget);
      expect(
        find.descendant(of: footer, matching: find.text('Trip details')),
        findsOneWidget,
      );
      expect(find.text('Start'), findsNothing);
      expect(find.text('Steps'), findsNothing);
      // The street ways' legend of road kinds isn't transit's.
      expect(find.byType(WayLegend), findsNothing);
      // The first is held: at the lowest anchor it's what shows, over the button.
      await moveTo(tester, footerAnchor);
      expect(
        tester.getRect(journeys().first).bottom,
        moreOrLessEquals(footerTop(tester)),
      );
    });

    testWidgets('a journey tapped is selected and held, and nothing opens', (
      tester,
    ) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await moveTo(tester, const SheetAnchor.full());
      final before = [
        for (var i = 0; i < 2; i++) tester.getRect(journeys().at(i)),
      ];
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      expect(selected(tester), [false, true]);
      expect(find.byType(TripDetailsPage), findsNothing);
      // Selecting moved neither row.
      expect([
        for (var i = 0; i < 2; i++) tester.getRect(journeys().at(i)),
      ], before);
      await moveTo(tester, footerAnchor);
      expect(
        tester.getRect(journeys().at(1)).bottom,
        moreOrLessEquals(footerTop(tester)),
      );
    });

    testWidgets('the map draws the selected journey alone', (tester) async {
      // The second leaves from further along, so its lines are its own.
      final plan = TransitPlan.fromJson({
        ...fakePlanJson(),
        'journeys': [
          ...fakePlanJson(tags: ['-x'])['journeys']! as List<Object?>,
          ...connectionsPlanJson()['journeys']! as List<Object?>,
        ],
      });
      await openTransit(tester, plan: plan);
      expect(drawnRides(), hasLength(2), reason: 'the first journey\'s two');
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      expect(drawnRides(), hasLength(4), reason: 'the second journey\'s four');
      await tester.tap(journeys().at(0));
      await tester.pumpAndSettle();
      expect(drawnRides(), hasLength(2));
    });

    testWidgets('Trip details opens the selected journey', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      await openDetails(tester);
      expect(openChain(tester).journey.rides.first.tripId, 't40-y');
      expect(drawnRides(), hasLength(2));
    });

    testWidgets('Trip details is disabled while transit loads, and with no '
        'journeys', (tester) async {
      picker.transit.immediately = null;
      await picker.open(tester);
      await showMode(tester, TravelMode.transit);
      bool enabled() => switch (tester.widget(detailsButton())) {
        FilledButton(:final onPressed) => onPressed != null,
        CupertinoButton(:final onPressed) => onPressed != null,
        _ => throw StateError('no button'),
      };
      expect(find.byType(OptionPlaceholder), findsNWidgets(3));
      expect(enabled(), isFalse);
      // How stops are reached can be changed while it loads.
      expect(find.text('Walking to stops'), findsOneWidget);

      picker.transit.asked.single.answer.complete(emptyPlan('no_egress'));
      await tester.pumpAndSettle();
      expect(
        find.text('No stops within walking range of the destination'),
        findsOneWidget,
      );
      expect(find.byType(OptionMessage), findsOneWidget);
      expect(enabled(), isFalse);
      expect(find.text('Walking to stops'), findsOneWidget);
    });

    testWidgets('its journeys fade in where its placeholders were', (
      tester,
    ) async {
      picker.transit.immediately = null;
      await picker.open(tester);
      await showMode(tester, TravelMode.transit);
      await moveTo(tester, const SheetAnchor.full());
      final placeholder = tester.getRect(find.byType(OptionPlaceholder).first);
      final sheet = sheetOf(tester).extent;
      picker.transit.asked.single.answer.complete(planOf(['-x', '-y']));
      await land(tester);
      await tester.pump(const Duration(milliseconds: 60));
      expect(opacityOf(tester, journeys().first), inExclusiveRange(0, 1));
      expect(tester.getRect(journeys().first), placeholder);
      await tester.pumpAndSettle();
      expect(find.byType(OptionPlaceholder), findsNothing);
      expect(tester.getRect(journeys().first), placeholder);
      expect(sheetOf(tester).extent, sheet);
    });

    for (final scale in [1.0, 1.5, 2.0]) {
      testWidgets('rests where every street way does, loading, with journeys '
          'and with none, at $scale×', (tester) async {
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        final report = FlutterError.onError;
        FlutterError.onError = (details) {
          // The search sheet under directions overflows at the largest sizes.
          if (details.toString().contains('lib/search/search_sheet.dart')) {
            return;
          }
          report?.call(details);
        };
        addTearDown(() => FlutterError.onError = report);
        picker.answerNothing();
        await picker.open(tester);
        final sheet = sheetOf(tester);
        List<double> anchors() => [
          extentOf(sheet, footerAnchor),
          extentOf(sheet, headerAnchor),
        ];
        Matcher same(List<double> extents) => pairwiseCompare<double, double>(
          extents,
          (expected, actual) => (expected - actual).abs() < 1e-9,
          'the same as',
        );
        final rests = anchors();
        final buttons = tester.getSize(footer);
        Future<void> check(TravelMode mode, String state) async {
          await showMode(tester, mode);
          expect(anchors(), same(rests), reason: '$mode, $state');
          expect(tester.getSize(footer), buttons, reason: '$mode, $state');
          expect(sheet.anchors, hasLength(2));
        }

        for (final mode in TravelMode.values) {
          await check(mode, 'loading');
        }
        picker.asked(TravelMode.driving).single.answer.complete(drivingRoutes);
        picker.asked(TravelMode.walking).single.answer.complete(walkingRoutes);
        picker.asked(TravelMode.cycling).single.answer.complete(cyclingRoutes);
        picker.transit.asked.single.answer.complete(planOf(['-x', '-y']));
        await tester.pumpAndSettle();
        for (final mode in TravelMode.values) {
          await check(mode, 'with options');
        }
        // And with no journeys, asked by wheelchair.
        await picker.store.settings.set(wheelchairByDefault, true);
        await tester.pumpAndSettle();
        await check(TravelMode.transit, 'loading again');
        picker.transit.asked.last.answer.complete(emptyPlan('no_service'));
        await tester.pumpAndSettle();
        await check(TravelMode.transit, 'with no journeys');
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('a street way and transit change places at the header anchor '
        'with nothing moving', (tester) async {
      await picker.open(tester);
      await moveTo(tester, headerAnchor);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final header = tester.getRect(tabs);
      final buttons = tester.getRect(footer);
      // Where the held row is painted: under the tabs, over the buttons.
      final held = tester.getRect(rows.first);
      expect(held.bottom, moreOrLessEquals(footerTop(tester)));

      void still(String when) {
        expect(sheet.extent, moreOrLessEquals(extent), reason: when);
        expect(tester.getRect(tabs), header, reason: when);
        for (final buttons_ in footer.evaluate()) {
          final box = buttons_.renderObject! as RenderBox;
          expect(
            box.localToGlobal(Offset.zero) & box.size,
            buttons,
            reason: when,
          );
        }
      }

      await tester.tap(
        find.descendant(
          of: tabs,
          matching: find.byIcon(TravelMode.transit.icon),
        ),
      );
      await tester.pump();
      still('as transit is tapped');
      await tester.pump(const Duration(milliseconds: 60));
      still('part way to transit');
      // Both ways' buttons, one fading into the other in the same place.
      expect(footer, findsNWidgets(2));
      await tester.pumpAndSettle();
      still('on transit');
      expect(footer, findsOneWidget);
      expect(tester.getRect(journeys().first), held);

      await tester.tap(
        find.descendant(
          of: tabs,
          matching: find.byIcon(TravelMode.driving.icon),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      still('part way back');
      await tester.pumpAndSettle();
      still('driving again');
      expect(tester.getRect(rows.first), held);
    });

    testWidgets('in the free range, one way\'s rows fade into the other\'s '
        'where each is', (tester) async {
      await picker.open(tester);
      await moveTo(tester, const SheetAnchor.full());
      final street = tester.getRect(rows.first);
      await tester.tap(
        find.descendant(
          of: tabs,
          matching: find.byIcon(TravelMode.transit.icon),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      // Both lists, each in its own place: transit's start under its line.
      expect(opacityOf(tester, rows.first), inExclusiveRange(0, 1));
      expect(opacityOf(tester, journeys().first), inExclusiveRange(0, 1));
      expect(tester.getRect(rows.first), street);
      final transit = tester.getRect(journeys().first);
      expect(transit.top, greaterThan(street.top));
      await tester.pumpAndSettle();
      expect(rows, findsNothing);
      expect(tester.getRect(journeys().first), transit);
    });

    testWidgets('opened with transit the way last chosen, it never shows as '
        'another way', (tester) async {
      await picker.store.settings.set(lastTravelMode, TravelMode.transit);
      await picker.open(tester, settle: false);
      final sheet = sheetOf(tester);
      var showed = false;
      for (var frame = 0; frame < 60; frame++) {
        if (opacityOf(tester, tabs) > 0) {
          showed = true;
          expect(
            tester.widget<ModeTabs>(tabs).selected,
            TravelMode.transit,
            reason: 'frame $frame',
          );
          expect(find.text('Start'), findsNothing, reason: 'frame $frame');
          expect(find.text('Trip details'), findsOneWidget);
          expect(find.text('Walking to stops'), findsOneWidget);
        }
        await tester.pump(const Duration(milliseconds: 16));
      }
      expect(showed, isTrue);
      await tester.pumpAndSettle();
      expect(shown(tester), ['', '']);
      // Where a street way opens: within the free range, at most 60% up.
      expect(sheet.restingIn, isNotNull);
      expect(sheet.extent, moreOrLessEquals(appScreen.height * 0.6));
    });

    testWidgets('how stops are reached is chosen from a menu, and asked '
        'with', (tester) async {
      await openTransit(tester);
      await tester.tap(find.text('Walking to stops'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Bike').last);
      await tester.pumpAndSettle();
      expect(picker.transit.asked.last.access, TransitAccess.cycling);
      expect(find.text('Cycling to stops'), findsOneWidget);
    });

    testWidgets('on iOS its rows are tinted cards, its menu the platform\'s, '
        'its button a capsule', (tester) async {
      await openTransit(tester, platform: TargetPlatform.iOS);
      await moveTo(tester, const SheetAnchor.full());
      Color fill(int row) =>
          (tester
                      .widget<AnimatedContainer>(
                        find.descendant(
                          of: journeys().at(row),
                          matching: find.byType(AnimatedContainer),
                        ),
                      )
                      .decoration!
                  as BoxDecoration)
              .color!;
      expect(fill(0).a, inExclusiveRange(0, 1));
      expect(fill(1).a, inExclusiveRange(0, 1));
      expect(fill(0), isNot(fill(1)));
      // A gap between the cards.
      expect(
        tester.getRect(journeys().at(1)).top,
        greaterThanOrEqualTo(tester.getRect(journeys().at(0)).bottom),
      );
      expect(find.byType(CupertinoMenuAnchor), findsOneWidget);
      expect(tester.widget(detailsButton()), isA<CupertinoButton>());
      await tester.tap(find.text('Walking to stops'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Wheelchair').last);
      await tester.pumpAndSettle();
      expect(picker.transit.asked.last.access, TransitAccess.wheelchair);
    });
  });

  group('editing an end', () {
    /// Pumps a frame at a time through the fade between the trip and the search,
    /// checking at each that [outgoing] fades out by as much as [incoming] fades
    /// in, that each stays where it was, and that nothing of [outgoing] can be
    /// tapped; until [incoming] shows in full.
    Future<void> crossFade(
      WidgetTester tester, {
      required Finder outgoing,
      required Finder incoming,
      required Map<Finder, Rect> staying,
    }) async {
      var shown = opacityOf(tester, incoming);
      var frames = 0;
      while (shown < 1) {
        expect(frames++, lessThan(30), reason: 'the fade ends');
        await tester.pump(const Duration(milliseconds: 16));
        // Gone once it's faded out.
        final out = outgoing.evaluate().isEmpty
            ? 0.0
            : opacityOf(tester, outgoing);
        final into = opacityOf(tester, incoming);
        expect(out + into, moreOrLessEquals(1), reason: 'frame $frames');
        expect(into, greaterThanOrEqualTo(shown), reason: 'frame $frames');
        shown = into;
        for (final MapEntry(key: finder, value: rect) in staying.entries) {
          if (finder.evaluate().isEmpty) continue;
          expect(
            tester.getRect(finder).shift(Offset(0, -contentTop(tester))),
            rectNear(rect),
            reason: '$finder, frame $frames',
          );
        }
        if (out > 0) {
          final box = tester.renderObject(outgoing);
          final at = tester.getCenter(outgoing);
          expect(
            tester
                .hitTestOnBinding(at)
                .path
                .any((entry) => entry.target == box),
            isFalse,
            reason: 'a tap on what fades out, frame $frames',
          );
        }
      }
    }

    for (final (platform, mode) in [
      for (final platform in [TargetPlatform.android, TargetPlatform.iOS])
        for (final mode in TravelMode.values) (platform, mode),
    ]) {
      testWidgets('the trip and the search cross-fade, each where it is, the '
          'held row with the trip: going in, and back: ${mode.name} on '
          '${platform.name}', (tester) async {
        final cupertino = platform == TargetPlatform.iOS;
        await picker.open(tester, platform: platform);
        await showMode(tester, mode);
        await moveTo(tester, headerAnchor);
        final row = mode == TravelMode.transit ? journeys() : rows;
        final sheet = sheetOf(tester);
        final card = tester.getRect(find.text('Museum of Flight'));
        Finder inCard() => find
            .descendant(
              of: find.byType(DirectionsPage),
              matching: find.text('Museum of Flight', findRichText: false),
            )
            .last;
        final field = find.byType(
          cupertino ? CupertinoSearchTextField : TextField,
        );
        final input = find.descendant(
          of: field,
          matching: find.byType(EditableText),
        );
        await tester.tap(
          find
              .ancestor(
                of: find.text('To'),
                matching: find.byType(cupertino ? CupertinoListTile : InkWell),
              )
              .first,
        );
        await tester.pump();
        // The trip is all still there, where it was, with the search under it.
        expect(tester.getRect(inCard()), card);
        expect(field, findsOneWidget);
        expect(tester.testTextInput.isVisible, isTrue);
        await tester.pump(const Duration(milliseconds: 60));
        expect(opacityOf(tester, tabs), inExclusiveRange(0, 1));
        expect(opacityOf(tester, row.first), inExclusiveRange(0, 1));
        expect(opacityOf(tester, footer), inExclusiveRange(0, 1));
        // The held row is still held, over the buttons.
        expect(
          tester.getRect(row.first).bottom,
          lessThanOrEqualTo(footerTop(tester) + 0.5),
        );
        await crossFade(tester, outgoing: tabs, incoming: field, staying: {});
        await tester.pumpAndSettle();
        expect(row, findsNothing);
        expect(tabs, findsNothing);
        expect(footer, findsNothing);
        expect(opacityOf(tester, field), 1);
        expect(sheet.isAtMax, isTrue);

        // And back: the search fades out where it is, in the sheet as it lowers
        // to the header anchor, as the trip fades in; the keyboard goes at once.
        final search = tester
            .getRect(field)
            .shift(Offset(0, -contentTop(tester)));
        final choice = cupertino
            ? find.byIcon(CupertinoIcons.location_fill)
            : find.text('Use current location');
        final choiceAt = tester
            .getRect(choice)
            .shift(Offset(0, -contentTop(tester)));
        await tester.binding.handlePopRoute();
        await tester.pump();
        expect(field, findsOneWidget);
        expect(tester.testTextInput.isVisible, isFalse);
        expect(tester.widget<EditableText>(input).focusNode.hasFocus, isFalse);
        expect(opacityOf(tester, field), 1);
        expect(
          tester.getRect(field).shift(Offset(0, -contentTop(tester))),
          rectNear(search),
        );
        expect(opacityOf(tester, tabs), 0);
        await crossFade(
          tester,
          outgoing: field,
          incoming: tabs,
          staying: {field: search, choice: choiceAt},
        );
        expect(field, findsNothing);
        expect(choice, findsNothing);
        await tester.pumpAndSettle();
        expect(sheet.restingAt, headerAnchor);
        expect(opacityOf(tester, row.first), 1);
        expect(
          tester.getRect(row.first).bottom,
          moreOrLessEquals(footerTop(tester)),
        );
        expect(tester.testTextInput.isVisible, isFalse);
      });
    }

    testWidgets('a choice tapped fades out with the search, where it was, and '
        "can't be tapped again", (tester) async {
      await picker.open(tester);
      await tester.tap(find.widgetWithText(InkWell, 'From'));
      await tester.pumpAndSettle();
      final choice = find.text('Use current location');
      final at = tester.getRect(choice).shift(Offset(0, -contentTop(tester)));
      await tester.tap(choice);
      await tester.pump();
      await tester.pump();
      expect(find.byType(TextField), findsOneWidget);
      expect(tabs, findsOneWidget);
      expect(tester.testTextInput.isVisible, isFalse);
      await crossFade(
        tester,
        outgoing: choice,
        incoming: tabs,
        staying: {choice: at},
      );
      await tester.pumpAndSettle();
      expect(choice, findsNothing);
      expect(find.text('My Location'), findsOneWidget);
      expect(sheetOf(tester).restingAt, headerAnchor);
    });

    testWidgets('editing again as the search fades out fades it back in, with '
        'the keyboard', (tester) async {
      await picker.open(tester);
      await tester.tap(find.widgetWithText(InkWell, 'To'));
      await tester.pumpAndSettle();
      await tester.binding.handlePopRoute();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      final field = find.byType(TextField);
      final fading = opacityOf(tester, field);
      expect(fading, inExclusiveRange(0, 1));
      tester.widget<InkWell>(find.widgetWithText(InkWell, 'From')).onTap!();
      await tester.pump();
      expect(field, findsOneWidget);
      expect(opacityOf(tester, field), moreOrLessEquals(fading));
      expect(tester.widget<TextField>(field).focusNode!.hasFocus, isTrue);
      expect(tester.testTextInput.isVisible, isTrue);
      await tester.pumpAndSettle();
      expect(opacityOf(tester, field), 1);
      expect(tabs, findsNothing);
      expect(
        SheetScaffold.controllerOf(tester.element(find.byType(DirectionsPage)))
            .isAtMax,
        isTrue,
      );
    });

    testWidgets('fades the search out where it was scrolled to, over the trip '
        'where it comes back to, though that\'s shorter', (tester) async {
      for (var i = 0; i < 20; i++) {
        final place = Place(
          id: 'poi:${100 + i}',
          kind: 'poi',
          name: 'Saved $i',
          center: LngLat(2, 2 + i / 100),
        );
        await picker.store.places.put(place);
        await picker.store.saved.savePlace(place.id);
      }
      picker.transit.immediately = fakePlan(journeys: 10);
      await picker.open(tester);
      await showMode(tester, TravelMode.transit);
      await moveTo(tester, const SheetAnchor.full());
      await tester.drag(journeys().at(2), const Offset(0, -400));
      await tester.pumpAndSettle();
      double scrolled() => tester
          .state<ScrollableState>(
            find.ancestor(of: tabs, matching: find.byType(Scrollable)).first,
          )
          .position
          .pixels;
      final left = scrolled();
      tester.widget<InkWell>(find.widgetWithText(InkWell, 'To')).onTap!();
      await tester.pumpAndSettle();
      await tester.drag(find.text('Saved 17'), const Offset(0, -300));
      await tester.pumpAndSettle();
      // A choice, not the end it becomes.
      final choice = find.widgetWithText(ListTile, 'Saved 8');
      final at = tester.getRect(choice).shift(Offset(0, -contentTop(tester)));
      // The new trip has placeholders while it's planned: far shorter.
      picker.transit.immediately = null;
      await tester.tap(choice);
      await tester.pump();
      await tester.pump();
      expect(find.byType(OptionPlaceholder), findsWidgets);
      expect(scrolled(), lessThan(left));
      await crossFade(
        tester,
        outgoing: choice,
        incoming: tabs,
        staying: {choice: at},
      );
      await tester.pumpAndSettle();
      expect(choice, findsNothing);
    });

    testWidgets('fades the trip out where it was scrolled to', (tester) async {
      picker.transit.immediately = fakePlan(journeys: 10);
      await picker.open(tester);
      await showMode(tester, TravelMode.transit);
      await moveTo(tester, const SheetAnchor.full());
      await tester.drag(journeys().at(2), const Offset(0, -150));
      await tester.pumpAndSettle();
      final third = tester.getRect(journeys().at(2));
      final field = find.widgetWithText(InkWell, 'To');
      // The ends are scrolled out of reach of a tap: as the card's row would.
      tester.widget<InkWell>(field).onTap!();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      expect(tester.getRect(journeys().at(2)), third);
    });
  });

  group('transit plans itself again', () {
    testWidgets('every half minute while its journeys show, the plan on '
        'screen staying until the answer', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await moveTo(tester, const SheetAnchor.full());
      final first = tester.element(journeys().first);
      final question = await refreshDue(tester);
      expect(question.access, TransitAccess.walking);
      await tester.pump(const Duration(seconds: 5));
      // Nothing says it's waiting.
      expect(shown(tester), ['-x', '-y']);
      expect(find.byType(OptionPlaceholder), findsNothing);
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(tabTime(tester, TravelMode.transit), isNotNull);

      // The same journeys, three minutes later: each row is the row it was.
      question.answer.complete(planOf(['-x', '-y'], shifts: [180, 780]));
      await land(tester);
      expect(shown(tester), ['-x', '-y']);
      expect(tester.element(journeys().first), same(first));
      expect(
        tester.widget<JourneyRow>(journeys().first).journey.departure,
        DateTime.fromMillisecondsSinceEpoch((fakeStart + 180) * 1000),
      );
      await tester.pumpAndSettle();
      // And again, half a minute after that answer.
      await refreshDue(tester);
    });

    testWidgets('a journey\'s times ease to the new ones in its timeline, '
        'and along the axis', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await moveTo(tester, const SheetAnchor.full());
      JourneyTimelineState timeline() => tester.state(
        find.descendant(
          of: journeys().at(1),
          matching: find.byType(JourneyTimeline),
        ),
      );
      double rideLeft() => timeline().drawn
          .firstWhere((piece) => piece.piece.kind == TimelineKind.ride)
          .left;
      double tickLeft() =>
          tester.getRect(find.byType(TimelineTicks)).left +
          tester
              .getRect(
                find
                    .descendant(
                      of: find.byType(TimelineTicks),
                      matching: find.byType(Text),
                    )
                    .first,
              )
              .center
              .dx;
      final before = rideLeft();
      final tick = tickLeft();
      final question = await refreshDue(tester);
      // The first leaves five minutes later, so the axis starts later, and the
      // second sits further left on it.
      question.answer.complete(planOf(['-x', '-y'], shifts: [300, 600]));
      await land(tester);
      await tester.pump(const Duration(milliseconds: 80));
      final during = rideLeft();
      final tickDuring = tickLeft();
      await tester.pumpAndSettle();
      final after = rideLeft();
      expect(after, lessThan(before));
      expect(during, inExclusiveRange(after, before));
      expect(tickLeft(), lessThan(tick));
      expect(tickDuring, inExclusiveRange(tickLeft(), tick));
    });

    testWidgets('a refresh that fails leaves the plan, and the next asks '
        'again', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await moveTo(tester, const SheetAnchor.full());
      final question = await refreshDue(tester);
      question.answer.completeError(const TransitPlanError('timed_out', ''));
      await tester.pumpAndSettle();
      expect(shown(tester), ['-x', '-y']);
      expect(find.byType(OptionMessage), findsNothing);
      expect(find.text('Live times from 1 of 2 feeds'), findsOneWidget);
      await refreshDue(tester);
    });

    testWidgets('not while another way shows, an end is edited, or the app '
        'is away; and at once when it shows again overdue', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      picker.transit.immediately = null;
      await showMode(tester, TravelMode.driving);
      await tester.pump(transitRefresh * 2);
      expect(picker.transit.asked, hasLength(1));

      // Back on transit with the plan as old as that: asked at once.
      picker.now = picker.now.add(transitRefresh * 2);
      await showMode(tester, TravelMode.transit);
      await tester.pump();
      expect(picker.transit.asked, hasLength(2));
      picker.transit.asked.last.answer.complete(planOf(['-x', '-y']));
      await tester.pumpAndSettle();

      for (final state in [
        AppLifecycleState.inactive,
        AppLifecycleState.hidden,
      ]) {
        tester.binding.handleAppLifecycleStateChanged(state);
      }
      await tester.pump(transitRefresh * 2);
      expect(picker.transit.asked, hasLength(2));
      for (final state in [
        AppLifecycleState.inactive,
        AppLifecycleState.resumed,
      ]) {
        tester.binding.handleAppLifecycleStateChanged(state);
      }
      await tester.pump(transitRefresh);
      expect(picker.transit.asked, hasLength(3));
      picker.transit.asked.last.answer.complete(planOf(['-x', '-y']));
      await tester.pumpAndSettle();

      await moveTo(tester, headerAnchor);
      await tester.tap(find.widgetWithText(InkWell, 'To'));
      await tester.pumpAndSettle();
      await tester.pump(transitRefresh * 2);
      expect(picker.transit.asked, hasLength(3));
    });

    testWidgets('the selected journey stays selected, by its trips, wherever '
        'the new plan has it', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y', '-z']));
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      final question = await refreshDue(tester);
      // The first has gone, and another comes after.
      question.answer.complete(
        planOf(['-y', '-z', '-w'], shifts: [600, 1200, 1800]),
      );
      await tester.pumpAndSettle();
      expect(shown(tester), ['-y', '-z', '-w']);
      expect(selected(tester), [true, false, false]);
      await moveTo(tester, footerAnchor);
      expect(
        tester.getRect(journeys().first).bottom,
        moreOrLessEquals(footerTop(tester)),
      );
    });

    testWidgets('with the selected journey gone from the new plan, its first '
        'is selected', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y', '-z']));
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      final question = await refreshDue(tester);
      question.answer.complete(planOf(['-x', '-z'], shifts: [0, 1200]));
      await tester.pumpAndSettle();
      expect(shown(tester), ['-x', '-z']);
      expect(selected(tester), [true, false]);
    });

    testWidgets('rows a new plan drops shrink away and those it adds grow '
        'in, the rest sliding, each still its own row', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y', '-z']));
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      final height = tester.getSize(journeys().first).height;
      final kept = tester.element(journeys().at(1));
      final top = tester.getRect(journeys().at(1)).top;
      final question = await refreshDue(tester);
      question.answer.complete(
        planOf(['-y', '-z', '-w'], shifts: [600, 1200, 1800]),
      );
      await land(tester);
      await tester.pump(const Duration(milliseconds: 80));
      // The one going is still there, part gone; the one coming part there.
      expect(shown(tester), ['-x', '-y', '-z', '-w']);
      double room(int row) => tester
          .getSize(
            find
                .ancestor(
                  of: journeys().at(row),
                  matching: find.byType(SizeTransition),
                )
                .first,
          )
          .height;
      expect(room(0), inExclusiveRange(0, height));
      expect(room(3), inExclusiveRange(0, height));
      expect(room(1), height);
      expect(tester.element(journeys().at(1)), same(kept));
      expect(
        tester.getRect(journeys().at(1)).top,
        inExclusiveRange(top - height, top),
      );
      await tester.pumpAndSettle();
      expect(shown(tester), ['-y', '-z', '-w']);
      expect(tester.element(journeys().first), same(kept));
      expect(tester.getRect(journeys().first).top, top - height);
    });

    testWidgets('rows a new plan puts in another order go and come, the '
        'selected one staying its own row', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y', '-z']));
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      final kept = tester.element(journeys().at(1));
      final question = await refreshDue(tester);
      question.answer.complete(
        planOf(['-y', '-x', '-z'], shifts: [0, 600, 1200]),
      );
      await land(tester);
      await tester.pump(const Duration(milliseconds: 80));
      // The first is on its way out where it was, and in where it is now.
      expect(shown(tester), ['-x', '-y', '-x', '-z']);
      expect(selected(tester), [false, true, false, false]);
      expect(tester.element(journeys().at(1)), same(kept));
      expect(tester.takeException(), isNull);
      await tester.pumpAndSettle();
      expect(shown(tester), ['-y', '-x', '-z']);
      expect(selected(tester), [true, false, false]);
      expect(tester.element(journeys().first), same(kept));
    });

    testWidgets('no row is added, dropped or moved while a finger is down, '
        'or for a moment after', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y', '-z']));
      await moveTo(tester, const SheetAnchor.full());
      final question = await refreshDue(tester);
      final rects = [
        for (var i = 0; i < 3; i++) tester.getRect(journeys().at(i)),
      ];
      final finger = await tester.startGesture(
        tester.getCenter(journeys().at(1)),
      );
      await tester.pump();
      question.answer.complete(
        planOf(['-y', '-z', '-w'], shifts: [600, 1200, 1800]),
      );
      await tester.pump();
      await tester.pump(const Duration(seconds: 2));
      expect(shown(tester), ['-x', '-y', '-z']);
      expect([
        for (var i = 0; i < 3; i++) tester.getRect(journeys().at(i)),
      ], rects);

      // The tap lands on the row that was under it.
      await finger.up();
      await tester.pump();
      expect(selected(tester), [false, true, false]);
      await tester.pump(structureSettle - const Duration(milliseconds: 20));
      expect(shown(tester), ['-x', '-y', '-z']);
      await tester.pump(const Duration(milliseconds: 30));
      await tester.pumpAndSettle();
      expect(shown(tester), ['-y', '-z', '-w']);
      expect(selected(tester), [true, false, false]);
    });

    testWidgets('a held row whose journey goes gives its place over the '
        'buttons to the first, and the sheet stays', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y', '-z']));
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      await moveTo(tester, footerAnchor);
      final sheet = sheetOf(tester);
      final extent = sheet.extent;
      final held = tester.getRect(journeys().at(1));
      expect(held.bottom, moreOrLessEquals(footerTop(tester)));
      final question = await refreshDue(tester);
      question.answer.complete(planOf(['-x', '-z'], shifts: [0, 1200]));
      await land(tester);
      for (var frame = 0; frame < 20; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        expect(sheet.extent, moreOrLessEquals(extent), reason: 'frame $frame');
        expect(tester.takeException(), isNull, reason: 'frame $frame');
      }
      await tester.pumpAndSettle();
      expect(shown(tester), ['-x', '-z']);
      expect(selected(tester), [true, false]);
      expect(sheet.restingAt, footerAnchor);
      expect(sheet.extent, moreOrLessEquals(extent));
      expect(tester.getRect(journeys().first), held);
    });

    testWidgets('times alone change under a finger', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await moveTo(tester, const SheetAnchor.full());
      final question = await refreshDue(tester);
      final finger = await tester.startGesture(
        tester.getCenter(journeys().at(1)),
      );
      await tester.pump();
      question.answer.complete(planOf(['-x', '-y'], shifts: [120, 720]));
      await land(tester);
      expect(
        tester.widget<JourneyRow>(journeys().first).journey.departure,
        DateTime.fromMillisecondsSinceEpoch((fakeStart + 120) * 1000),
      );
      await finger.up();
      await tester.pumpAndSettle();
    });
  });

  group('Trip details, as transit plans itself again', () {
    Future<void> choose(WidgetTester tester, String trip) async {
      final tile = find.byWidgetPredicate(
        (w) => w is ConnectionTile && w.departure.tripId == trip,
      );
      await tester.ensureVisible(tile);
      await tester.pumpAndSettle();
      await tester.tap(tile);
      await tester.pumpAndSettle();
    }

    testWidgets('stays on its journey if the new plan has it, with the '
        'departures chosen, its times changing', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await moveTo(tester, const SheetAnchor.full());
      await tester.tap(journeys().at(1));
      await tester.pumpAndSettle();
      await openDetails(tester);
      await choose(tester, 't40c-y');
      expect(openChain(tester).rides.first.departure?.tripId, 't40c-y');
      final page = tester.element(find.byType(TripDetailsPage));
      final arrival = openChain(tester).arrival;

      final question = await refreshDue(tester);
      await tester.pump(structureSettle);
      // The first journey has gone; this one runs two minutes later.
      question.answer.complete(planOf(['-y', '-z'], shifts: [720, 1200]));
      await land(tester);
      expect(find.byType(TripDetailsPage), findsOneWidget);
      expect(tester.element(find.byType(TripDetailsPage)), same(page));
      final chain = openChain(tester);
      expect(chain.journey.rides.first.tripId, 't40-y');
      expect(
        chain.journey.departure,
        DateTime.fromMillisecondsSinceEpoch((fakeStart + 720) * 1000),
      );
      expect(chain.rides.first.departure?.tripId, 't40c-y');
      expect(chain.rides.first.chosen, isTrue);
      expect(chain.arrival, arrival!.add(const Duration(minutes: 2)));
      await tester.pumpAndSettle();
      expect(find.byType(TripDetailsPage), findsOneWidget);
      // The map draws the journey as the new plan has it, with the choice.
      expect(drawnRides(), hasLength(2));
      // And it goes on being planned again while the details show.
      await refreshDue(tester);
    });

    testWidgets('keeps the stops of a departure chosen showing while its leg '
        'is fetched again', (tester) async {
      await openTransit(tester, plan: planOf(['-x']));
      await openDetails(tester);
      await choose(tester, 't40c-x');
      picker.transit.rides.single.answer.complete(
        fakeRideLeg(
          't40c-x',
          const [
            [0.6, 2.1],
            [0.7, 2.6],
            [0.9, 3.2],
          ],
          departure: fakeStart + 1320,
          from: 'Fremont Ave N & N 34th St',
          to: 'Westlake Station',
          between: const ['Aurora Bridge'],
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('1 stop before'), findsOneWidget);
      final question = await refreshDue(tester);
      await tester.pump(structureSettle);
      question.answer.complete(planOf(['-x']));
      await tester.pumpAndSettle();
      // Asked again of the new plan, and what was known stands in meanwhile.
      expect(picker.transit.rides, hasLength(2));
      expect(find.text('1 stop before'), findsOneWidget);
    });

    testWidgets('returns to the journeys if the new plan hasn\'t its '
        'journey', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await openDetails(tester);
      final question = await refreshDue(tester);
      await tester.pump(structureSettle);
      question.answer.complete(planOf(['-y', '-z'], shifts: [600, 1200]));
      await tester.pumpAndSettle();
      expect(find.byType(TripDetailsPage), findsNothing);
      expect(shown(tester), ['-y', '-z']);
      expect(selected(tester), [true, false]);
    });

    testWidgets('returns to the journeys when the trip changes, though the '
        'new plan has its journey', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await openDetails(tester);
      // How stops are reached follows the default, so transit is asked again.
      await picker.store.settings.set(wheelchairByDefault, true);
      await tester.pumpAndSettle();
      expect(picker.transit.asked.last.access, TransitAccess.wheelchair);
      expect(find.byType(TripDetailsPage), findsNothing);
      expect(shown(tester), ['-x', '-y']);
    });

    testWidgets('waits for a finger to lift', (tester) async {
      await openTransit(tester, plan: planOf(['-x', '-y']));
      await openDetails(tester);
      final question = await refreshDue(tester);
      final finger = await tester.startGesture(
        tester.getCenter(find.byType(JourneyHeader)),
      );
      await tester.pump();
      question.answer.complete(planOf(['-y'], shifts: [600]));
      await tester.pump();
      await tester.pump(const Duration(seconds: 1));
      expect(find.byType(TripDetailsPage), findsOneWidget);
      await finger.up();
      await tester.pump(structureSettle + const Duration(milliseconds: 20));
      await tester.pumpAndSettle();
      expect(find.byType(TripDetailsPage), findsNothing);
    });

    testWidgets('at twice the text size, choosing a departure moves nothing '
        'above it', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final report = FlutterError.onError;
      FlutterError.onError = (details) {
        if (details.toString().contains('lib/search/search_sheet.dart')) {
          return;
        }
        report?.call(details);
      };
      addTearDown(() => FlutterError.onError = report);
      await picker.store.settings.set(lastTravelMode, TravelMode.transit);
      picker.transit.immediately = TransitPlan.fromJson(connectionsPlanJson());
      await picker.open(tester);
      await openDetails(tester);
      SheetScaffold.controllerOf(tester.element(find.byType(TripDetailsPage)))
          .animateTo(const SheetAnchor.full());
      await tester.pumpAndSettle();
      // Another route's departure of the first ride, headed somewhere else.
      final tile = find.byWidgetPredicate(
        (w) => w is ConnectionTile && w.departure.tripId == '7-1',
      );
      await tester.ensureVisible(tile);
      await tester.pumpAndSettle();
      final before = tester.getRect(tile);
      final header = tester.getRect(
        find.byType(JourneyHeader, skipOffstage: false),
      );
      final stop = tester.getRect(find.text('Stop A').first);
      await tester.tap(tile);
      await tester.pumpAndSettle();
      expect(openChain(tester).rides.first.departure?.tripId, '7-1');
      expect(find.text('Rainier Beach'), findsWidgets);
      expect(tester.getRect(tile), before);
      expect(
        tester.getRect(find.byType(JourneyHeader, skipOffstage: false)),
        header,
      );
      expect(tester.getRect(find.text('Stop A').first), stop);
      expect(tester.takeException(), isNull);
    });
  });
}
