/// Where the app's servers are, from `--dart-define`s (see `dart_defines.example.json`).
library;

/// The tile server's TileJSON URL for the `openmaptiles` source.
const tilesUrl = String.fromEnvironment('CARDINAL_TILES_URL');

/// Where the geocoder's API is: requests go to `<url>/v1/...`.
const geocoderUrl = String.fromEnvironment('CARDINAL_GEOCODER_URL');

/// The offline area that street directions and transit plans use, for development until
/// areas can be downloaded: a directory holding `tiles.tar`, a transit build, and
/// `realtime.json`, or the name of one under the app's `offline/` folder. Directions
/// and transit need it; without it, they say there's no offline area.
const offlineAreaPath = String.fromEnvironment('CARDINAL_OFFLINE_AREA');

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

/// Where the map opens on the first launch, as `lng,lat,zoom`; the whole planet if empty.
/// After that it opens where it was (see `map/viewport.dart`).
const initialView = String.fromEnvironment('CARDINAL_VIEW');
