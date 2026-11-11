# iOS native look — dark map

Branch: `feature/ios-style`. Flutter stable 3.47.9.

## What changed

On both platforms:

- **Dark streets style** (`assets/map/headway-dark.json`, linking to
  `crates/sparkle_harness/styles/headway-dark.json`): made from the light Headway style by
  `tool/dark_style.dart`. Rerun `dart run tool/dark_style.dart` whenever the light style
  changes; a test fails if the bundled dark style is stale. Colors are remapped in OKLab
  by what they paint:
  - ground: a dark, slightly cool grey; other land fills keep their differences,
    compressed and muted;
  - forests: a vivid green, nearly opaque; grassland and meadows a softer, yellower
    green a step below; city parks and gardens (landcover `grass` with subclass
    `park`, `garden` and the like) between the two. The low-zoom landcover uses the
    same colors, so zooming in doesn't change them;
  - protected areas (the `park` layer, which in planetiler-openmaptiles is national
    parks to conservation easements, not city parks): a faint grey-green tint and
    their outline, since they cover whole deserts and ranges and overlap;
  - desert scrub (landcover `grass` with subclass `scrub`, `heath`, `tundra` and the
    like, all green in the light style): a dusty tan; sand a lighter tan;
  - water: a mid blue, lighter than the land; streams a lighter blue, as
    thin lines;
  - glaciers, ice and low-zoom snow: a bright, cool grey;
  - roads: lighter than the ground, motorways lightest and trunks next (the light
    style tells motorways apart by dark casings, which go to the ground here);
    casings just under the ground;
  - 3D buildings: just above the ground, so streets stay the lightest thing on a
    block;
  - labels light, keeping their hue; halos dark;
  - highway shields unchanged (their text is drawn on the light shield sprites);
  - the low-zoom relief raster dimmed; the pedestrian-area pattern a flat fill.
- **The map follows the app's theme** (`map/map.dart`, `map/layers.dart`): `MapLayers`
  has `dark`, and streets loads `darkMapStyle` (`CARDINAL_DARK_STYLE`, default
  `headway-dark.json`) when the app is dark. It switches live with the system setting.
  Topo has no dark style yet and stays light.
- This fixes dark mode's status bar over the light map (phases 2 and 3's known issue).

## Checked on this machine (iOS simulator, iPhone 18 Pro, iOS 27.2)

- `flutter analyze` clean; `flutter test` 378 passing, including the new
  `test/map/layers_test.dart` cases (dark streets, topo unchanged, dark style up to
  date).
- Dark mode at world zoom; the Bay Area at regional zoom (water, woods, motorway
  network); suburban Sunnyvale at z13 and z17 (parks, creeks, buildings, POIs);
  Portland at z15 (rail, US shields); Death Valley (national park, woodland, sand),
  the Mojave by Baker (sand, scrub) and Shoshone at street zoom; Emmons Glacier on
  Mount Rainier. The selection pin and area outline
  in dark. Switching the system appearance swaps the map's style.

## Known issues

- POI icons are the light sprites, white discs that are bright on the dark map. A
  dark sprite sheet would fix it.
- Topo has no dark style.
- Rounded building corners still aren't drawn (sparkle ignores
  `fill-extrusion-rounded-corner-distance`).

## For the Android parity check

Expected: **the map goes dark when the system is dark**, as on iOS, with the same
style. Nothing else on Android changes.

Please check, on an Android device or emulator:

1. `flutter analyze` and `flutter test` pass.
2. With the system in dark mode, the streets map is dark; switching the system theme
   while the app is open swaps the map between light and dark.
3. Topo stays light in dark mode.
4. A city at street zoom, a park, water and a motorway read clearly in dark, as in
   the checks above; nothing renders as the light style's colors.

## Android results

_(To be filled in by the Android session.)_
