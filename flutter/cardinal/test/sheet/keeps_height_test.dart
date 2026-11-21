import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

const _screen = Size(400, 800);
const _low = SheetAnchor.fraction(0.25);
const _high = SheetAnchor.fraction(0.75);
const _other = SheetAnchor.fraction(0.5);

/// A page of the harness, named for finding it.
typedef _Page = ({String name, List<SheetAnchor> anchors, bool keepsHeight});

_Page _keeping(String name) =>
    (name: name, anchors: const [_low, _high], keepsHeight: true);

/// Pages over one another in a sheet, pushed and popped by [_HarnessState].
class _Harness extends StatefulWidget {
  const _Harness({super.key, required this.first});

  final _Page first;

  @override
  State<_Harness> createState() => _HarnessState();
}

class _HarnessState extends State<_Harness> {
  late final List<_Page> _pages = [widget.first];

  void push(_Page page) => setState(() => _pages.add(page));

  @override
  Widget build(BuildContext context) => MaterialApp(
    home: Scaffold(
      body: SheetScaffold(
        body: const SizedBox.expand(),
        sheet: Navigator(
          pages: [
            for (final page in _pages)
              SheetPage<void>(
                key: ValueKey(page.name),
                child: SheetContent(
                  anchors: page.anchors,
                  keepsHeight: page.keepsHeight,
                  slivers: [
                    SliverToBoxAdapter(
                      child: SizedBox(height: 50, child: Text(page.name)),
                    ),
                    SliverList.builder(
                      itemCount: 40,
                      itemBuilder: (context, i) =>
                          SizedBox(height: 50, child: Text('${page.name} $i')),
                    ),
                  ],
                ),
              ),
          ],
          onDidRemovePage: (page) => setState(
            () => _pages.removeWhere((p) => ValueKey(p.name) == page.key),
          ),
        ),
      ),
    ),
  );
}

final _harness = GlobalKey<_HarnessState>();

Future<SheetController> _pump(WidgetTester tester, _Page first) async {
  tester.view
    ..physicalSize = _screen * 2
    ..devicePixelRatio = 2;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(_Harness(key: _harness, first: first));
  await tester.pumpAndSettle();
  return SheetScaffold.controllerOf(tester.element(find.text(first.name)));
}

Future<void> _push(WidgetTester tester, _Page page) async {
  _harness.currentState!.push(page);
  await tester.pumpAndSettle();
}

/// Back on the sheet's navigator, the inner one.
Future<void> _back(WidgetTester tester) async {
  await tester.state<NavigatorState>(find.byType(Navigator).last).maybePop();
  await tester.pumpAndSettle();
}

Future<void> _moveTo(
  WidgetTester tester,
  SheetController sheet,
  SheetAnchor anchor,
) async {
  sheet.animateTo(anchor);
  await tester.pumpAndSettle();
}

double _at(double fraction) => _screen.height * fraction;

/// Sends a predictive back event, as Android's engine does.
Future<void> _sendBack(
  WidgetTester tester,
  String method, [
  double? progress,
]) async {
  await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
    'flutter/backgesture',
    const StandardMethodCodec().encodeMethodCall(
      MethodCall(method, {
        if (progress != null) ...{
          'touchOffset': [5.0, 300.0],
          'progress': progress,
          'swipeEdge': 0,
        },
      }),
    ),
    (_) {},
  );
  await tester.pump();
}

