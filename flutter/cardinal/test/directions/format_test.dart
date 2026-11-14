import 'package:cardinal/directions/format.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('miles from a tenth of a mile, feet below that', () {
    expect(formatDistance(32.3, units: UnitSystem.imperial), '106 ft');
    expect(formatDistance(161, units: UnitSystem.imperial), '0.1 mi');
    expect(formatDistance(10460, units: UnitSystem.imperial), '6.5 mi');
    expect(formatDistance(32187, units: UnitSystem.imperial), '20 mi');
  });

  test('kilometers from one, meters below that', () {
    expect(formatDistance(420.4, units: UnitSystem.metric), '420 m');
    expect(formatDistance(1500, units: UnitSystem.metric), '1.5 km');
    expect(formatDistance(23400, units: UnitSystem.metric), '23 km');
  });

  test('durations round to minutes and read in hours past an hour', () {
    expect(formatDuration(0), '<1 min');
    expect(formatDuration(20), '<1 min');
    expect(formatDuration(672), '11 min');
    expect(formatDuration(7380), '2 hr 3 min');
    expect(formatDuration(7200), '2 hr');
  });
}
