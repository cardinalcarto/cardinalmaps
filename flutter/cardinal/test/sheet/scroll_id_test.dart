import 'package:cardinal/sheet/sheet.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

const _screen = Size(400, 800);

/// One page whose content changes, as a list does to one of its items.
class _Harness extends StatefulWidget {
  const _Harness({super.key});

  @override
  State<_Harness> createState() => _HarnessState();
}

class _HarnessState extends State<_Harness> {
  String content = 'list';

  void show(String value) => setState(() => content = value);

  @override
  Widget build(BuildContext context) => MaterialApp(
    home: Scaffold(
      body: SheetScaffold(
        body: const SizedBox.expand(),
        sheet: Navigator(
          pages: [
            SheetPage<void>(
              key: const ValueKey('page'),
              child: SheetContent(
                anchors: const [SheetAnchor.full()],
                scrollId: content,
                slivers: [
                  SliverList.builder(
                    itemCount: 40,
                    itemBuilder: (context, i) =>
                        SizedBox(height: 50, child: Text('$content $i')),
                  ),
                ],
              ),
            ),
          ],
          onDidRemovePage: (_) {},
        ),
      ),
    ),
  );
}

final _harness = GlobalKey<_HarnessState>();

Future<void> _pump(WidgetTester tester) async {
  tester.view
    ..physicalSize = _screen * 2
    ..devicePixelRatio = 2;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(_Harness(key: _harness));
  await tester.pumpAndSettle();
}

double _offset(WidgetTester tester) =>
    tester.state<ScrollableState>(find.byType(Scrollable).last).position.pixels;

void main() {
  testWidgets('new content starts at its top, on its first frame', (
    tester,
  ) async {
    await _pump(tester);
    await tester.drag(find.text('list 3'), const Offset(0, -600));
    await tester.pumpAndSettle();
    expect(_offset(tester), greaterThan(0));

    _harness.currentState!.show('item');
    await tester.pump();
    expect(_offset(tester), 0);
    expect(find.text('item 0'), findsOneWidget);
  });

  testWidgets('content shown again is where it was left', (tester) async {
    await _pump(tester);
    await tester.drag(find.text('list 3'), const Offset(0, -600));
    await tester.pumpAndSettle();
    final left = _offset(tester);

    _harness.currentState!.show('item');
    await tester.pumpAndSettle();
    _harness.currentState!.show('list');
    await tester.pump();
    expect(_offset(tester), left);
  });
}
