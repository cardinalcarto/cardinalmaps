import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';
import 'package:url_launcher/url_launcher.dart';

import '../l10n/app_localizations.dart';
import '../sheet/sheet.dart';
import '../ui/adaptive.dart';
import 'map.dart';
import 'map_control.dart';

/// The map's credits (see [SparkleAttribution]), in the bottom start corner, riding the
/// top of the sheet across from [MapButtons] and fading as they do. Its links open in
/// the browser.
class MapAttribution extends StatelessWidget {
  const MapAttribution({super.key});

  @override
  Widget build(BuildContext context) {
    final padding = MediaQuery.paddingOf(context);
    final scheme = Theme.of(context).colorScheme;
    final l10n = AppLocalizations.of(context);
    // Clear of the buttons at the end: a floating action button, or iOS's capsule.
    final buttons = isCupertinoOf(context) ? MapControl.size : 56.0;
    return SheetBuilder(
      child: SparkleAttribution(
        controller: MapScope.of(context),
        onLink: (link) => launchUrl(link, mode: LaunchMode.externalApplication),
        background: scheme.surfaceContainer.withValues(alpha: 0.85),
        foreground: scheme.onSurfaceVariant,
        showLabel: l10n.mapCreditsShow,
        hideLabel: l10n.mapCreditsHide,
      ),
      builder: (context, sheet, child) {
        final docked = sheet.geometry.placement == SheetPlacement.docked;
        final height = sheet.geometry.size.height;
        // As the buttons fade: see `MapButtons`.
        final opacity = docked
            ? 1.0
            : (1 - (sheet.extent - height * 0.45) / (height * 0.1)).clamp(
                0.0,
                1.0,
              );
        return Stack(
          children: [
            PositionedDirectional(
              start: 16 + padding.left,
              end: 16 + buttons + 16 + padding.right,
              bottom: 16 + (docked ? padding.bottom : sheet.obscured.bottom),
              child: IgnorePointer(
                ignoring: opacity == 0,
                child: Opacity(
                  opacity: opacity,
                  child: Align(
                    alignment: AlignmentDirectional.bottomStart,
                    child: child,
                  ),
                ),
              ),
            ),
          ],
        );
      },
    );
  }
}
