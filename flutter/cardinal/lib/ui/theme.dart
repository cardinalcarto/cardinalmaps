import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoThemeData;
import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:material_ui/material_ui.dart';

/// The app's accent, on every platform: the primary color of the blue-grey seed the
/// app started with.
const _seed = Colors.blueGrey;

/// Whether [platform] gets the iOS look: Cupertino controls, iOS type and colors.
bool isCupertino(TargetPlatform platform) =>
    platform == TargetPlatform.iOS || platform == TargetPlatform.macOS;

/// The app's theme for [brightness] on [platform].
///
/// On Android and the desktop it's Material 3 from the seed. On iOS and macOS it's the
/// same accent over the system's colors, with its text styles, and without ink
/// ripples, so Material widgets that stay look at home until they're replaced.
ThemeData appTheme(Brightness brightness, {TargetPlatform? platform}) =>
    _appTheme(brightness, platform ?? defaultTargetPlatform);

ThemeData _appTheme(Brightness brightness, TargetPlatform platform) {
  final seeded = ColorScheme.fromSeed(seedColor: _seed, brightness: brightness);
  if (!isCupertino(platform)) {
    return ThemeData(colorScheme: seeded, platform: platform);
  }
  final colors = _iosColors(seeded, brightness);
  final base = ThemeData(colorScheme: colors, platform: platform);
  final dark = brightness == Brightness.dark;
  return base.copyWith(
    textTheme: _iosText(base.textTheme),
    primaryTextTheme: _iosText(base.primaryTextTheme),
    // iOS highlights what's pressed and doesn't ripple.
    splashFactory: NoSplash.splashFactory,
    highlightColor: dark ? const Color(0x24FFFFFF) : const Color(0x14000000),
    dividerTheme: DividerThemeData(
      thickness: 0.5,
      space: 0.5,
      color: colors.outlineVariant,
    ),
    cupertinoOverrideTheme: CupertinoThemeData(
      brightness: brightness,
      primaryColor: colors.primary,
    ),
  );
}

/// The system's colors (UIColor's semantic colors) around [seeded]'s accent.
ColorScheme _iosColors(ColorScheme seeded, Brightness brightness) =>
    switch (brightness) {
      Brightness.light => seeded.copyWith(
        // systemBackground, then secondary and tertiary system fills over it.
        surface: const Color(0xFFFFFFFF),
        onSurface: const Color(0xFF000000),
        onSurfaceVariant: const Color(0x993C3C43), // secondaryLabel
        surfaceContainerLowest: const Color(0xFFFFFFFF),
        surfaceContainerLow: const Color(0xFFF2F2F7), // systemGroupedBackground
        surfaceContainer: const Color(0xFFF2F2F7),
        surfaceContainerHigh: const Color(0xFFE5E5EA), // systemGray5
        surfaceContainerHighest: const Color(0xFFD1D1D6), // systemGray4
        outline: const Color(0x4A3C3C43), // separator
        outlineVariant: const Color(0x293C3C43),
        error: const Color(0xFFFF3B30), // systemRed
      ),
      Brightness.dark => seeded.copyWith(
        surface: const Color(0xFF000000),
        onSurface: const Color(0xFFFFFFFF),
        onSurfaceVariant: const Color(0x99EBEBF5),
        surfaceContainerLowest: const Color(0xFF000000),
        surfaceContainerLow: const Color(0xFF1C1C1E),
        surfaceContainer: const Color(0xFF1C1C1E),
        surfaceContainerHigh: const Color(0xFF2C2C2E),
        surfaceContainerHighest: const Color(0xFF3A3A3C),
        outline: const Color(0x99545458),
        outlineVariant: const Color(0x66545458),
        error: const Color(0xFFFF453A),
      ),
    };

/// One of iOS's text styles: its size, weight, tracking and line height.
typedef _IosStyle = (
  double size,
  FontWeight weight,
  double tracking,
  double line,
);

/// The iOS text styles (Human Interface Guidelines, Typography) standing in for each
/// Material one, by what it's used for rather than by size.
const _iosStyles = <String, _IosStyle>{
  'displayLarge': (34, FontWeight.w700, 0.40, 41), // Large Title
  'displayMedium': (34, FontWeight.w700, 0.40, 41),
  'displaySmall': (34, FontWeight.w700, 0.40, 41),
  'headlineLarge': (34, FontWeight.w700, 0.40, 41),
  'headlineMedium': (28, FontWeight.w700, 0.38, 34), // Title 1
  'headlineSmall': (28, FontWeight.w700, 0.38, 34), // page titles
  'titleLarge': (20, FontWeight.w600, -0.45, 25), // Title 3
  'titleMedium': (17, FontWeight.w600, -0.43, 22), // Headline
  'titleSmall': (15, FontWeight.w600, -0.23, 20),
  'bodyLarge': (17, FontWeight.w400, -0.43, 22), // Body
  'bodyMedium': (15, FontWeight.w400, -0.23, 20), // Subheadline
  'bodySmall': (13, FontWeight.w400, -0.08, 18), // Footnote
  'labelLarge': (15, FontWeight.w600, -0.23, 20), // buttons
  'labelMedium': (12, FontWeight.w500, 0, 16), // Caption 1
  'labelSmall': (11, FontWeight.w500, 0.06, 13), // Caption 2
};

TextTheme _iosText(TextTheme t) {
  TextStyle? ios(TextStyle? style, String name) {
    final (size, weight, tracking, line) = _iosStyles[name]!;
    return style?.copyWith(
      fontSize: size,
      fontWeight: weight,
      letterSpacing: tracking,
      height: line / size,
    );
  }

  return t.copyWith(
    displayLarge: ios(t.displayLarge, 'displayLarge'),
    displayMedium: ios(t.displayMedium, 'displayMedium'),
    displaySmall: ios(t.displaySmall, 'displaySmall'),
    headlineLarge: ios(t.headlineLarge, 'headlineLarge'),
    headlineMedium: ios(t.headlineMedium, 'headlineMedium'),
    headlineSmall: ios(t.headlineSmall, 'headlineSmall'),
    titleLarge: ios(t.titleLarge, 'titleLarge'),
    titleMedium: ios(t.titleMedium, 'titleMedium'),
    titleSmall: ios(t.titleSmall, 'titleSmall'),
    bodyLarge: ios(t.bodyLarge, 'bodyLarge'),
    bodyMedium: ios(t.bodyMedium, 'bodyMedium'),
    bodySmall: ios(t.bodySmall, 'bodySmall'),
    labelLarge: ios(t.labelLarge, 'labelLarge'),
    labelMedium: ios(t.labelMedium, 'labelMedium'),
    labelSmall: ios(t.labelSmall, 'labelSmall'),
  );
}
