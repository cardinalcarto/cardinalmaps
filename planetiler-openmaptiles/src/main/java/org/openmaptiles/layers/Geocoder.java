package org.openmaptiles.layers;

import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.reader.SourceFeature;
import com.onthegomap.planetiler.stats.Stats;
import java.util.List;
import java.util.Map;
import java.util.Set;
import org.openmaptiles.Layer;
import org.openmaptiles.OpenMapTilesProfile;
import org.openmaptiles.generated.Tables;
import org.openmaptiles.util.AssociatedStreets;
import org.openmaptiles.util.OmtLanguageUtils;
import org.openmaptiles.util.WofAdmins;

/**
 * An unstyled z14 layer of the named elements the geocoder indexes that no other layer has with its name: named
 * buildings, landuse, natural features, places, bridges, memorials and so on.
 * <p>
 * It takes what airmail's indexer would make a POI of (an element with a name, that isn't a road, a boundary or an
 * administrative place, which Who's On First covers), leaving out {@code power=*}. Each feature has the element's
 * names, its {@link #DEFINING_KEYS defining tag} as {@code key=value}, its {@code addr:*} tags, the Who's On First
 * areas it's in, its {@code cardinal:importance}, and its poi_id as its ID.
 * <p>
 * The profile runs it after every other layer, since what it takes depends on what they made of an element.
 */
public class Geocoder implements Layer {

  public static final String LAYER_NAME = "geocoder";
  private static final int ZOOM = 14;
  private static final double BUFFER_SIZE = 4;
  private static final String ADDRESS_PREFIX = "addr:";
  private static final String STREET_TAG = "addr:street";

  /** The tags that say what an element is, most telling first. A feature carries the first one it has. */
  static final List<String> DEFINING_KEYS = List.of("amenity", "shop", "craft", "office", "tourism", "leisure",
    "historic", "man_made", "military", "emergency", "healthcare", "aeroway", "railway", "public_transport",
    "aerialway", "natural", "water", "waterway", "place", "landuse", "building", "sport");

  /** {@code place} values Who's On First covers as administrative areas, which airmail's indexer leaves out. */
  private static final Set<String> ADMIN_PLACES = Set.of("borough", "city", "country", "county", "district", "hamlet",
    "municipality", "neighbourhood", "province", "quarter", "region", "state", "suburb", "town", "village");

  /**
   * Layers that already have an element with its name at z14, so the geocoder finds it there. A house number has the
   * names of an element the poi layer doesn't.
   */
  private static final Set<String> NAMED_ELSEWHERE = Set.of(Poi.LAYER_NAME, Housenumber.LAYER_NAME,
    MountainPeak.LAYER_NAME, WaterName.LAYER_NAME, Park.LAYER_NAME, AerodromeLabel.LAYER_NAME, Place.LAYER_NAME,
    Waterway.LAYER_NAME, TransportationName.LAYER_NAME);

  private final Stats stats;
  private AssociatedStreets streets = new AssociatedStreets();
  private WofAdmins admins;

  public Geocoder(Stats stats) {
    this.stats = stats;
  }

  @Override
  public String name() {
    return LAYER_NAME;
  }

  /** Uses {@code streets}, which the profile shares between layers and registers to see relations, for addresses. */
  public void useAssociatedStreets(AssociatedStreets streets) {
    this.streets = streets;
  }

  /** Gives features the areas they're in from {@code admins}, the instance {@link Poi} uses, if not null. */
  public void useAdmins(WofAdmins admins) {
    this.admins = admins;
  }

  /** Whether airmail's indexer would index an element, if named: it isn't a road, boundary or administrative place. */
  static boolean airmailIndexes(SourceFeature source) {
    Object highway = source.getTag("highway");
    Object place = source.getTag("place");
    return (highway == null || "bus_stop".equals(highway)) &&
      !source.hasTag("boundary") &&
      !source.hasTag("admin_level") &&
      !(place != null && ADMIN_PLACES.contains(place.toString()));
  }

  /** Whether another layer has the element {@code features} holds what it made of it, with its name at z14. */
  private static boolean namedElsewhere(SourceFeature source, FeatureCollector features) {
    // The poi layer adds some stops only once it has seen them all.
    if (Tables.OsmPoiPoint.MAPPING.evaluate(source) || Tables.OsmPoiPolygon.MAPPING.evaluate(source)) {
      return true;
    }
    for (var feature : features) {
      if (NAMED_ELSEWHERE.contains(feature.getLayer()) && feature.getMinZoom() <= ZOOM &&
        feature.getMaxZoom() >= ZOOM &&
        (Housenumber.LAYER_NAME.equals(feature.getLayer()) || feature.getAttrsAtZoom(ZOOM).get("name") != null)) {
        return true;
      }
    }
    return false;
  }

  /** Adds the element to this layer if it belongs here, once {@code features} holds what other layers made of it. */
  public void processAfterOtherLayers(SourceFeature source, FeatureCollector features) {
    if (source.tags().isEmpty() || source.hasTag("power") || !airmailIndexes(source)) {
      return;
    }
    Map<String, Object> names = OmtLanguageUtils.searchNames(source.tags());
    if (names.isEmpty() || namedElsewhere(source, features)) {
      return;
    }
    FeatureCollector.Feature feature;
    if (source.isPoint()) {
      feature = features.point(LAYER_NAME);
    } else if (source.canBePolygon()) {
      feature = features.centroidIfConvex(LAYER_NAME);
    } else {
      feature = features.centroid(LAYER_NAME);
    }
    feature.setBufferPixels(BUFFER_SIZE)
      .setMinZoom(ZOOM)
      .putAttrs(names)
      .setAttr(Poi.IMPORTANCE_TAG, Poi.importance(source));
    for (String key : DEFINING_KEYS) {
      Object value = source.getTag(key);
      if (value != null && !value.toString().isEmpty()) {
        feature.setAttr(key, value);
        break;
      }
    }
    for (var tag : source.tags().entrySet()) {
      if (tag.getKey().startsWith(ADDRESS_PREFIX)) {
        feature.setAttr(tag.getKey(), tag.getValue());
      }
    }
    feature.setAttr(STREET_TAG, streets.streetOf(source));
    feature.setId(Poi.poiId(source));
    Poi.addAdmins(admins, stats, feature, source);
  }

  /** Whether {@code source} is from OpenStreetMap, the only source this layer reads. */
  public static boolean reads(SourceFeature source) {
    return OpenMapTilesProfile.OSM_SOURCE.equals(source.getSource());
  }
}
