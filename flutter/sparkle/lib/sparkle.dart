/// MapLibre-style vector maps from local tiles: processed in Rust by sparkle and drawn
/// with Flutter GPU.
library;

export 'src/rust/api/map.dart' show CameraState, ClickedFeature;
export 'src/sparkle_map.dart' show MapStatus, SparkleMap, SparkleMapController, initSparkle;
