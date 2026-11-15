# Turn-by-turn navigation: first build and run on iOS

Issue: cardinalcarto/cardinalmaps#12. The `navigation` branch is merged into `main`, so this
was run from `main`. Flutter stable 3.47.9, Xcode 27.2, iOS simulator (iPhone 18 Pro,
iOS 27.2). Not yet run on a physical iPhone.

## What broke, and what changed

- **Navigation's location settings were thrown away on iOS** (fixed). `geolocator_apple`
  keeps a single position stream per app and hands it to every later caller, whatever
  settings that caller asks for. This is the trap `NavigationFixes.kt` already avoids
  on Android. The map's location dot opens that stream first, with plain
  `LocationSettings(distanceFilter: 2)`, so `GeolocatorNavigationSource`'s
  `AppleSettings` (best-for-navigation accuracy, no distance filter, automotive activity
  type, no automatic pausing, background updates allowed) never reached Core Location.
  The log shows it: with the old code the app's only location manager has
  `activityType` 1 (other); with the fix there is also navigation's own manager with
  `activityType` 2 (automotive navigation). The dot's manager also has
  `allowsBackgroundLocationUpdates` off, so on a phone, fixes would be expected to stop
  with the screen off. The simulator kept delivering fixes to the old build in the
  background, so that part needs checking on a device.

  Fix: navigation has its own Core Location manager in native code, like Android.
  `ios/Runner/NavigationFixes.swift` serves the same `cardinal/navigation/fixes` channel,
  with the same map keys and the same `off` / `denied` / `failed` errors as
  `NavigationFixes.kt`, and is registered in `AppDelegate.swift`. On the Dart side,
  `AndroidNavigationSource` is now `PlatformNavigationSource`, used on Android and iOS;
  it also passes the trip's mode, which iOS turns into the activity type (Android
  ignores it). `GeolocatorNavigationSource` is left for macOS and the Linux desktop.
- **`flutter_tts` was missing from `ios/Podfile.lock`** (fixed). It came with navigation
  but iOS had never been built since; `pod install` added it.

## What worked

- The build: the Rust core (Ferrostar and the router, through Cargokit) builds for
  `aarch64-apple-ios-sim` with nothing changed (about four minutes the first time).
- The permission prompt shows the new text: "Cardinal shows where you are on the map
  and guides you turn by turn while you navigate, including with the screen off."
- Offline routing on the device: a downtown → Fremont Troll drive (3.0 mi, 6 min)
  routed in about a second, after downloading a central Seattle area.
- **Simulated trip** (`CARDINAL_SIMULATE_NAVIGATION=true`), start to end: maneuver
  banner with lanes, speed limit, road name, ETA bar, announcements spoken (the iOS
  speech engine loads; the audio category is set in `speech.dart`), and "You've
  arrived". No Dart errors in the log.
- **Location-service trip** (simulator location replayed along Aurora Avenue with
  `xcrun simctl location start`), start to end on the new native source: fixes once a
  second, off-route detected, two reroutes (about 90–190 ms each), arrival. With the app
  in the background for 20 s mid-trip, navigation's manager kept receiving fixes every
  second, and the trip had moved on when the app came back. Ending the trip stops
  navigation's manager; only the dot's carries on.

## Not checked

- A physical iPhone: real GPS, the screen locked, spoken announcements on the
  speaker and over Bluetooth, and whether other audio ducks and comes back.
- Denying location, or turning location services off mid-trip, on iOS (the error paths
  in `NavigationFixes.swift` mirror Android's but haven't been run).

## Seen in passing, not navigation

- Typing into search right after tapping the field can be dropped in the simulator.
  The text arrives once the field has focus. This is probably the simulator's input,
  not the app.
- "nearby transit: PathNotFoundException" is logged at startup when no transit has
  been downloaded.
