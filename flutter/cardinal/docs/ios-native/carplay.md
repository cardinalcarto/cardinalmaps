# CarPlay: the phone's trip on the car's screen

A trip started on the phone shows on a connected car: the app's map with the route and
the chevron, the next maneuver and the trip's distance and time in CarPlay's own panels,
End, mute, and a button for the rest of the trip. Nothing is started from the car yet.

**Written without a Mac: none of this has been compiled, run in the CarPlay simulator, or
run through `dart format` and the tests.** The first build is the first check.

## How it fits together

```
phone's engine (main)                     native                      car's engine (carPlayMain)
─────────────────────                     ──────                      ──────────────────────────
NavigationController
  └ CarPlayLink ── cardinal/carplay ──▶ CarPlayBridge ── cardinal/carplay/screen ──▶ CarScreen
      map, route, trip                    │  keeps the latest of each                  └ CarMap
      ◀── connected, end, setMuted        ▼                                              (SparkleMap)
                                        CarPlaySceneDelegate
                                          CPMapTemplate, CPNavigationSession
```

- `lib/carplay/car_trip.dart` says what the car is told of the trip, worded as the
  Android notification is (`navigationNoticeOf`) and rounded as the banner is
  (`roundManeuverDistance`, split out of `formatManeuverDistance`).
- `lib/carplay/car_link.dart` sends it while a car is connected, and does what the car's
  End and mute ask.
- `ios/Runner/CarPlayBridge.swift` passes it on, to the templates and to the car's map.
- `ios/Runner/CarPlaySceneDelegate.swift` is the CarPlay scene: the map template, the
  navigation session, the buttons, and the engine for the map.
- `lib/carplay/car_screen.dart` and `car_map.dart` are the car's map, in an engine of its
  own, since Flutter on iOS shows one view an engine. It has no plugins, no store and no
  navigation: it draws what it's sent.

## To check, in this order

1. `dart format lib test` and `scripts/check flutter`. The Dart was laid out by hand.
2. It builds. The Swift uses `CPNavigationSession.PauseReason`, `CPBarButton(title:handler:)`
   and `MKPlacemark` (deprecated from iOS 26, a warning).
3. In the simulator (I/O ▸ External Displays ▸ CarPlay), Cardinal has an icon and opens
   to the map. The entitlement is set for simulator builds only (below).
4. **The second engine draws.** Sparkle hasn't run in two engines of one app before: the
   Rust library is loaded once and initialized from each isolate, and Flutter GPU has to
   be on for an engine made in code as it is for the storyboard's
   (`FLTEnableFlutterGPU`).
5. A simulated trip (`CARDINAL_SIMULATE_NAVIGATION=true`) shows on both screens: the
   maneuver and its icon, distances counting down in the banner's steps, a reroute, the
   arrival, End from either screen, mute from either.
6. **Lock the phone mid-trip: the car's map must keep moving.** Flutter stops drawing when
   it thinks the app is in the background, and whether it counts a CarPlay scene as
   foreground is the least certain thing here.
7. Connect the car with the app not running, then open the app on the phone. The car has
   no map until the phone's engine is up to say where the tiles are; if iOS doesn't start
   the phone's scene by itself, that needs a fix (starting the phone's engine without its
   scene).
8. The camera: where the chevron sits beside CarPlay's panels (`followPadding` with the
   view's safe area), and the chevron's size (`carPuckStyle`) on a small screen.
9. On an iPad, `UIApplicationSupportsMultipleScenes` is now on, as CarPlay needs: check
   that a second window can't be opened onto the one Flutter engine.

## The entitlement

`ios/Runner/Runner.entitlements` asks for `com.apple.developer.carplay-maps`, and the
project uses it for simulator builds only (`"CODE_SIGN_ENTITLEMENTS[sdk=iphonesimulator*]"`),
so device builds still sign while Apple hasn't granted it. A device build that asks for
it without the grant fails to sign ("Entitlement com.apple.developer.carplay-maps not
found and could not be included in profile"; tried 2026-11-28, Xcode 27.2). Once Apple
has granted it and the provisioning profile carries it, make the setting unconditional
in the three Runner configurations.

## Not done

- Starting a trip from the car: saved places, recents, search, route preview.
- The dashboard and instrument-cluster scenes (their own windows, so a map each). The
  navigation session's maneuvers already reach clusters and head-up displays that show
  them.
- Lane guidance and the banner's secondary line and exit badges; the speed limit.
- Panning the map from the car (`CPMapTemplate`'s pan buttons).
- The map's attribution, which the phone shows and the car's screen doesn't.
- Android Auto.
