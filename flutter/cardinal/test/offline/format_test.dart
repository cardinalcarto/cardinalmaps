import 'package:cardinal/offline/format.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

void main() {
  test('sizes are in decimal units, with a decimal under 100', () {
    String size(int bytes) => formatBytes(bytes).replaceAll(' ', ' ');
    expect(size(0), '0 KB');
    expect(size(1500), '1.5 KB');
    expect(size(4 * 1000 * 1000), '4 MB');
    expect(size(42300000), '42.3 MB');
    expect(size(136 * 1000 * 1000), '136 MB');
    expect(size(12400000000), '12.4 GB');
    // Rounding up into the next unit.
    expect(size(99960000), '100 MB');
    expect(size(999600000), '1 GB');
    // Never split across lines.
    expect(formatBytes(136 * 1000 * 1000), '136 MB');
  });

  testWidgets('an area is named for when it was drawn, and dated', (
    tester,
  ) async {
    late BuildContext context;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (c) {
            context = c;
            return const SizedBox();
          },
        ),
      ),
    );
    final when = DateTime(2026, 11, 12, 21, 41);
    expect(defaultAreaName(context, when), 'Nov 12, 9:41 PM');
    expect(areaDate(context, when, now: DateTime(2026, 12, 1)), 'Nov 12');
    expect(areaDate(context, when, now: DateTime(2027, 1, 2)), 'Nov 12, 2026');
  });
}
