package org.openmaptiles.addons;

import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.FeatureMerge;
import com.onthegomap.planetiler.ForwardingProfile;
import com.onthegomap.planetiler.VectorTile;
import com.onthegomap.planetiler.config.PlanetilerConfig;
import com.onthegomap.planetiler.geo.GeometryException;
import com.onthegomap.planetiler.reader.SourceFeature;
import java.util.List;
import org.openmaptiles.Layer;
import org.openmaptiles.OpenMapTilesProfile;

/**
 * Lines on the ground that aren't for traveling along, but tell you where you are or stop you going further, by
 * {@code class}: {@code power}, {@code pipeline} and {@code cutline} in {@code landmark_line} from z13, {@code fence}
 * and {@code wall} in {@code barrier} from z14.
 */
public class LandmarkLine implements Layer, OpenMapTilesProfile.OsmAllProcessor, ForwardingProfile.LayerPostProcessor {

  private static final int BUFFER_PIXELS = 4;

  private final String layer;
  private final PlanetilerConfig config;

  private LandmarkLine(String layer, PlanetilerConfig config) {
    this.layer = layer;
    this.config = config;
  }

  public static LandmarkLine landmarks(PlanetilerConfig config) {
    return new LandmarkLine("landmark_line", config);
  }

  public static LandmarkLine barriers(PlanetilerConfig config) {
    return new LandmarkLine("barrier", config);
  }

  @Override
  public String name() {
    return layer;
  }

  private String classOf(SourceFeature source) {
    if ("barrier".equals(layer)) {
      if (source.hasTag("barrier", "fence")) {
        return "fence";
      }
      return source.hasTag("barrier", "wall", "retaining_wall") ? "wall" : null;
    }
    if (source.hasTag("power", "line")) {
      return "power";
    }
    if (source.hasTag("man_made", "pipeline")) {
      return "pipeline";
    }
    return source.hasTag("man_made", "cutline") ? "cutline" : null;
  }

  @Override
  public void processAllOsm(SourceFeature source, FeatureCollector features) {
    if (!source.canBeLine()) {
      return;
    }
    String clazz = classOf(source);
    if (clazz != null) {
      features.line(layer)
        .setBufferPixels(BUFFER_PIXELS)
        .setAttr("class", clazz)
        .setMinPixelSize(0)
        .setMinZoom("barrier".equals(layer) ? 14 : 13);
    }
  }

  @Override
  public List<VectorTile.Feature> postProcess(int zoom, List<VectorTile.Feature> items) throws GeometryException {
    return FeatureMerge.mergeLineStrings(items, zoom >= config.maxzoom() ? 0 : 4, config.tolerance(zoom),
      BUFFER_PIXELS);
  }
}
