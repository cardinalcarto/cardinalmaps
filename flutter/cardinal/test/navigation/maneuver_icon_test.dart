import 'package:cardinal/navigation/maneuver_glyphs.dart';
import 'package:cardinal/navigation/maneuver_icon.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

/// The icons in Ferrostar iOS's `Directions.xcassets` (FerrostarSwiftUI, at
/// 6a03904) that a maneuver's type and modifier can name: all but `close`, `flag`,
/// `updown` and the `invalid` ones.
const ferrostarIcons = {
  'arrive', 'arrive_left', 'arrive_right', 'arrive_straight', //
  'continue', 'continue_left', 'continue_right', 'continue_slight_left',
  'continue_slight_right', 'continue_straight', 'continue_uturn', //
  'depart', 'depart_left', 'depart_right', 'depart_straight', //
  'end_of_road_left', 'end_of_road_right', //
  'fork', 'fork_left', 'fork_right', 'fork_slight_left', 'fork_slight_right',
  'fork_straight', //
  'merge_left', 'merge_right', 'merge_slight_left', 'merge_slight_right',
  'merge_straight', //
  'new_name_left', 'new_name_right', 'new_name_sharp_left',
  'new_name_sharp_right', 'new_name_slight_left', 'new_name_slight_right',
  'new_name_straight', //
  'notification_left', 'notification_right', 'notification_sharp_left',
  'notification_sharp_right', 'notification_slight_left',
  'notification_slight_right', 'notification_straight', //
  'off_ramp_left', 'off_ramp_right', 'off_ramp_slight_left',
  'off_ramp_slight_right', //
  'on_ramp_left', 'on_ramp_right', 'on_ramp_sharp_left', 'on_ramp_sharp_right',
  'on_ramp_slight_left', 'on_ramp_slight_right', 'on_ramp_straight', //
  'rotary', 'rotary_left', 'rotary_right', 'rotary_sharp_left',
  'rotary_sharp_right', 'rotary_slight_left', 'rotary_slight_right',
  'rotary_straight', //
  'roundabout', 'roundabout_left', 'roundabout_right', 'roundabout_sharp_left',
  'roundabout_sharp_right', 'roundabout_slight_left', 'roundabout_slight_right',
  'roundabout_straight', //
  'turn_left', 'turn_right', 'turn_sharp_left', 'turn_sharp_right',
  'turn_slight_left', 'turn_slight_right', 'turn_straight', 'turn_uturn',
};

void main() {
  test('names are the type, then the modifier, spelled as OSRM does', () {
    expect(
      maneuverIconName(ManeuverType.turn, ManeuverModifier.left),
      'turn_left',
    );
    expect(
      maneuverIconName(ManeuverType.newName, ManeuverModifier.slightRight),
      'new_name_slight_right',
    );
    expect(
      maneuverIconName(ManeuverType.continue_, ManeuverModifier.uTurn),
      'continue_uturn',
    );
    expect(
      maneuverIconName(ManeuverType.endOfRoad, ManeuverModifier.sharpLeft),
      'end_of_road_sharp_left',
    );
    expect(
      maneuverIconName(ManeuverType.exitRoundabout, null),
      'exit_roundabout',
    );
  });

  test('the font has an icon for each that Ferrostar has, and no others', () {
    expect(maneuverGlyphs.keys.toSet(), ferrostarIcons);
    expect(
      maneuverGlyphs.values.map((icon) => icon.codePoint).toSet(),
      hasLength(ferrostarIcons.length),
    );
    for (final icon in maneuverGlyphs.values) {
      expect(icon.fontFamily, 'CardinalIcons');
    }
  });

  test('every type and modifier gets its icon, or none as in Ferrostar', () {
    var mapped = 0;
    for (final type in ManeuverType.values) {
      for (final modifier in <ManeuverModifier?>[
        null,
        ...ManeuverModifier.values,
      ]) {
        final name = maneuverIconName(type, modifier);
        final icon = maneuverIcon(type, modifier);
        if (ferrostarIcons.contains(name)) {
          mapped++;
          expect(icon, maneuverGlyphs[name], reason: name);
        } else {
          expect(icon, isNull, reason: name);
        }
      }
    }
    expect(mapped, ferrostarIcons.length);
  });

  test('no icon stands in for one directions-icons lacks', () {
    // No nearer icon, as for an exit from a roundabout, or a plain turn.
    expect(
      maneuverIcon(ManeuverType.exitRoundabout, ManeuverModifier.right),
      isNull,
    );
    expect(maneuverIcon(ManeuverType.turn, null), isNull);
    expect(
      maneuverIcon(ManeuverType.arrive, ManeuverModifier.sharpLeft),
      isNull,
    );
    // But without a modifier, the type's own, as on iOS.
    expect(maneuverIcon(ManeuverType.arrive, null), maneuverGlyphs['arrive']);
    expect(maneuverIcon(null, ManeuverModifier.left), isNull);
  });
}
