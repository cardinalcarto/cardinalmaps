/// The units the app measures in, for directions and, later, the topo map.
library;

import 'dart:ui' show Locale;

import 'package:flutter/widgets.dart' show WidgetsBinding;

import 'store/store.dart';

/// Which units distances are in.
enum UnitSystem {
  /// The units of the device's region.
  locale,

  /// Kilometers and meters.
  metric,

  /// Miles and feet.
  imperial;

  /// The units to use in [locale]: [metric] or [imperial], never [UnitSystem.locale],
  /// which becomes the locale region's.
  UnitSystem resolve(Locale locale) => switch (this) {
    UnitSystem.locale =>
      _imperialRegions.contains(locale.countryCode) ? imperial : metric,
    _ => this,
  };

  /// [resolve] for the region the device is set to, now.
  ///
  /// The locale comes from the widgets binding rather than `PlatformDispatcher.instance`
  /// so that widget tests can set it.
  UnitSystem get onDevice =>
      resolve(WidgetsBinding.instance.platformDispatcher.locale);
}

/// The regions whose distances are in miles: those CLDR puts in the US measurement system
/// (the US, Liberia and Myanmar), and Great Britain, which CLDR puts in its own system,
/// metric but for miles on the road.
const _imperialRegions = {'US', 'LR', 'MM', 'GB'};

/// The units the app measures in. Not in the settings screen yet: it follows the device's
/// region.
final unitSystem = Setting.choice(
  'units',
  UnitSystem.values,
  UnitSystem.locale,
);
