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
export 'src/rust/api/offline.dart'
    show
        ArchiveChunk,
        ArchiveError,
        ArchivePlanner,
        ArchiveRange,
        ArchiveTile,
        FoundTile,
        HashError,
        RangeTile,
        RangeTransferTile,
        TileCheckError,
        archiveFirstFetch,
        archiveGapBytes,
        archiveMaxRangeBytes,
        checkTransferTile,
        sha256File,
        unpackTransferRange;
export 'src/rust/api/router.dart'
    show OfflineRouter, OpenError, OpenErrorKind, RouterResponse;
export 'src/rust/api/storage.dart' show FreeSpaceError, freeBytes;
export 'src/rust/api/transit.dart'
    show
        OfflineTransit,
        TransitCancel,
        TransitFeed,
        TransitFeedKind,
        TransitOpenError,
        TransitResponse;

/// Loads the native router. Call once (it is safe to call again) before opening one.
Future<void> initRouting() => _init ??= RustLib.init();
Future<void>? _init;
