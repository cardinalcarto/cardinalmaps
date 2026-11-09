import 'dart:ui' show Locale;

import 'package:cardinal/directions/format.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('miles from a tenth of a mile, feet below that', () {
    expect(formatDistance(32.3, miles: true), '106 ft');
    expect(formatDistance(161, miles: true), '0.1 mi');
    expect(formatDistance(10460, miles: true), '6.5 mi');
    expect(formatDistance(32187, miles: true), '20 mi');
  });

  test('kilometers from one, meters below that', () {
    expect(formatDistance(420.4, miles: false), '420 m');
    expect(formatDistance(1500, miles: false), '1.5 km');
    expect(formatDistance(23400, miles: false), '23 km');
  });

  test('durations round to minutes and read in hours past an hour', () {
    expect(formatDuration(0), '<1 min');
    expect(formatDuration(20), '<1 min');
    expect(formatDuration(672), '11 min');
    expect(formatDuration(7380), '2 hr 3 min');
    expect(formatDuration(7200), '2 hr');
  });

  test('the countries whose roads are in miles use them', () {
    expect(usesMiles(const Locale('en', 'US')), isTrue);
    expect(usesMiles(const Locale('en', 'GB')), isTrue);
    expect(usesMiles(const Locale('de', 'DE')), isFalse);
    expect(usesMiles(const Locale('en')), isFalse);
  });
}
