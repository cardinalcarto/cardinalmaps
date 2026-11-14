import 'dart:ui' show Locale;

import 'package:cardinal/store/store.dart';
import 'package:cardinal/units.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('follow the locale until chosen', () async {
    final settings = SettingsStore.inMemory();
    expect(await settings.get(unitSystem), UnitSystem.locale);
    await settings.set(unitSystem, UnitSystem.metric);
    expect(await settings.get(unitSystem), UnitSystem.metric);
  });

  test("the locale's are its region's", () {
    for (final (locale, units) in [
      (const Locale('en', 'US'), UnitSystem.imperial),
      (const Locale('es', 'US'), UnitSystem.imperial),
      (const Locale('en', 'LR'), UnitSystem.imperial),
      (const Locale('my', 'MM'), UnitSystem.imperial),
      // Metric, but miles on the road.
      (const Locale('en', 'GB'), UnitSystem.imperial),
      (const Locale('en', 'CA'), UnitSystem.metric),
      (const Locale('pt', 'PT'), UnitSystem.metric),
      // No region.
      (const Locale('en'), UnitSystem.metric),
    ]) {
      expect(UnitSystem.locale.resolve(locale), units, reason: '$locale');
    }
  });

  test('a choice holds in any locale', () {
    const us = Locale('en', 'US');
    const france = Locale('fr', 'FR');
    expect(UnitSystem.metric.resolve(us), UnitSystem.metric);
    expect(UnitSystem.imperial.resolve(france), UnitSystem.imperial);
  });
}
