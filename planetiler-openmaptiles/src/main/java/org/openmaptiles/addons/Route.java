package org.openmaptiles.addons;

import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.FeatureMerge;
import com.onthegomap.planetiler.ForwardingProfile;
import com.onthegomap.planetiler.VectorTile;
import com.onthegomap.planetiler.config.PlanetilerConfig;
import com.onthegomap.planetiler.geo.GeometryException;
import com.onthegomap.planetiler.reader.SourceFeature;
import com.onthegomap.planetiler.reader.osm.OsmElement;
import com.onthegomap.planetiler.reader.osm.OsmReader;
import com.onthegomap.planetiler.reader.osm.OsmRelationInfo;
import com.onthegomap.planetiler.util.MemoryEstimator;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import org.openmaptiles.Layer;
import org.openmaptiles.OpenMapTilesProfile;

/**
 * Hiking, cycling and other non-motorized routes, whichever ways they run along: a way doesn't need a name of its own
 * to be part of the Pacific Crest Trail. A way in several routes gets a feature for each.
 * <p>
 * Features have {@code route} (hiking, foot, bicycle, mtb, horse, canoe, snowmobile, piste or ski) and, where a route
 * has them, {@code network}, {@code ref}, {@code name}, {@code colour} and {@code osmc_symbol}.
 */
public class Route implements Layer, OpenMapTilesProfile.OsmAllProcessor, ForwardingProfile.OsmRelationPreprocessor,
  ForwardingProfile.LayerPostProcessor {

  public static final String LAYER_NAME = "route";
  private static final Set<String> TYPES =
    Set.of("hiking", "foot", "bicycle", "mtb", "horse", "canoe", "snowmobile", "piste", "ski");
  private static final int BUFFER_PIXELS = 4;
  private static final double MIN_PIXELS = 8;

  private final PlanetilerConfig config;

  public Route(PlanetilerConfig config) {
    this.config = config;
  }

  @Override
  public String name() {
    return LAYER_NAME;
  }

  record RouteInfo(@Override long id, String route, String network, String ref, String routeName, String colour,
    String symbol) implements OsmRelationInfo {

    @Override
    public long estimateMemoryUsageBytes() {
      return MemoryEstimator.CLASS_HEADER_BYTES + MemoryEstimator.estimateSizeLong(id) +
        5 * MemoryEstimator.POINTER_BYTES + MemoryEstimator.estimateSize(route) +
        MemoryEstimator.estimateSize(network) + MemoryEstimator.estimateSize(ref) +
        MemoryEstimator.estimateSize(routeName) + MemoryEstimator.estimateSize(colour) +
        MemoryEstimator.estimateSize(symbol);
    }
  }

  /** International and national routes from z8, regional from z10, the rest from z12. */
  static int minZoom(String network) {
    if (network == null || network.isEmpty()) {
      return 12;
    }
    // iwn, nwn, rwn, lwn and their ncn, nhn, npn... equivalents
    return switch (network.charAt(0)) {
      case 'i', 'n' -> 8;
      case 'r' -> 10;
      default -> 12;
    };
  }

  @Override
  public List<OsmRelationInfo> preprocessOsmRelation(OsmElement.Relation relation) {
    String route = relation.getString("route");
    if (route == null || !TYPES.contains(route)) {
      return null;
    }
    return List.of(new RouteInfo(relation.id(), route, relation.getString("network"), relation.getString("ref"),
      relation.getString("name"), relation.getString("colour"), relation.getString("osmc:symbol")));
  }

  @Override
  public void processAllOsm(SourceFeature source, FeatureCollector features) {
    if (!source.canBeLine()) {
      return;
    }
    List<OsmReader.RelationMember<RouteInfo>> relations = source.relationInfo(RouteInfo.class);
    if (relations.isEmpty()) {
      return;
    }
    Set<Long> seen = new HashSet<>(relations.size());
    for (var member : relations) {
      RouteInfo info = member.relation();
      if (seen.add(info.id())) {
        features.line(LAYER_NAME)
          .setBufferPixels(BUFFER_PIXELS)
          .setAttr("route", info.route())
          .setAttr("network", info.network())
          .setAttr("ref", info.ref())
          .setAttr("name", info.routeName())
          .setAttr("colour", info.colour())
          .setAttr("osmc_symbol", info.symbol())
          .setMinPixelSize(0) // merge during post-processing, then limit by size
          .setMinZoom(minZoom(info.network()));
      }
    }
  }

  @Override
  public List<VectorTile.Feature> postProcess(int zoom, List<VectorTile.Feature> items) throws GeometryException {
    double minLength = zoom >= config.maxzoom() ? 0 : MIN_PIXELS;
    return FeatureMerge.mergeLineStrings(items, minLength, config.tolerance(zoom), BUFFER_PIXELS);
  }
}
