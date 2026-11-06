package org.openmaptiles.addons;

import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.FeatureMerge;
import com.onthegomap.planetiler.ForwardingProfile;
import com.onthegomap.planetiler.VectorTile;
import com.onthegomap.planetiler.config.PlanetilerConfig;
import com.onthegomap.planetiler.geo.GeometryException;
import com.onthegomap.planetiler.reader.SourceFeature;
import com.onthegomap.planetiler.util.Parse;
import com.onthegomap.planetiler.util.ZoomFunction;
import java.util.List;
import org.openmaptiles.Layer;
import org.openmaptiles.OpenMapTilesProfile;

/**
 * Contour lines in one unit, from the ways {@code topo_pipeline} appends to the OSM input ({@code contour=elevation}
 * with {@code contour:unit=m|ft}). Without such ways this layer is empty.
 * <p>
 * Features have {@code ele}, the elevation as a whole number in the layer's unit, and {@code idx=1} on the index
 * contours for that zoom, which are the ones to draw heavier and label. Each zoom only has lines at its own interval.
 */
public class Contour implements Layer, OpenMapTilesProfile.OsmAllProcessor, ForwardingProfile.LayerPostProcessor {

  public static final String METRIC_LAYER = "contour_m";
  public static final String IMPERIAL_LAYER = "contour_ft";
  private static final int BUFFER_PIXELS = 4;
  /** Shorter fragments than this many pixels are not worth a line, except at the zoom with every line. */
  private static final double MIN_PIXELS_BELOW_MAX = 8;

  /**
   * Intervals from a zoom upward, most detailed first.
   *
   * @param minZoom  the lowest zoom using these intervals
   * @param interval lines kept at this zoom are multiples of this
   * @param index    lines drawn as index contours at this zoom are multiples of this
   */
  record Zoomed(int minZoom, int interval, int index) {}

  private static final List<Zoomed> METRIC = List.of(
    new Zoomed(14, 10, 50),
    new Zoomed(12, 20, 100),
    new Zoomed(11, 50, 250),
    new Zoomed(10, 100, 500),
    new Zoomed(9, 200, 1000),
    new Zoomed(8, 500, 2500)
  );
  private static final List<Zoomed> IMPERIAL = List.of(
    new Zoomed(14, 40, 200),
    new Zoomed(12, 80, 400),
    new Zoomed(11, 200, 1000),
    new Zoomed(10, 400, 2000),
    new Zoomed(9, 1000, 5000),
    new Zoomed(8, 2000, 10000)
  );

  private final String layer;
  private final String unit;
  private final String elevationTag;
  private final List<Zoomed> zooms;
  private final PlanetilerConfig config;

  private Contour(String layer, String unit, String elevationTag, List<Zoomed> zooms, PlanetilerConfig config) {
    this.layer = layer;
    this.unit = unit;
    this.elevationTag = elevationTag;
    this.zooms = zooms;
    this.config = config;
  }

  public static Contour metric(PlanetilerConfig config) {
    return new Contour(METRIC_LAYER, "m", "ele", METRIC, config);
  }

  public static Contour imperial(PlanetilerConfig config) {
    return new Contour(IMPERIAL_LAYER, "ft", "ele:ft", IMPERIAL, config);
  }

  @Override
  public String name() {
    return layer;
  }

  /** The lowest zoom showing a line at this elevation, or {@code null} if none does. */
  private Integer minZoom(long elevation) {
    Integer found = null;
    for (var zoom : zooms) {
      if (elevation % zoom.interval == 0) {
        found = zoom.minZoom;
      }
    }
    return found;
  }

  private Integer indexAt(int zoom, long elevation) {
    for (var entry : zooms) {
      if (zoom >= entry.minZoom) {
        return elevation % entry.index == 0 ? 1 : null;
      }
    }
    return null;
  }

  @Override
  public void processAllOsm(SourceFeature feature, FeatureCollector features) {
    if (!feature.hasTag("contour", "elevation") || !feature.hasTag("contour:unit", unit) || !feature.canBeLine()) {
      return;
    }
    Long elevation = Parse.parseLongOrNull(feature.getTag(elevationTag));
    if (elevation == null) {
      return;
    }
    Integer minZoom = minZoom(elevation);
    if (minZoom == null) {
      return;
    }
    features.line(layer)
      .setBufferPixels(BUFFER_PIXELS)
      .setAttr("ele", elevation)
      .setAttr("idx", (ZoomFunction<Integer>) zoom -> indexAt(zoom, elevation))
      .setMinPixelSize(0) // merge during post-processing, then limit by size
      .setMinZoom(minZoom);
  }

  @Override
  public List<VectorTile.Feature> postProcess(int zoom, List<VectorTile.Feature> items) throws GeometryException {
    double minLength = zoom >= config.maxzoom() ? 0 : MIN_PIXELS_BELOW_MAX;
    return FeatureMerge.mergeLineStrings(items, minLength, config.tolerance(zoom), BUFFER_PIXELS);
  }
}
