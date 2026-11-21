/// MapLibre-style vector maps from local tiles: processed in Rust by sparkle and drawn
/// with Flutter GPU.
library;

export 'src/attribution.dart'
    show AttributionSpan, SparkleAttribution, parseAttribution;
export 'src/rust/api/map.dart'
    show CameraState, ClickedFeature, EaseCurve, SpriteImageInput;
export 'src/sparkle_map.dart'
    show
        MapGestures,
        MapStatus,
        PuckStyle,
        SparkleMap,
        SparkleMapController,
        initSparkle;
