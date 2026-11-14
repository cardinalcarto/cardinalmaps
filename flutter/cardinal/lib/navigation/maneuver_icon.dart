/// The icon for a maneuver, as Ferrostar picks it: Mapbox's directions-icons, by the
/// maneuver's type and modifier.
library;

import 'package:flutter/widgets.dart';
import 'package:routing/routing.dart';

import 'maneuver_glyphs.dart';

/// The directions-icons name for a maneuver of [type] and [modifier]: the type, then
/// the modifier if there is one, as OSRM spells them with `_` for each space, such as
/// `turn_slight_left` or `roundabout`.
///
/// This is Ferrostar iOS's `ManeuverImageName`
/// (`apple/Sources/FerrostarSwiftUI/Models/ManeuverImageName.swift`, with the spellings
/// of `Extensions/ManeuverImageKeys.swift`). iOS is followed rather than Android, which
/// shows nothing for a maneuver without a modifier, such as a plain `arrive`.
String maneuverIconName(ManeuverType type, ManeuverModifier? modifier) => [
  _typeName(type),
  if (modifier != null) _modifierName(modifier),
].join('_').replaceAll(' ', '_');

/// The icon for a maneuver of [type] and [modifier], from the app's icon font; null if
/// directions-icons has none for it, such as `exit_roundabout` or `arrive_sharp_left`.
///
/// As in Ferrostar, the name must match an icon exactly, with no nearer icon stood in:
/// Ferrostar draws nothing for these (`ManeuverImage.swift` on iOS, `ManeuverImage.kt`
/// on Android). Nor does it draw roundabouts by their exit's degrees, or mirror icons
/// where traffic drives on the left; neither does this.
IconData? maneuverIcon(ManeuverType? type, ManeuverModifier? modifier) {
  if (type == null) return null;
  return maneuverGlyphs[maneuverIconName(type, modifier)];
}

/// OSRM's spelling of [type].
String _typeName(ManeuverType type) => switch (type) {
  ManeuverType.turn => 'turn',
  ManeuverType.newName => 'new name',
  ManeuverType.depart => 'depart',
  ManeuverType.arrive => 'arrive',
  ManeuverType.merge => 'merge',
  ManeuverType.onRamp => 'on ramp',
  ManeuverType.offRamp => 'off ramp',
  ManeuverType.fork => 'fork',
  ManeuverType.endOfRoad => 'end of road',
  ManeuverType.continue_ => 'continue',
  ManeuverType.roundabout => 'roundabout',
  ManeuverType.rotary => 'rotary',
  ManeuverType.roundaboutTurn => 'roundabout turn',
  ManeuverType.notification => 'notification',
  ManeuverType.exitRoundabout => 'exit roundabout',
  ManeuverType.exitRotary => 'exit rotary',
};

/// OSRM's spelling of [modifier].
String _modifierName(ManeuverModifier modifier) => switch (modifier) {
  ManeuverModifier.uTurn => 'uturn',
  ManeuverModifier.sharpRight => 'sharp right',
  ManeuverModifier.right => 'right',
  ManeuverModifier.slightRight => 'slight right',
  ManeuverModifier.straight => 'straight',
  ManeuverModifier.slightLeft => 'slight left',
  ManeuverModifier.left => 'left',
  ManeuverModifier.sharpLeft => 'sharp left',
};
