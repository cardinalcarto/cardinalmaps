# iOS native look — phase 3: screens

Branch: `ios-native-screens` (from `ios-native-chrome`). Flutter stable 3.47.9.

## What changed

On iOS only (the theme's platform), unless noted:

- **Search** (`search/search_sheet.dart`): `CupertinoSearchTextField` with a Cancel button
  while searching (Cancel does what back does: stops searching, keeps the query). Results
  are one inset grouped list, each with an icon for its kind. A spinner while loading;
  pinned places as tinted capsules.
- **Place card** (`place/place_page.dart`): the close button is a grey circle; Directions
  and Save are a row of equal buttons across the card, icon over label. A spinner while
  loading.
- **Saved** (`saved/`): iOS icons; selection circles instead of checkboxes in edit mode;
  the Move / Copy / Delete bar on floating glass, Delete in red. The "Saved in" and
  "Move to" pickers are iOS sheets headed by Cancel, title and Done (or the action);
  rows are grouped with checkmarks, the filter is an iOS search field. New list is an
  iOS alert; editing an item is a form sheet (`showFormDialog`). The saved layers sheet
  has iOS selection circles (a minus when a list is partly on).
- **Layers** (`map/layers.dart`): grouped rows: Streets / Topo with a checkmark; Topo's
  contours as a segmented control and Winter as a switch (either still chooses Topo);
  Saved places with a chevron. Done in the bar.
- **Directions** (`directions/directions_page.dart`): From and To in one grouped card with
  markers and clear buttons, Recalculate and Swap beside it; travel modes as a segmented
  control (Transit disabled); the summary as the time large with the distance under it
  and a green Go (disabled until navigation exists); steps as a grouped list. Choosing an
  end has an iOS search field with Cancel, and its choices grouped.
- **Shared** (`ui/`): `AppCloseButton`, `AppActionButton`, `AppSheetBar`,
  `showAppSheet`, `groupedRowColor`; grouped list headers and footers in iOS's footnote
  style; iOS sheet styling in the theme (corners, grabber, background).
- **Sheet glass, both platforms' code:** the glass now stays at full strength until the
  sheet is solid, instead of fading out as the sheet's color fades in. The two fading
  together left the sheet clearest partway up, with map labels showing through text.

## Checked on this machine (iOS simulator, iPhone 18 Pro, iOS 27.2)

- `flutter analyze` clean; `flutter test` 376 passing, including `test/ui/screens_test.dart`
  (search, place card, directions, layers, saved dialogs on iOS).
- Each screen above in light; layers and directions in dark. Directions was checked with
  canned routes plugged in for the session (the routing server isn't reachable from here);
  they aren't committed.

## Known issues

- Dark mode's status bar over the light map, still (phase 4: a dark map style).
- Directions has no turn icons: the routing data has no maneuver types yet, so steps are
  numbered.

## For the Android parity check

Expected: **Android looks as it did.** Changes there are limited to:

- The place card's and directions' close buttons are now `AppCloseButton`: on Android the
  same Material close icon button with the "Close" tooltip.
- The place card's buttons are now `AppActionButton`: on Android the same compact filled
  Directions and outlined Save buttons.
- Pickers open through `showAppSheet` (the same Material bottom sheet), and the edit form
  through `showFormDialog` (the same dialog). New list opens with `showAdaptiveDialog`
  (a Material alert on Android).
- Directions: when the route's summary or steps aren't there, nothing new appears.

Please check, on an Android device or emulator:

1. `flutter analyze` and `flutter test` pass.
2. Search, a place card, Saved (browse, edit mode, Move / Copy / Delete, New list, Edit),
   the layer picker and its Saved places sheet, and directions (with a reachable router,
   or the "Couldn't get directions" message) look and work as on `ios-native-chrome`.
3. The "Saved in" picker still has its Done button at the bottom, and the Move picker its
   "New list here" and "Move here" buttons.

## Android results

Checked on 2026-11-11 on a Moto G Play 2024, Flutter stable 3.47.9, profile build with
`dart_defines.json`.

1. `flutter analyze`: no issues. `flutter test`: all 376 pass.
2. Everything looks and works as on `ios-native-chrome`:
   - Search results and the place card: the same Material cards; Directions filled,
     Save outlined, the close icon button.
   - Directions: outlined From and To fields, the four-mode button row, "Couldn't get
     directions" (no router is reachable, unrelated to this branch). Choosing an end
     has the outlined field and result cards.
   - Saved: browsing, and edit mode with checkboxes and the floating Material
     Move / Copy / Delete bar (in code, Android keeps the same `Material` bar and icons;
     the red Delete is iOS only). New list is a Material alert. Edit opens the "Edit
     place" dialog, and its emoji saved. Copy, Move and Delete each worked, with Undo
     in their snack bars.
   - Layers: Streets, Topo with ft/m and the winter button, and Saved places; its
     "Saved places on the map" sheet has Material checkboxes.
3. The "Saved in" picker has Done at the bottom. The Move picker, and the Add and Copy
   pickers like it, have "New list here" and the action button ("Move here" etc.), with
   the source list greyed out.

The Saved checks used throwaway lists ("Parity check", "Parity copy", "Parity move") and
a place that wasn't saved before. All three lists were deleted afterward, leaving the
phone's saved places as they were.
