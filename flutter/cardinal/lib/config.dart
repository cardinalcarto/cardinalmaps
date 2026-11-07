/// Where the app's servers are, from `--dart-define`s (see `dart_defines.example.json`).
library;

/// The tile server's TileJSON URL for the `openmaptiles` source.
const tilesUrl = String.fromEnvironment('CARDINAL_TILES_URL');

/// Where the geocoder's API is: requests go to `<url>/v1/...`.
const geocoderUrl = String.fromEnvironment('CARDINAL_GEOCODER_URL');

/// The bundled map style, a file in `assets/map`.
const mapStyle = String.fromEnvironment('CARDINAL_STYLE', defaultValue: 'headway.json');

/// Where the map starts, as `lng,lat,zoom`; the tileset's center if empty.
const initialView = String.fromEnvironment('CARDINAL_VIEW');
