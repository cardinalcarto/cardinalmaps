/// Routing for the app: Ferrostar's request generator and parser ([DirectionsAdapter]),
/// and Valinor's Valhalla-compatible router, in process, for offline use.
///
/// [DirectionsAdapter.request] makes the body for the server's `/route`. Give that body to
/// [OfflineRouter.route] instead and the response's bytes are what the server would have
/// sent (see `valinor/docs/flutter-api.md`). Either way, [DirectionsAdapter.parse] reads
/// them.
library;

import 'src/rust/frb_generated.dart';

export 'src/rust/api/directions.dart'
    show
        DirectionsAdapter,
        DirectionsError,
        DirectionsErrorKind,
        DirectionsPoint,
        DirectionsRequest,
        DirectionsRoute,
        DirectionsStep;
export 'src/rust/api/router.dart'
export 'src/rust/api/transit.dart';
    show OfflineRouter, OpenError, OpenErrorKind, RouterResponse;

/// Loads the native router. Call once (it is safe to call again) before opening one.
Future<void> initRouting() => _init ??= RustLib.init();
Future<void>? _init;
