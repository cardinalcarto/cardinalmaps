/// Routing for the app: Ferrostar's request generator and parser ([DirectionsAdapter]),
/// Ferrostar's navigation along a route ([NavigationSession]), and Valinor's
/// Valhalla-compatible router, in process, for offline use.
///
/// [DirectionsAdapter.request] makes the body for the server's `/route`. Give that body to
/// [OfflineRouter.route] instead and the response's bytes are what the server would have
/// sent (see `valinor/docs/flutter-api.md`). Either way, [DirectionsAdapter.parse] reads
/// them. [OfflineRouter.traceRoute] and [OfflineRouter.traceAttributes] map match traces
/// (such as recent positions during navigation) with the server's `/trace_route` and
/// `/trace_attributes` bodies, and [OfflineRouter.matchPosition] matches navigation's
/// recent fixes in one call, for the session's `updateLocation`.
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
        DirectionsStep,
        DistanceUnits,
        NavigableRoute;
export 'src/rust/api/matching.dart' show MatchKind, MatchedPosition;
export 'src/rust/api/navigation.dart'
    show
        LocationSimulation,
        ManeuverModifier,
        ManeuverType,
        NavDeviation,
        NavDrivingSide,
        NavInstructionContent,
        NavLane,
        NavLocation,
        NavProgress,
        NavSpokenInstruction,
        NavStep,
        NavUpdate,
        NavVisualInstruction,
        NavigationMode,
        NavigationSession,
        SimulationBias,
        SimulationError,
        TripStatus;
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
