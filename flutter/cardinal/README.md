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
