/// Offline routing on the device: Valinor's Valhalla-compatible router, in process.
///
/// Give [OfflineRouter.route] the body Ferrostar's request generator makes for the
/// server's `/route`, and hand the response's bytes to Ferrostar's parser: they are
/// what the server would have sent (see `valinor/docs/flutter-api.md`).
library;

import 'src/rust/frb_generated.dart';

export 'src/rust/api/router.dart'
    show OfflineRouter, OpenError, OpenErrorKind, RouterResponse;

/// Loads the native router. Call once (it is safe to call again) before opening one.
Future<void> initRouting() => _init ??= RustLib.init();
Future<void>? _init;
