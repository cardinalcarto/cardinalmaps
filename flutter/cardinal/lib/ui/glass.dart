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

/// How far toward tinted the app's glass is on iOS 27's Liquid Glass slider, from 0
/// (clear) to 1 (tinted): partway, so chrome over a busy map stays legible.
const double appGlassTint = 0.5;

/// iOS 27's regular glass, for toolbars, tab bars and buttons, in the theme's
/// brightness, [tintAmount] of the way to tinted.
LiquidGlassSettings appGlassSettings(
  BuildContext context, {
  double tintAmount = appGlassTint,
}) => LiquidGlassSettings.ios27Regular(
  brightness: Theme.of(context).brightness,
  tintAmount: tintAmount,
);

LiquidGlassAppearance _appearance(BuildContext context) =>
    LiquidGlassAppearance.ios27Regular(
      brightness: Theme.of(context).brightness,
    );

/// A layer for the [GlassSurface]s in [child] to share, sampling what's behind them
/// once. Without glass it's just [child].
class AppGlassLayer extends StatelessWidget {
  const AppGlassLayer({
    super.key,
    required this.child,
    this.tintAmount = appGlassTint,
  });

  final Widget child;

  /// How far toward tinted its glass is; see [appGlassTint].
  final double tintAmount;

  @override
  Widget build(BuildContext context) {
    if (!glassEnabledOf(context)) return child;
    return LiquidGlassLayer(
      settings: appGlassSettings(context, tintAmount: tintAmount),
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
  const GlassSurface({
    super.key,
    required this.child,
    this.radius = 24,
    this.floating = false,
  });

  final Widget child;
  final double radius;

  /// Whether it floats over the map, with a shadow to lift it off whatever's below.
  final bool floating;

  static const _floatingShadows = [
    BoxShadow(color: Color(0x29000000), blurRadius: 16, offset: Offset(0, 4)),
    BoxShadow(color: Color(0x14000000), blurRadius: 2, offset: Offset(0, 1)),
  ];

  @override
  Widget build(BuildContext context) {
    final corners = BorderRadius.circular(radius);
    if (glassEnabledOf(context)) {
      return LiquidGlass.auto(
        shape: LiquidRoundedSuperellipse(borderRadius: radius),
        settings: appGlassSettings(context),
        appearance: _appearance(context),
        shadows: floating ? _floatingShadows : const [],
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

/// The key of the sheet's glass, for tests to tell it from the rest.
const sheetGlassKey = Key('sheet glass');

/// How far toward tinted the sheet's glass is; see [appGlassTint].
const double sheetGlassTint = 0.7;

/// How much of the sheet's color lies over its glass while it peeks: enough for text
/// over any map, little enough for the map's colors to read through the blur.
const double sheetGlassWash = 0.5;

/// The sheet's glass, for `SheetScaffold.backdrop`: at full strength while the sheet
/// shows any of it, under the sheet's color fading in as it rises, so the sheet only
/// ever gets more opaque. (Fading the glass out as the color faded in left the sheet
/// clearest halfway, with the map's labels showing through its text.) Null where glass
/// is off, so the sheet stays solid.
Widget Function(BuildContext context, double cornerRadius, double clarity)?
sheetGlassOf(BuildContext context) {
  if (!glassEnabledOf(context)) return null;
  return (context, cornerRadius, clarity) => AppGlassLayer(
    key: sheetGlassKey,
    tintAmount: sheetGlassTint,
    child: GlassSurface(
      radius: cornerRadius,
      // A thick material, as iOS gives sheets: the sheet's color over the glass, so
      // the map tints the sheet without coloring the text on it.
      child: ColoredBox(
        color: Theme.of(context).colorScheme.surfaceContainerLow
            .withValues(alpha: sheetGlassWash),
        child: const SizedBox.expand(),
      ),
    ),
  );
}
