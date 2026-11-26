/// Where the app's servers are, from `--dart-define`s (see `dart_defines.example.json`).
library;

/// The tile server for the `openmaptiles` source. With [releasesUrl], it's a PMTiles
/// accelerator serving every map release (`deploy/pmtiles-accelerator/`), and the map
/// reads `<url>/<release>/planet.json` for the release the app pinned. Without, it's a
/// TileJSON URL, used as is (a development tile server).
const tilesUrl = String.fromEnvironment('CARDINAL_TILES_URL');

/// Where the geocoder's API is: requests go to `<url>/v1/...`.
const geocoderUrl = String.fromEnvironment('CARDINAL_GEOCODER_URL');

/// Where the Who's On First service is (`wof_service/`): requests go to `<url>/v1/places`.
/// Offline search fetches the areas of each downloaded region from it, once, when the
/// region is ready. Empty means offline search has no areas (no cities, no "in Seattle").
const wofUrl = String.fromEnvironment('CARDINAL_WOF_URL');

/// Where offline releases are published: the bucket's `releases/` over HTTPS, holding
/// `latest.json` (see `publish/README.md`). Empty means no downloads.
const releasesUrl = String.fromEnvironment('CARDINAL_RELEASES_URL');

/// The public GTFS-RT cache root, including its `gtfs-rt/` prefix.
/// Empty means transit uses the static timetable without fetching real-time feeds.
const realtimeCacheUrl = String.fromEnvironment('CARDINAL_REALTIME_CACHE_URL');

/// The offline folder: the directory holding `routing/<date>/`, `transit/<date>/` and
/// `map/<date>/`, laid out as the bucket's `releases/` are. Empty means the app's
/// `offline/` folder (see `offline/offline_area.dart`).
const offlineRoot = String.fromEnvironment('CARDINAL_OFFLINE_ROOT');

/// For development, a routing release put in the offline folder by hand, its date, as
/// in `routing/<date>/`: with [transitRelease], directions and transit use these rather
/// than the releases downloads pinned.
const routingRelease = String.fromEnvironment('CARDINAL_ROUTING_RELEASE');

/// For development, the transit release put in the offline folder by hand with
/// [routingRelease], its date, as in `transit/<date>/`.
const transitRelease = String.fromEnvironment('CARDINAL_TRANSIT_RELEASE');

/// The bundled map style, a file in `assets/map`.
const mapStyle = String.fromEnvironment(
  'CARDINAL_STYLE',
  defaultValue: 'headway.json',
);

/// The bundled map style for dark mode, a file in `assets/map`, made from [mapStyle] by
/// `tool/dark_style.dart`.
const darkMapStyle = String.fromEnvironment(
  'CARDINAL_DARK_STYLE',
  defaultValue: 'headway-dark.json',
);

/// The bundled Cardinal Topo style, a file in `assets/map`.
const topoStyle = 'cardinal-topo.json';

/// The bundled Cardinal Topo style for dark mode, a file in `assets/map`, made from
/// [topoStyle] by `tool/dark_topo_style.dart`.
const darkTopoStyle = 'cardinal-topo-dark.json';

/// The simplified basemaps for driving and cycling navigation, in `assets/map`.
const drivingStyle = 'cardinal-driving.json';
const darkDrivingStyle = 'cardinal-driving-dark.json';

/// Where the map opens on the first launch, as `lng,lat,zoom`; the whole planet if empty.
/// After that it opens where it was (see `map/viewport.dart`).
const initialView = String.fromEnvironment('CARDINAL_VIEW');

/// For development, whether navigation follows a simulated trip along the route rather
/// than the device's location (see `SimulatedTripLocation`), so it can be driven on a
/// desktop or at a desk. Off unless built with
/// `--dart-define=CARDINAL_SIMULATE_NAVIGATION=true`; it's read in any build mode,
/// since the desktop is run in profile mode to see the map as it draws on a phone.
const simulateNavigation = bool.fromEnvironment('CARDINAL_SIMULATE_NAVIGATION');
