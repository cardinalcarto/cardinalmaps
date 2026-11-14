# cardinal

A new Flutter project.

## Development defines

Servers come from `--dart-define`s, as in `dart_defines.example.json`; `lib/config.dart`
says what each one does. For development there are also:

- `CARDINAL_SIMULATE_NAVIGATION=true`: navigation drives a simulated trip along the
  route, at a typical speed for the mode, instead of following the device's location, so
  it can be tried on the Linux desktop or at a desk. Off by default. It's honored in any
  build mode, since the desktop is run as a profile build.

On Linux, `flutter_tts` has no speech engine, so navigation's announcements are logged
as `cardinal-speech: <text>` instead of spoken.

## Navigation screenshots

`tool/navigation_screenshots.sh OUT_DIR --bucket BUCKET_DIR` takes screenshots of the
navigation screen in a simulated drive through downtown Seattle, for reviewing how it
looks: the maneuver banner with and without a secondary row, landscape, a roundabout if
the drive has one, the recenter button, and the dark theme. It needs the Seattle region
downloaded in the desktop app; `BUCKET_DIR` is a `cardinal-publish --target dir:`
release bucket, served locally as the app's release URL. `--shots progress`, `buttons`,
`steps`, `status` or `guidance` takes another set instead: trip progress, the road name
and map buttons, the banner's list of steps, the status cards, the GPS-lost indicator
and the arrival, made by steering the simulated trip from the Dart file, or lane
guidance, exit badges and the speed-limit sign on I-5 and in Wallingford.

It builds a profile build for the Linux desktop and runs
`tool/screenshots/navigation_screenshots.dart` in it with `flutter drive -d linux`, and
refuses to run anywhere else: don't run that file with `flutter test` or on a phone.
Nothing is tapped or typed: the app's own code opens the screens, starts the trip and
moves the map. The screenshots are taken in the app, not of the desktop, in a 412 × 915
view at a device pixel ratio of 2. The one place it adds to the app's store is deleted
by its ID at the end.

## Third-party notices

See `NOTICE.md`, including the maneuver icons built into `assets/fonts/CardinalIcons.ttf`
by `tool/icon_font.py`.
