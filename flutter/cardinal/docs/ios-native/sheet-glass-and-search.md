# iOS native look — sheet glass and search results

Branch: `ios-native-sheet-glass` (from `ios-native-screens`). Flutter stable 3.47.9.

## What changed

On iOS only:

- **Sheet glass** (`ui/glass.dart`): thinner while the sheet peeks, so the map's colors
  read through the blur as on iOS. The sheet's color over the glass went from 70% to
  50% (`sheetGlassWash`), and the glass from 0.9 to 0.7 of the way to tinted
  (`sheetGlassTint`). Chosen from a sweep over streets, forest and water on the
  simulator; text stayed readable over each. The sheet still turns solid by halfway.
- **Search results** (`search/search_sheet.dart`): a plain list straight on the sheet,
  as iOS lists what a search finds, with separators starting at the text. Icons are a
  soft tint: the accent for places, grey for addresses, streets and areas. Each address
  starts with how far it is from you, when you've allowed location already (the search
  doesn't ask). Rows have more room. The results keep the geocoder's order.
- **Directions** (`directions/directions_page.dart`): the disabled Go button is a grey
  label on a light fill, where it was white on pale grey and hard to see.
- `AppListTile` takes an optional `padding`.

## Checked on this machine (iOS simulator, iPhone 18 Pro, iOS 27.2)

- `flutter analyze` clean; `flutter test` 376 passing.
- The sheet at rest over streets, forest, and ocean beside forest; directions at its
  resting and lowest heights (with canned routes for the session, not committed);
  search results with distances.

## For the Android parity check

Expected: **no change on Android.** Nothing here runs on Android except
`AppListTile`'s new `padding`, which defaults to the platform's own (`ListTile`'s
`contentPadding` null, as before).

Please check `flutter analyze` and `flutter test` pass, and that search results, the
place card and directions look as on `ios-native-screens`.

## Android results

Checked on 2026-11-11 on a Moto G Play 2024 (Adreno, Impeller falling back from Vulkan
to OpenGL ES), Flutter stable 3.47.9, profile build with `dart_defines.json`, on
`feature/ios-style` at `fbc5bd8`.

1. `flutter analyze`: no issues, in the app and in `flutter/sparkle`. `flutter test`:
   all 376 pass.
2. Search results, the place card and directions look as on `ios-native-screens`:
   Material result cards with no distances, Directions filled and Save outlined, and the
   same directions sheet ("Couldn't get directions"; no router is reachable, unrelated
   to this branch).

This branch also has two commits this doc doesn't cover, and both run on Android:

- **Headway v4 streets style** (`3c1b183`): the app bundles it (`assets/map/headway.json`
  links to it), so Android's map changes as intended: grey roads, the new route shields
  (I-5, Washington's silhouette for SR 99), and POIs at high zoom. It loads and draws.
- **A command buffer per render pass** (`f0b253e`): shared renderer code. The map draws
  correctly at every zoom tried, including buildings at high zoom (top-down, not
  pitched), with no renderer errors in the log. Frame cost on this GPU-bound device
  wasn't measured.
