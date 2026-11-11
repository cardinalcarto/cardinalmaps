# iOS native look — dark topo, and no winter mode

Branch: `feature/ios-style`. Flutter stable 3.47.9.

## What changed

On both platforms:

- **Winter mode is gone.** Winter will be a style of its own later. Removed: the
  `map.winter` setting, `MapLayers.winter`, the Winter switch (iOS) and snowflake
  button (Android) in the layer picker, the topo style's `winter` global state, and its
  two layers drawn only in winter, `piste-line` and `piste-label`. Ski areas'
  tint (`winter-sports`) stays: it was always drawn, not part of winter mode.
- **The topo asset is a link**: `assets/map/cardinal-topo.json` was a copy of
  `crates/sparkle_harness/styles/cardinal-topo.json`; it now links to it, as
  `headway.json` does, so the two can't drift.
- **Dark topo style** (`assets/map/cardinal-topo-dark.json`, linking into
  `crates/sparkle_harness/styles/`): made by `tool/dark_topo_style.dart`. Topo is for
  finding your way, so legibility comes first and every color is chosen by hand in a
  palette keyed by the light color: trails a bright red, roads saturated on dark
  casings, contours a light brown, tracks and paths light, water a mid blue with
  lighter streams and names, labels light on dark halos; landcover mid-dark, told apart
  by hue, so nothing large glares. The script stops on a color the palette doesn't
  cover, and a test fails if the bundled dark style is stale.
  - Peaks get a light disc (`peak-marker`) under their triangle: the triangle is a dark
    sprite the style can't recolor.
  - The park tint stays faint (it lies over forest, and lifted it under the contours);
    faint outlines and danger areas are a little stronger.
- **Topo follows the app's theme** like streets: `darkTopoStyle` when the app is dark.
  Contour units still apply.

- **Stopping a search clears it** (both platforms): back while searching, or Cancel on
  iOS, now empties the query and its results, as iOS's own search fields do. Before, it
  kept them under the bar, where dragging the sheet scrolled them instead of raising the
  sheet.

## Checked on this machine (iOS simulator, iPhone 18 Pro, iOS 27.2)

- `flutter analyze` clean; `flutter test` 379 passing, including the dark topo cases in
  `test/map/layers_test.dart` and the picker tests without winter.
- The layer picker has no Winter row. Dark topo at Lake Tahoe (regional: Tahoe Rim
  Trail, roads, water, peaks), Half Dome at z14 (dense contours and their labels, the
  cables trail, peaks and elevations) and Yosemite Valley (trail names, streams,
  Mirror Lake, viewpoint POIs).

## For the Android parity check

Expected: **the layer picker loses its snowflake button**, and **topo is dark when the
system is dark**. Nothing else changes.

Please check, on an Android device or emulator:

1. `flutter analyze` and `flutter test` pass.
2. The layer picker's Topo row has the ft / m control and no winter button; choosing a
   unit still chooses Topo.
3. With the system dark, Topo is dark; switching the system theme swaps it. Contours
   follow ft / m in both.
4. In a mountain area at z13–15, contours and their labels, trails, peaks (with their
   light markers), streams and POIs all read clearly.
5. Search, then press back: the search bar is empty and no results are left under it;
   dragging the sheet up doesn't scroll anything.

## Android results

_(To be filled in by the Android session.)_
