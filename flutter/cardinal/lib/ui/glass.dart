/// Liquid Glass on iOS, with `liquid_glass_renderer`; opaque surfaces elsewhere.
///
/// Every glass layer copies what's behind it, at a cost that grows with its area, so
/// glass comes in small layers around the chrome that needs it, not one around the app.
/// Put glass that sits together in one [AppGlassLayer] to share that copy.
library;

import 'package:liquid_glass_renderer/liquid_glass_renderer.dart';
import 'package:material_ui/material_ui.dart';

import 'adaptive.dart';

/// Whether glass draws: on iOS, unless more contrast is asked for, Flutter's nearest
/// to iOS's Reduce Transparency.
bool glassEnabledOf(BuildContext context) =>
    isCupertinoOf(context) && !MediaQuery.highContrastOf(context);

/// iOS 27's regular glass, for toolbars, tab bars and buttons, in the theme's
/// brightness.
LiquidGlassSettings appGlassSettings(BuildContext context) =>
    LiquidGlassSettings.ios27Regular(brightness: Theme.of(context).brightness);

LiquidGlassAppearance _appearance(BuildContext context) =>
    LiquidGlassAppearance.ios27Regular(
      brightness: Theme.of(context).brightness,
    );

/// A layer for the [GlassSurface]s in [child] to share, sampling what's behind them
/// once. Without glass it's just [child].
class AppGlassLayer extends StatelessWidget {
  const AppGlassLayer({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    if (!glassEnabledOf(context)) return child;
    return LiquidGlassLayer(
      settings: appGlassSettings(context),
      defaultAppearance: _appearance(context),
      child: child,
    );
  }
}

/// A surface for chrome over the map: glass with rounded corners of [radius] on iOS,
/// an elevated surface elsewhere or when glass is off. [child] paints on top.
///
/// It uses the [AppGlassLayer] above it, if any, or a layer of its own.
class GlassSurface extends StatelessWidget {
  const GlassSurface({super.key, required this.child, this.radius = 24});

  final Widget child;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final corners = BorderRadius.circular(radius);
    if (glassEnabledOf(context)) {
      return LiquidGlass.auto(
        shape: LiquidRoundedSuperellipse(borderRadius: radius),
        settings: appGlassSettings(context),
        appearance: _appearance(context),
        child: child,
      );
    }
    final colors = Theme.of(context).colorScheme;
    return Material(
      color: colors.surfaceContainerHigh,
      shadowColor: colors.shadow,
      elevation: 3,
      shape: RoundedSuperellipseBorder(borderRadius: corners),
      clipBehavior: Clip.antiAlias,
      child: child,
    );
  }
}
