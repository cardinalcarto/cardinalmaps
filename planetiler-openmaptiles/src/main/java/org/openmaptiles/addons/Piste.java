package org.openmaptiles.addons;

import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.FeatureMerge;
import com.onthegomap.planetiler.ForwardingProfile;
import com.onthegomap.planetiler.VectorTile;
import com.onthegomap.planetiler.config.PlanetilerConfig;
import com.onthegomap.planetiler.geo.GeometryException;
import com.onthegomap.planetiler.reader.SourceFeature;
import java.util.List;
import java.util.Set;
import org.openmaptiles.Layer;
import org.openmaptiles.OpenMapTilesProfile;

/**
 * Winter routes from {@code piste:type=*}. They're mostly not on paths or tracks, so they have their own layer.
 * Features have {@code piste_type}, {@code piste_difficulty}, {@code piste_grooming}, {@code name} and {@code ref}.
 */
public class Piste implements Layer, OpenMapTilesProfile.OsmAllProcessor, ForwardingProfile.LayerPostProcessor {

  public static final String LAYER_NAME = "piste";
  private static final int BUFFER_PIXELS = 4;
  private static final int MIN_ZOOM = 11;
  private static final Set<String> TYPES =
    Set.of("downhill", "nordic", "skitour", "hike", "sled", "snow_park", "sleigh",
      "ice_skate", "connection");
  private static final Set<String> DIFFICULTIES = Set.of("novice", "easy", "intermediate", "advanced", "expert",
    "freeride", "extreme");

  private final PlanetilerConfig config;

  public Piste(PlanetilerConfig config) {
    this.config = config;
  }

  @Override
  public String name() {
    return LAYER_NAME;
  }

  /** The first of a {@code ;} separated value, if it's a known one. */
  static String firstKnown(String value, Set<String> known) {
    if (value == null) {
      return null;
    }
    int end = value.indexOf(';');
    String first = (end < 0 ? value : value.substring(0, end)).trim();
    return known.contains(first) ? first : null;
  }

  @Override
  public void processAllOsm(SourceFeature source, FeatureCollector features) {
    if (!source.canBeLine() || !source.hasTag("piste:type")) {
      return;
    }
    String type = firstKnown(source.getString("piste:type"), TYPES);
    if (type != null) {
      features.line(LAYER_NAME)
        .setBufferPixels(BUFFER_PIXELS)
        .setAttr("piste_type", type)
        .setAttr("piste_difficulty", firstKnown(source.getString("piste:difficulty"), DIFFICULTIES))
        .setAttrWithMinzoom("piste_grooming", source.getString("piste:grooming"), 13)
        .setAttrWithMinzoom("name", source.getString("name"), 13)
        .setAttrWithMinzoom("ref", source.getString("ref"), 13)
        .setMinPixelSize(0)
        .setMinZoom(MIN_ZOOM);
    }
  }

  @Override
  public List<VectorTile.Feature> postProcess(int zoom, List<VectorTile.Feature> items) throws GeometryException {
    return FeatureMerge.mergeLineStrings(items, zoom >= config.maxzoom() ? 0 : 8, config.tolerance(zoom),
      BUFFER_PIXELS);
  }
}