void main() {
  testWidgets('a page pushed at a height it has stays there', (tester) async {
    final sheet = await _pump(tester, _keeping('a'));
    await _moveTo(tester, sheet, _high);
    await _push(tester, _keeping('b'));
    expect(find.text('b'), findsOneWidget);
    expect(sheet.extent, _at(0.75));
    expect(sheet.restingAt, _high);
  });

  testWidgets('it stays there every frame of the way in', (tester) async {
    final sheet = await _pump(tester, _keeping('a'));
    await _moveTo(tester, sheet, _high);
    _harness.currentState!.push(_keeping('b'));
    for (var i = 0; i < 30; i++) {
      await tester.pump(const Duration(milliseconds: 16));
      expect(sheet.extent, _at(0.75), reason: 'frame $i');
    }
  });

  testWidgets('going back keeps the height the page over it was left at', (
    tester,
  ) async {
    final sheet = await _pump(tester, _keeping('a'));
    await _moveTo(tester, sheet, _high);
    await _push(tester, _keeping('b'));
    await _moveTo(tester, sheet, _low);
    await _back(tester);
    expect(find.text('b'), findsNothing);
    expect(sheet.extent, _at(0.25));
    expect(sheet.restingAt, _low);
  });

  testWidgets('it carries on through several pages, both ways', (tester) async {
    final sheet = await _pump(tester, _keeping('a'));
    await _moveTo(tester, sheet, _high);
    await _push(tester, _keeping('b'));
    await _push(tester, _keeping('c'));
    expect(sheet.extent, _at(0.75));
    await _moveTo(tester, sheet, _low);
    await _back(tester);
    await _back(tester);
    expect(find.text('a'), findsOneWidget);
    expect(sheet.extent, _at(0.25));
  });

  testWidgets('a collapsed sheet stays collapsed, both ways', (tester) async {
    final sheet = await _pump(tester, (
      name: 'a',
      anchors: const [_low, _high, SheetAnchor.collapsed()],
      keepsHeight: true,
    ));
    await _moveTo(tester, sheet, const SheetAnchor.collapsed());
    await _push(tester, (
      name: 'b',
      anchors: const [_low, _high, SheetAnchor.collapsed()],
      keepsHeight: true,
    ));
    expect(sheet.isCollapsed, isTrue);
    await _back(tester);
    expect(sheet.isCollapsed, isTrue);
  });

  testWidgets('a page without the height goes to its first anchor', (
    tester,
  ) async {
    final sheet = await _pump(tester, _keeping('a'));
    await _moveTo(tester, sheet, _high);
    await _push(tester, (
      name: 'b',
      anchors: const [_other, _low],
      keepsHeight: true,
    ));
    expect(sheet.extent, _at(0.5));
    // Gone back to, the page that has it keeps the height it can: it was left at 0.5,
    // which it hasn't, so it goes back to where it was.
    await _back(tester);
    expect(sheet.extent, _at(0.75));
  });

  testWidgets('without it, pages move as before', (tester) async {
    final sheet = await _pump(tester, (
      name: 'a',
      anchors: const [_low, _high],
      keepsHeight: false,
    ));
    await _moveTo(tester, sheet, _high);
    await _push(tester, (
      name: 'b',
      anchors: const [_low, _high],
      keepsHeight: false,
    ));
    // To its first anchor...
    expect(sheet.extent, _at(0.25));
    await _moveTo(tester, sheet, _high);
    await _moveTo(tester, sheet, _low);
    // ...and back to where the page below was left.
    await _back(tester);
    expect(sheet.extent, _at(0.75));
  });

  testWidgets('a back gesture holds the height all the way', (tester) async {
    final sheet = await _pump(tester, _keeping('a'));
    await _push(tester, _keeping('b'));
    await _moveTo(tester, sheet, _high);
    await _sendBack(tester, 'startBackGesture', 0);
    for (final progress in [0.2, 0.5, 0.8]) {
      await _sendBack(tester, 'updateBackGestureProgress', progress);
      expect(sheet.extent, _at(0.75), reason: 'at $progress');
    }
    await _sendBack(tester, 'commitBackGesture');
    for (var i = 0; i < 30; i++) {
      await tester.pump(const Duration(milliseconds: 16));
      expect(sheet.extent, _at(0.75), reason: 'frame $i');
    }
    await tester.pumpAndSettle();
    expect(find.text('b'), findsNothing);
    expect(sheet.restingAt, _high);
  });
}
