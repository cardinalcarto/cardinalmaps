import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../ui/adaptive.dart';
import '../units.dart';
import 'maneuver_banner.dart';
import 'navigation_format.dart';

/// The steps after the current one, [stepIndex], of [steps]: what the maneuver banner
/// opens to list. None once the trip's past its last step.
List<NavStep> upcomingSteps(List<NavStep> steps, int stepIndex) =>
    stepIndex + 1 >= steps.length || stepIndex < 0
    ? const []
    : steps.sublist(stepIndex + 1);

/// The list the maneuver banner opens into, under its rows: each of [steps] after the
/// current one, in order, as Ferrostar lists them (spec §1.4 and §2.2).
///
/// Each row is the step's first banner's primary content (its icon, then its text) and,
/// above the text, the step's length: a step's banner is for the maneuver at its end,
/// so its length is how far that maneuver is from the one before it. It's in the
/// banner's primary row's type, and the list ends with arriving, the last step with a
/// banner (the route's very last step, of no length, has none). Steps without a banner
/// are left out, as both platforms leave them.
///
/// A divider comes before each row, set in from the start by the rows' side padding as
/// iOS sets it in by its 16, so the first parts the list from the banner's rows; the
/// rows are 8 apart above and below, Ferrostar's 8. It scrolls, Material's clamping
/// on Android and the desktop and iOS's bounce on Cupertino.
class StepList extends StatelessWidget {
  const StepList({super.key, required this.steps, required this.units});

  final List<NavStep> steps;

  /// [UnitSystem.metric] or [UnitSystem.imperial], resolved for the device's region.
  final UnitSystem units;

  static const _ends = 8.0;

  @override
  Widget build(BuildContext context) {
    final listed = [
      for (final step in steps)
        if (step.visualInstructions.firstOrNull case final banner?)
          (step, banner.primary),
    ];
    final colors = Theme.of(context).colorScheme;
    final (distanceStyle, textStyle) = ManeuverBanner.primaryStyles(context);
    final words = AppLocalizations.of(context);
    const sides = ManeuverBanner.sides;
    return ListView.builder(
      // The banner is already inside the screen's insets.
      padding: EdgeInsets.zero,
      itemCount: listed.length,
      itemBuilder: (context, i) {
        final (step, content) = listed[i];
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Divider(
              height: 1,
              thickness: isCupertinoOf(context) ? 0.5 : 1,
              indent: sides,
              color: colors.outlineVariant,
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(sides, _ends, sides, _ends),
              child: ManeuverRow(
                content: content,
                distance: formatManeuverDistance(
                  step.distance,
                  units: units,
                  words: words,
                ),
                distanceStyle: distanceStyle,
                textStyle: textStyle,
              ),
            ),
          ],
        );
      },
    );
  }
}
