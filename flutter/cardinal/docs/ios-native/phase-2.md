# iOS native look — phase 2: glass chrome

Branch: `ios-native-chrome` (from `ios-native-foundation`). Flutter stable 3.47.9.

## What changed

On iOS only (the theme's platform), unless noted:

- **Map controls** (`map/buttons.dart`, new `map/map_control.dart`): Layers and My
  location share one floating glass capsule, more tinted than other glass and with a
  shadow so it stands out from the map. Android keeps its two floating action buttons.
- **Tab bar** (`home/navigation_bar.dart`): a floating glass capsule of five labeled
  tabs, Search in the middle, with the current page tinted on a lens. It sits low, into
  the home indicator's margin, and content scrolls under it. Android keeps its bar.
- **Sheet** (`sheet/`): `SheetScaffold.backdrop` draws glass behind the sheet. The
  sheet is glass while it peeks (up to a third of the screen), turns solid by halfway
  (`SheetGeometry.solidity`), and draws no glass once solid. The glass carries a 70%
  wash of the sheet color, a thick material as iOS gives sheets. Pages paint no
  background of their own over a backdrop; `SheetView.surfaceColor` gives pinned
  headers the sheet's color at its current solidity (used by the search bar's header).
- **Messages** (`saved/saved_feedback.dart`, new `ui/toast.dart`): a glass toast at the
  top of the screen, under the status bar, with its action; it goes after 3 s (5 s with
  an action). Android keeps its snack bar. `SheetScaffold.maybeViewOf` lets messages
  show without a sheet.
- **Location button** (both platforms): its messages go through `showSheetMessage` and
  its first-launch prompt through `showAppDialog` (Cupertino alert on iOS).

## Checked on this machine (iOS simulator, iPhone 18 Pro, iOS 27.2)

- `flutter analyze` clean; `flutter test` 369 passing, including `test/ui/chrome_test.dart`
  (tab bar, sheet glass and solidity, toast, map control, on iOS and Android).
- Home, search results, a place sheet, Saved, save and undo with toasts, light and dark.
- Glass refracts the map; content scrolls under the tab bar and its last row clears it.
- Cold start, engine to first map frame: 5.0 s with glass on screen (one run; phase 1
  measured 12.8 s without), so glass on the first screen doesn't hold startup up. The
  phase 1 plan to preload shaders after the first frame was dropped: glass on the first
  screen loads them at once.

## Known issues

- In dark mode the status bar turns light over the light map. The map has no dark style
  yet (phase 4).
- Glass cost on a real device is unmeasured: the simulator renders on the Mac's GPU.
  Pans redraw the map, and the sheet, tab bar and map controls each re-read it.

## For the Android parity check

Expected: **Android looks as it did**, except that the location button's first-launch
prompt is now built by `showAppDialog` (still a Material alert, same text and buttons).

Please check, on an Android device or emulator:

1. `flutter analyze` and `flutter test` pass.
2. Map buttons are the two floating action buttons; the bottom bar is the Material row
   with Search filled; the sheet is opaque at every height, including while it peeks.
3. Saving a place shows the snack bar above the sheet, with Move… / Undo working.
4. On first launch with location not yet allowed, the "Enable location?" prompt shows
   and No / Yes behave as before.
5. Sheet page transitions (search → place → directions and back) don't show the page
   below through the new page.

## Android results

_(To be filled in by the Android session.)_
