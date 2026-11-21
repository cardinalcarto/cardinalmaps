import 'package:cardinal/directions/step_icon.dart';
import 'package:cardinal/navigation/maneuver_glyphs.dart';
import 'package:flutter/widgets.dart' show IconData;
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

import '../fakes/fake_routes.dart';

void main() {
  IconData? icon(ManeuverType? type, [ManeuverModifier? modifier]) =>
      stepIcon(fakeStep('', type, modifier: modifier));

  test('a turn, each way, and its mirror', () {
    for (final (modifier, name) in [
      (ManeuverModifier.straight, 'turn_straight'),
      (ManeuverModifier.slightLeft, 'turn_slight_left'),
      (ManeuverModifier.slightRight, 'turn_slight_right'),
      (ManeuverModifier.left, 'turn_left'),
      (ManeuverModifier.right, 'turn_right'),
      (ManeuverModifier.sharpLeft, 'turn_sharp_left'),
      (ManeuverModifier.sharpRight, 'turn_sharp_right'),
      (ManeuverModifier.uTurn, 'turn_uturn'),
    ]) {
      expect(icon(ManeuverType.turn, modifier), maneuverGlyphs[name]);
    }
    expect(
      icon(ManeuverType.turn, ManeuverModifier.left),
      isNot(icon(ManeuverType.turn, ManeuverModifier.right)),
    );
  });

  test('merges, ramps and forks, by their own glyphs', () {
    expect(
      icon(ManeuverType.merge, ManeuverModifier.slightLeft),
      maneuverGlyphs['merge_slight_left'],
    );
    expect(
      icon(ManeuverType.onRamp, ManeuverModifier.right),
      maneuverGlyphs['on_ramp_right'],
    );
    expect(
      icon(ManeuverType.offRamp, ManeuverModifier.slightRight),
      maneuverGlyphs['off_ramp_slight_right'],
    );
    expect(
      icon(ManeuverType.fork, ManeuverModifier.slightLeft),
      maneuverGlyphs['fork_slight_left'],
    );
  });

  test('a roundabout by the way its exit goes', () {
    expect(
      stepIcon(
        fakeStep(
          '',
          ManeuverType.roundabout,
          modifier: ManeuverModifier.sharpRight,
          exit: 3,
        ),
      ),
      maneuverGlyphs['roundabout_sharp_right'],
    );
    expect(icon(ManeuverType.rotary), maneuverGlyphs['rotary']);
  });

  test('the start and the end', () {
    expect(icon(ManeuverType.depart), maneuverGlyphs['depart']);
    expect(icon(ManeuverType.arrive), maneuverGlyphs['arrive']);
    expect(
      icon(ManeuverType.arrive, ManeuverModifier.right),
      maneuverGlyphs['arrive_right'],
    );
  });

  test('where there\'s no glyph for the pair, the nearest that says the '
      'same', () {
    // No arrive_sharp_left: an arrival.
    expect(
      icon(ManeuverType.arrive, ManeuverModifier.sharpLeft),
      maneuverGlyphs['arrive'],
    );
    // No exit_roundabout_right: a turn right.
    expect(
      icon(ManeuverType.exitRoundabout, ManeuverModifier.right),
      maneuverGlyphs['turn_right'],
    );
  });

  test('none for a step that doesn\'t say, or with nothing near enough', () {
    expect(icon(null), isNull);
    expect(icon(null, ManeuverModifier.left), isNull);
    expect(icon(ManeuverType.exitRoundabout), isNull);
    expect(icon(ManeuverType.merge), isNull);
  });
}
