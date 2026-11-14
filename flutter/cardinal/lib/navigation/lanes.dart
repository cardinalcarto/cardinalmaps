/// Lane guidance: the row of lane arrows under the maneuver banner's primary row.
library;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import 'maneuver_glyphs.dart';

/// The arrow a lane is drawn with: a directions-icons `turn_` icon, and whether it's
/// mirrored.
typedef LaneGlyph = ({IconData icon, bool mirrored});

/// The arrow [lane] is drawn with, where traffic keeps to [drivingSide].
///
/// One arrow a lane, as the banner's lanes are small: an active lane's is the direction
/// to take it in ([NavLane.activeDirection], else its first), and an inactive lane's its
/// first direction. A lane with no arrows painted on it (`none`), or one this doesn't
/// know, is drawn straight, as nav apps draw them.
///
/// Mapbox's directions-icons has no lane icons, so each direction is drawn with the
/// maneuver icon for turning that way, `turn_left`, `turn_slight_right` and so on, whose
/// arrows all point up from the bottom middle as lane arrows do. The U-turn icon turns
/// to the left, as a U-turn does where traffic keeps right; where it keeps left
/// ([NavDrivingSide.left]), it's mirrored to turn right. The others aren't: a left lane
/// is a left lane on either side of the road.
LaneGlyph laneGlyph(NavLane lane, NavDrivingSide? drivingSide) {
  final direction = lane.active
      ? lane.activeDirection ?? lane.directions.firstOrNull
      : lane.directions.firstOrNull;
  final name = switch (direction?.trim().replaceAll(' ', '_')) {
    'left' => 'turn_left',
    'right' => 'turn_right',
    'slight_left' => 'turn_slight_left',
    'slight_right' => 'turn_slight_right',
    'sharp_left' => 'turn_sharp_left',
    'sharp_right' => 'turn_sharp_right',
    'uturn' => 'turn_uturn',
    _ => 'turn_straight',
  };
  return (
    icon: maneuverGlyphs[name]!,
    mirrored: name == 'turn_uturn' && drivingSide == NavDrivingSide.left,
  );
}

/// The lanes at the coming maneuver, left to right, each as its arrow ([laneGlyph]):
/// those that can be used for it in the on-surface color, and the rest dimmed to
/// [inactiveOpacity], with a hairline between each two as lane lines.
///
/// It shrinks to fit a road of more lanes than the banner is wide.
class LaneStrip extends StatelessWidget {
  const LaneStrip({super.key, required this.lanes, this.drivingSide});

  final List<NavLane> lanes;
  final NavDrivingSide? drivingSide;

  /// Each arrow's size, and its lane's width.
  static const iconSize = 30.0;
  static const laneWidth = 36.0;

  /// How opaque an inactive lane's arrow is.
  static const inactiveOpacity = 0.3;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final words = AppLocalizations.of(context);
    final divider = SizedBox(
      height: iconSize * 0.8,
      child: VerticalDivider(
        width: 1,
        thickness: 1,
        color: colors.outlineVariant,
      ),
    );
    return Semantics(
      label: words.navigationLanes(
        lanes.where((lane) => lane.active).length,
        lanes.length,
      ),
      excludeSemantics: true,
      child: FittedBox(
        fit: BoxFit.scaleDown,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          // Lanes are left to right whatever the language's direction.
          textDirection: TextDirection.ltr,
          children: [
            for (final (i, lane) in lanes.indexed) ...[
              if (i > 0) divider,
              SizedBox(
                width: laneWidth,
                child: _Lane(glyph: laneGlyph(lane, drivingSide), lane: lane),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _Lane extends StatelessWidget {
  const _Lane({required this.glyph, required this.lane});

  final LaneGlyph glyph;
  final NavLane lane;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.onSurface;
    Widget icon = Icon(
      glyph.icon,
      size: LaneStrip.iconSize,
      color: lane.active
          ? color
          : color.withValues(alpha: LaneStrip.inactiveOpacity),
    );
    if (glyph.mirrored) {
      icon = Transform.flip(flipX: true, child: icon);
    }
    return icon;
  }
}
