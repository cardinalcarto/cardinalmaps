import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../ui/adaptive.dart';
import '../units.dart';
import 'maneuver_icon.dart';
import 'navigation_format.dart';

/// The banner at the top of the navigation screen: the next maneuver's icon, how far
/// off it is, and what to do there, and under that, when there is one, the maneuver
/// right after it.
///
/// It's Ferrostar iOS's collapsed instructions banner (spec §2.1, `InstructionsView`
/// in FerrostarSwiftUI): iOS's rather than Android's because only iOS shows the
/// secondary row. Each row is an icon column, then the text: the primary row has the
/// distance above two lines at most of the instruction, and the secondary row, on a
/// band of its own, just its instruction. Lanes, the sub row, exit numbers and the
/// list of steps that iOS opens on a tap aren't here yet.
///
/// Nothing shows without an [instruction], as in Ferrostar: before the trip has one,
/// and between routes.
///
/// Material on Android and the desktop takes its type from Ferrostar Android's
/// (`InstructionRowTheme.kt`); Cupertino takes iOS's, and its card's corners and
/// shadow. Both are tighter than Ferrostar's 16 around each row.
class ManeuverBanner extends StatelessWidget {
  const ManeuverBanner({
    super.key,
    required this.instruction,
    required this.distance,
    required this.units,
  });

  final NavVisualInstruction? instruction;

  /// How far the maneuver is, in meters; null leaves the distance out.
  final double? distance;

  /// [UnitSystem.metric] or [UnitSystem.imperial], resolved for the device's region.
  final UnitSystem units;

  /// The icon's size, and its column's width: iOS draws the icon up to 48 wide.
  static const iconSize = 48.0;

  /// The secondary row's icon: smaller than the primary's, so the row reads as the
  /// lesser one, in a column as wide as the primary's so the texts still line up.
  static const secondaryIconSize = 32.0;

  /// Between the icon column and the text.
  static const _gap = 12.0;

  /// Around each row's content: Ferrostar has 16 at the sides and above and below,
  /// and 8 more between the rows.
  static const _sides = 12.0;
  static const _ends = 10.0;
  static const _between = 6.0;

  @override
  Widget build(BuildContext context) {
    final instruction = this.instruction;
    if (instruction == null) return const SizedBox.shrink();
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final text = theme.textTheme;
    final cupertino = isCupertinoOf(context);
    final secondary = instruction.secondary;
    final distance = this.distance;

    final primaryRow = _Row(
      content: instruction.primary,
      distance: distance == null
          ? null
          : formatManeuverDistance(
              distance,
              units: units,
              words: AppLocalizations.of(context),
            ),
      // iOS: Title 1 bold for the distance, Title 2 in the secondary label color for
      // the instruction (Title 3's size standing in, the app's theme having no Title
      // 2). Android: title large for the distance, headline small for the
      // instruction.
      distanceStyle: (cupertino ? text.headlineMedium : text.titleLarge)
          ?.copyWith(color: colors.onSurface),
      textStyle:
          (cupertino
                  ? text.titleLarge?.copyWith(fontWeight: FontWeight.w400)
                  : text.headlineSmall)
              ?.copyWith(color: colors.onSurfaceVariant),
    );
    final rows = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: EdgeInsets.fromLTRB(
            _sides,
            _ends,
            _sides,
            secondary == null ? _ends : _between,
          ),
          child: primaryRow,
        ),
        if (secondary != null)
          ColoredBox(
            // iOS's secondary system background: the iOS theme's container low.
            color: cupertino
                ? colors.surfaceContainerLow
                : colors.surfaceContainerHigh,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(
                _sides,
                _between,
                _sides,
                _ends,
              ),
              child: _Row(
                content: secondary,
                iconSize: ManeuverBanner.secondaryIconSize,
                distance: null,
                distanceStyle: null,
                // iOS's Subheadline.
                textStyle: (cupertino ? text.bodyMedium : text.bodyLarge)
                    ?.copyWith(color: colors.onSurfaceVariant),
              ),
            ),
          ),
      ],
    );
    const radius = BorderRadius.all(Radius.circular(12));
    if (cupertino) {
      // iOS's drawer: corner radius 12 and a shadow of radius 12, SwiftUI's default
      // shadow color being black at a third.
      return DecoratedBox(
        decoration: BoxDecoration(
          color: colors.surface,
          borderRadius: radius,
          boxShadow: const [
            BoxShadow(color: Color(0x55000000), blurRadius: 12),
          ],
        ),
        child: ClipRRect(borderRadius: radius, child: rows),
      );
    }
    return Material(
      color: colors.surface,
      elevation: 3,
      borderRadius: radius,
      clipBehavior: Clip.antiAlias,
      child: rows,
    );
  }
}

/// One row of the banner: [content]'s icon, then [distance] above its text.
class _Row extends StatelessWidget {
  const _Row({
    required this.content,
    this.iconSize = ManeuverBanner.iconSize,
    required this.distance,
    required this.distanceStyle,
    required this.textStyle,
  });

  final NavInstructionContent content;

  /// The icon's size; its column is [ManeuverBanner.iconSize] wide whatever it is.
  final double iconSize;
  final String? distance;
  final TextStyle? distanceStyle;
  final TextStyle? textStyle;

  @override
  Widget build(BuildContext context) {
    final icon = maneuverIcon(content.maneuverType, content.maneuverModifier);
    final distance = this.distance;
    return Row(
      children: [
        // The column stays without an icon, as Ferrostar's does, so the text lines up
        // from one maneuver to the next.
        SizedBox(
          width: ManeuverBanner.iconSize,
          child: icon == null
              ? null
              : Icon(
                  icon,
                  size: iconSize,
                  color: Theme.of(context).colorScheme.onSurface,
                ),
        ),
        const SizedBox(width: ManeuverBanner._gap),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (distance != null) Text(distance, style: distanceStyle),
              Text(
                content.text,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: textStyle,
              ),
            ],
          ),
        ),
      ],
    );
  }
}
