/// Where the app's servers are, from `--dart-define`s (see `dart_defines.example.json`).
library;

/// The tile server's TileJSON URL for the `openmaptiles` source.
const tilesUrl = String.fromEnvironment('CARDINAL_TILES_URL');

/// Where the geocoder's API is: requests go to `<url>/v1/...`.
const geocoderUrl = String.fromEnvironment('CARDINAL_GEOCODER_URL');

/// Where the routing server's Valhalla API is: requests go to `<url>/route`.
const routingUrl = String.fromEnvironment('CARDINAL_ROUTING_URL');

/// The bundled map style, a file in `assets/map`.
const mapStyle = String.fromEnvironment(
  'CARDINAL_STYLE',
  defaultValue: 'headway.json',
);

/// The bundled Cardinal Topo style, a file in `assets/map`.
const topoStyle = 'cardinal-topo.json';

/// Where the map opens on the first launch, as `lng,lat,zoom`; the whole planet if empty.
/// After that it opens where it was (see `map/viewport.dart`).
const initialView = String.fromEnvironment('CARDINAL_VIEW');
