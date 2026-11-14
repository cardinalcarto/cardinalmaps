import 'package:cardinal/navigation/navigation_format.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';

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
}
