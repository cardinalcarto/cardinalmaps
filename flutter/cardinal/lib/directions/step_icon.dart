/// The arrow a route's step is shown with on the Steps page: the same directions-icons
/// glyphs the maneuver banner shows while navigating, so a maneuver looks the same
/// before the trip and during it.
library;

import 'package:flutter/widgets.dart';
import 'package:routing/routing.dart';

import '../navigation/maneuver_icon.dart';

/// The icon for [step]'s maneuver, or null if the step doesn't say what its maneuver
/// is or there's no icon near enough for it.
///
/// The glyph for its type and modifier, as the banner picks it ([maneuverIcon]). Where
/// directions-icons has none for that pair, the nearest that says the same: the type's
/// own glyph without the modifier (an arrival to the sharp left is an arrival), then a
/// turn the modifier's way (leaving a roundabout to the right is a turn right).
/// directions-icons draws left and right as glyphs of their own, so nothing is
/// mirrored here.
IconData? stepIcon(DirectionsStep step) {
  final type = step.maneuverType;
  if (type == null) return null;
  final modifier = step.maneuverModifier;
  return maneuverIcon(type, modifier) ??
      maneuverIcon(type, null) ??
      (modifier == null ? null : maneuverIcon(ManeuverType.turn, modifier));
}
