import 'package:cardinal/l10n/app_localizations.dart';
import 'package:cardinal/navigation/navigation_format.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

void main() {
  String metric(double meters) =>
      formatManeuverDistance(meters, units: UnitSystem.metric);
  String imperial(double meters) =>
      formatManeuverDistance(meters, units: UnitSystem.imperial);

  test('metric: meters in steps of 5, 10 and 100 up to a kilometer', () {
    expect(metric(0), '0 m');
    expect(metric(7), '5 m');
    expect(metric(8), '10 m');
    expect(metric(10), '10 m');
    expect(metric(14), '10 m');
    expect(metric(96), '100 m');
    expect(metric(100), '100 m');
    expect(metric(149), '100 m');
    expect(metric(151), '200 m');
    expect(metric(949), '900 m');
    expect(metric(960), '1,000 m');
    expect(metric(1000), '1,000 m');
  });

  test('metric: kilometers to a tenth up to 10, whole above', () {
    expect(metric(1001), '1 km');
    expect(metric(1260), '1.3 km');
    expect(metric(9970), '10 km');
    expect(metric(10000), '10 km');
    expect(metric(10400), '10 km');
    expect(metric(23600), '24 km');
  });

  test('imperial: feet in steps of 5, 10, 50 and 100 up to 289 m', () {
    expect(imperial(0), '0 ft');
    // 12 m is 39.4 ft.
    expect(imperial(12), '40 ft');
    // 20 m is 65.6 ft.
    expect(imperial(20), '70 ft');
    // 100 m is 328 ft.
    expect(imperial(100), '350 ft');
    // 200 m is 656 ft.
    expect(imperial(200), '700 ft');
    // 289 m is 948 ft.
    expect(imperial(289), '900 ft');
  });

  test('imperial: miles to a tenth up to 10, whole above', () {
    // 290 m is 0.18 mi.
    expect(imperial(290), '0.2 mi');
    expect(imperial(1609.344), '1 mi');
    expect(imperial(2414), '1.5 mi');
    expect(imperial(16093.44), '10 mi');
    expect(imperial(16900), '11 mi');
  });

  group('trip duration', () {
    final words = lookupAppLocalizations(const Locale('en'));
    String duration(double seconds) => formatTripDuration(seconds, words);

    test('under a minute is 0m, and nothing left is too', () {
      expect(duration(0), '<1 min');
      expect(duration(1), '<1 min');
      expect(duration(59.9), '<1 min');
      // Never negative, nor anything for a value that isn't one.
      expect(duration(-5), '<1 min');
      expect(duration(double.nan), '<1 min');
      expect(duration(double.infinity), '<1 min');
    });

    test('minutes count down, truncated rather than rounded', () {
      expect(duration(60), '1 min');
      expect(duration(119), '1 min');
      expect(duration(18 * 60 + 45), '18 min');
      expect(duration(3599), '59 min');
    });

    test('at an hour, the minutes drop out when they are zero', () {
      expect(duration(3600), '1 hr');
      expect(duration(3659), '1 hr');
      expect(duration(3660), '1 hr 1 min');
      expect(duration(3600 + 5 * 60), '1 hr 5 min');
      expect(duration(2 * 3600), '2 hr');
      expect(duration(2 * 3600 - 1), '1 hr 59 min');
    });

    test('a long trip stays in hours, never days', () {
      expect(duration(24 * 3600), '24 hr');
      expect(duration(26 * 3600 + 5 * 60), '26 hr 5 min');
      expect(duration(100 * 3600 + 59 * 60 + 59), '100 hr 59 min');
    });
  });

  group('arrival time', () {
    Future<String> arrival(
      WidgetTester tester,
      DateTime when, {
      required bool use24,
    }) async {
      late String shown;
      await tester.pumpWidget(
        MaterialApp(
          home: MediaQuery(
            data: MediaQueryData(alwaysUse24HourFormat: use24),
            child: Builder(
              builder: (context) {
                shown = formatArrivalTime(context, when);
                return const SizedBox();
              },
            ),
          ),
        ),
      );
      return shown;
    }

    testWidgets('in the 12-hour clock, or the 24-hour one if set', (
      tester,
    ) async {
      final afternoon = DateTime(2026, 11, 14, 17, 20);
      final morning = DateTime(2026, 11, 14, 9, 5);
      final midnight = DateTime(2026, 11, 15, 0, 7);
      expect(await arrival(tester, afternoon, use24: false), '5:20 PM');
      expect(await arrival(tester, afternoon, use24: true), '17:20');
      expect(await arrival(tester, morning, use24: false), '9:05 AM');
      expect(await arrival(tester, morning, use24: true), '09:05');
      expect(await arrival(tester, midnight, use24: false), '12:07 AM');
      expect(await arrival(tester, midnight, use24: true), '00:07');
    });
  });
}
