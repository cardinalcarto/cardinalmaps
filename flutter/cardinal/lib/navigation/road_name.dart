import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../ui/adaptive.dart';
import 'navigation_controller.dart';

/// The road the navigation screen names in its road-name pill, or null when it shows
/// none: the current step's road name, trimmed, as Ferrostar takes it (spec §2.4).
///
/// Shown only while the camera is [following] you and you're on the route, as on iOS:
/// off the route, and while a new route comes, the step is the old route's and may not
/// be the road you're on; and with the camera showing the rest of the trip, or moved
/// by hand, the road under you may not be on screen at all. iOS counts a shortcut onto
/// a later step ([NavDeviation.offStep]) as off too, since the current step is then
/// one you've left. Nor at the destination, where the trip is over.
String? roadNameToShow(
  NavigationController controller, {
  required bool following,
}) {
  if (!following || controller.state != NavigationState.onRoute) return null;
  final update = controller.update;
  if (update == null || update.deviation != NavDeviation.onRoute) return null;
  final steps = controller.steps;
  final index = update.stepIndex;
  if (index < 0 || index >= steps.length) return null;
  final name = steps[index].roadName?.trim();
  return name == null || name.isEmpty ? null : name;
}

/// The pill naming the road you're on, near the bottom of the navigation screen: iOS's
/// `CurrentRoadNameView` (spec §2.4), in the app's theme.
///
/// Ferrostar fills it with a fixed blue and edges it in white. Here it's the theme's
/// accent, its primary color, with on-primary text, so it's the app's color and dark
/// themes change it; and the edge is the surface color, which keeps the pill apart
/// from the map under it as white does, in either theme. The edge is iOS's 2 on Cupertino
/// and Android's 1 on Material.
///
/// The type is iOS's callout, bold, on Cupertino. On Material it's label large, a step
/// up from Android's label small: at 11 px the name was hard to read at a glance from a
/// mounted phone, and at 14 it's still the smallest type on the screen, under trip
/// progress's title medium (16) and the banner's title large and headline small (22
/// and 24), so the banner stays what's read first. Label large is Material's type for
/// text inside a component, as in a chip, which the pill is. On Cupertino, callout (16)
/// under trip progress and the banner's Title 3 (20) keeps the same order. The
/// padding is iOS's, 6 above and below and 12 at the sides, which is tighter than
/// Android's 12 all round, as the banner and trip progress are.
///
/// A name too long to fit shrinks, as iOS's does, to half its size at most; past that,
/// it's cut off with an ellipsis.
class RoadNamePill extends StatelessWidget {
  const RoadNamePill({super.key, required this.name});

  /// The road's name; nothing shows without one.
  final String? name;

  /// The least the name shrinks to before it's cut off: iOS's `minimumScaleFactor`.
  static const minScale = 0.5;

  static const _sides = 12.0;
  static const _ends = 6.0;

  /// How tall the pill is in [context], with a name or without one: its line of type,
  /// at the device's text size, in its padding and edge. The navigation screen keeps
  /// the chevron clear of it by this, so the camera doesn't move as the pill comes and
  /// goes.
  static double heightOf(BuildContext context) {
    final painter = TextPainter(
      text: TextSpan(text: ' ', style: _style(context)),
      maxLines: 1,
      textDirection: Directionality.of(context),
      textScaler: MediaQuery.textScalerOf(context),
    )..layout();
    final line = painter.height;
    painter.dispose();
    return line + 2 * (_ends + _border(context));
  }

  /// The edge's width: iOS's 2 on Cupertino, Android's 1 on Material.
  static double _border(BuildContext context) =>
      isCupertinoOf(context) ? 2.0 : 1.0;

  /// The name's type (see the class's description).
  static TextStyle _style(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final text = theme.textTheme;
    return (isCupertinoOf(context)
                ? text.bodyLarge?.copyWith(
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                  )
                : text.labelLarge)
            ?.copyWith(color: colors.onPrimary) ??
        TextStyle(color: colors.onPrimary);
  }

  @override
  Widget build(BuildContext context) {
    final name = this.name;
    if (name == null || name.isEmpty) return const SizedBox.shrink();
    final colors = Theme.of(context).colorScheme;
    final cupertino = isCupertinoOf(context);
    final style = _style(context);
    final border = _border(context);
    final shape = StadiumBorder(
      side: BorderSide(color: colors.surface, width: border),
    );
    final label = LayoutBuilder(
      builder: (context, constraints) {
        // What the text has across, inside the padding and the edge.
        final room = constraints.maxWidth - 2 * (_sides + border);
        final scale = _scaleToFit(context, name, style, room);
        return Text(
          name,
          maxLines: 1,
          softWrap: false,
          overflow: TextOverflow.ellipsis,
          style: style.copyWith(fontSize: (style.fontSize ?? 14) * scale),
        );
      },
    );
    final content = Padding(
      padding: EdgeInsets.symmetric(
        horizontal: _sides + border,
        vertical: _ends + border,
      ),
      child: label,
    );
    if (cupertino) {
      return DecoratedBox(
        decoration: ShapeDecoration(color: colors.primary, shape: shape),
        child: content,
      );
    }
    // Android's has a shadow, as the trip-progress pill does.
    return Material(
      color: colors.primary,
      elevation: 3,
      shape: shape,
      child: content,
    );
  }

  /// How much [name] in [style] shrinks to fit in [room] logical pixels: not at all if
  /// it fits, and never below [minScale].
  static double _scaleToFit(
    BuildContext context,
    String name,
    TextStyle style,
    double room,
  ) {
    if (!room.isFinite) return 1;
    final painter = TextPainter(
      text: TextSpan(text: name, style: style),
      maxLines: 1,
      textDirection: Directionality.of(context),
      textScaler: MediaQuery.textScalerOf(context),
    )..layout();
    final width = painter.width;
    painter.dispose();
    if (width <= room || width <= 0) return 1;
    return math.max(minScale, room / width);
  }
}
