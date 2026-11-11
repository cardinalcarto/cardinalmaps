# iOS native look — phase 1: foundation

Branch: `ios-native-foundation` (from `flutter-stable`). Flutter stable 3.47.9 / Dart 3.13.7.

## What changed

- **Dependencies:** `cupertino_ui ^1.1.4` (already pulled in by `material_ui`),
  `cupertino_icons ^1.0.8`, `liquid_glass_renderer 1.0.0-dev.2` (pinned: prerelease;
  its Dart constraint `<3.14` will need bumping before a Flutter upgrade).
- **Theme** (`lib/ui/theme.dart`, used by `main.dart`): `appTheme(brightness, {platform})`.
  - Android and desktop: unchanged Material 3 from the blue-grey seed.
  - iOS and macOS: same accent over iOS system colors; iOS text styles with tracking
    and line heights; no ink ripple (highlight only); hairline dividers; Cupertino
    widgets get the accent.
- **Building blocks**, not yet used by screens (phases 2 and 3 adopt them):
  - `lib/ui/adaptive.dart`: `AppButton` (filled / tinted / plain), `AppListSection` +
    `AppListTile`, `AppSegmentedControl`, `showAppDialog`. Cupertino on iOS, Material
    elsewhere, chosen by the theme's platform.
  - `lib/ui/glass.dart`: `GlassSurface`, `AppGlassLayer`, `glassEnabledOf`. Glass on
    iOS only; Android, and iOS with Increase Contrast, get an elevated opaque surface.
  - `lib/ui/symbols.dart`: `AppSymbols.of(context)` for common action icons.
  - `lib/ui/haptics.dart`: `Haptics.selection/impact/success`.

## Checked on this machine (iOS simulator, iPhone 18 Pro, iOS 27.2)

- `flutter analyze` clean; `flutter test` 359 passing, including new `test/ui/` tests
  that run every building block under both iOS and Android.
- Home, search sheet and Saved page render with the iOS type and colors, light and dark.
- **Startup:** awaiting nothing, `LiquidGlass.precache()` at launch still delayed the
  first frame (25.4 s vs 12.8 s, debug build on the simulator, one run each), so it was
  removed. Revisit in phase 2, after the first frame.

## For the Android parity check

Expected: **no visible change on Android.** The Android theme is the same `ThemeData`
as before, and no screen uses the new building blocks yet.

Please check, on an Android device or emulator:

1. `flutter analyze` and `flutter test` pass.
2. The app builds and launches (new dependencies: `liquid_glass_renderer`,
   `cupertino_ui`, `cupertino_icons`; Flutter GPU is already on in the manifest).
3. Home, search, a place sheet, Saved, layers and directions look as they did on
   `flutter-stable`: Material type, ripples, colors.
4. Note anything in the logs from `liquid_glass_renderer` at startup.

## Android results

_(To be filled in by the Android session.)_
