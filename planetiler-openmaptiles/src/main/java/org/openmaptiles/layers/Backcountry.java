package org.openmaptiles.layers;

import static org.openmaptiles.util.Utils.nullIfEmpty;

import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.reader.SourceFeature;
import com.onthegomap.planetiler.util.Parse;
import java.util.List;
import java.util.Set;
import org.openmaptiles.util.Utils;

/**
 * What the {@code poi} layer does for people in the backcountry: which features it has beyond OpenMapTiles' mappings,
 * and how early they appear.
 * <p>
 * Water appears at z10, then trailheads and shelters at z11. Backcountry POIs are sparse enough for that to work, and a
 * city showing the same things is fine too, so z10-13 only has a generous limit per label grid square to keep tiles
 * from getting heavy, and {@link #priority} decides what stays.
 */
final class Backcountry {

  static final int MIN_ZOOM_WATER = 10;
  static final int MIN_ZOOM_TRAILHEAD = 11;
  static final int MIN_ZOOM_SHELTER = 11;
  /** The highest zoom with a limit per label grid square. */
  static final int MAX_ZOOM_LIMITED = 13;
  private static final int NO_LOW_ZOOM = 14;

  /** Kinds of {@code amenity=shelter} that wouldn't help anyone caught out in the weather, or that are in town. */
  private static final Set<String> NOT_A_REFUGE = Set.of("public_transport", "picnic_shelter", "gazebo", "sun_shelter",
    "field_shelter", "pavilion", "changing_rooms", "parking", "lean_to_parking");

  /** Details of a backcountry POI, kept under their OSM keys. */
  private static final List<String> DETAILS = List.of("drinking_water", "intermittent", "fee", "backcountry", "tents",
    "reservation", "openfire", "fireplace", "capacity", "operator", "ref", "shelter_type", "height", "hiking", "locked",
    "direction", "tower:type", "climbing", "cairn");

  /** Subclasses from OpenMapTiles' mappings that also get {@link #DETAILS}. */
  private static final Set<String> WITH_DETAILS_SUBCLASSES = Set.of("drinking_water", "shelter", "alpine_hut",
    "camp_site", "caravan_site", "viewpoint", "guidepost", "visitor_centre", "office", "map", "toilets", "chalet",
    "gate", "lift_gate", "stile", "parking", "picnic_site", "bbq", "waste_basket");

  /** Values of {@code hazard=*} about the terrain. Most others are road signs, such as for animals crossing. */
  private static final Set<String> HAZARDS = Set.of("rockfall", "landslide", "avalanche", "flooding", "contamination",
    "erosion", "falling_rocks", "shooting_range", "unexploded_ordnance", "minefield");

  private Backcountry() {}

  /** A POI OpenMapTiles' mappings don't have. */
  record Kind(String clazz, String subclass, int minZoom) {}

  /** The kind of POI a feature is, or {@code null} if it's not one this class adds. */
  static Kind kindOf(SourceFeature feature) {
    if (feature.hasTag("tourism", "wilderness_hut")) {
      return new Kind("lodging", "wilderness_hut", MIN_ZOOM_SHELTER);
    }
    if (feature.hasTag("tourism", "camp_pitch")) {
      boolean backcountry = feature.hasTag("backcountry", "yes") || feature.hasTag("informal", "yes");
      return new Kind("campsite", "camp_pitch", backcountry ? 13 : 14);
    }
    if (feature.hasTag("natural", "spring")) {
      return new Kind("spring", "spring", MIN_ZOOM_WATER);
    }
    if (feature.hasTag("man_made", "spring_box")) {
      return new Kind("spring", "spring_box", MIN_ZOOM_WATER);
    }
    if (feature.hasTag("man_made", "water_tap")) {
      return new Kind("water_tap", "water_tap", drinkable(feature) ? MIN_ZOOM_WATER : 14);
    }
    if (feature.hasTag("amenity", "water_point")) {
      return new Kind("water_point", "water_point", drinkable(feature) ? MIN_ZOOM_WATER : 14);
    }
    // most wells are village infrastructure, so one only counts if it's said to be drinkable
    if (feature.hasTag("man_made", "water_well") && feature.hasTag("drinking_water", "yes")) {
      return new Kind("water_well", "water_well", MIN_ZOOM_WATER);
    }
    if (feature.hasTag("highway", "trailhead")) {
      return new Kind("trailhead", "trailhead", MIN_ZOOM_TRAILHEAD);
    }
    if (feature.hasTag("emergency", "mountain_rescue")) {
      return new Kind("emergency", "mountain_rescue", MIN_ZOOM_SHELTER);
    }
    if (feature.hasTag("emergency", "rescue_box")) {
      return new Kind("emergency", "rescue_box", MIN_ZOOM_SHELTER);
    }
    if (feature.hasTag("emergency", "access_point") || feature.hasTag("highway", "emergency_access_point")) {
      return new Kind("emergency", "access_point", 12);
    }
    if (feature.hasTag("emergency", "phone")) {
      return new Kind("emergency", "phone", 12);
    }
    if (feature.hasTag("emergency", "fire_lookout")) {
      return new Kind("emergency", "fire_lookout", 12);
    }
    if (feature.hasTag("amenity", "ranger_station")) {
      return new Kind("ranger_station", "ranger_station", 12);
    }
    if (feature.hasTag("natural", "hot_spring")) {
      return new Kind("hot_spring", "hot_spring", 12);
    }
    if (feature.hasTag("natural", "geyser")) {
      return new Kind("hot_spring", "geyser", 12);
    }
    if (feature.hasTag("waterway", "waterfall")) {
      return new Kind("waterfall", "waterfall", 12);
    }
    // the only ford on a way is already on that way, in the transportation layer
    if (feature.isPoint() && feature.hasTag("ford", "yes", "stepping_stones")) {
      return new Kind("ford", feature.hasTag("ford", "stepping_stones") ? "stepping_stones" : "ford", 13);
    }
    Kind more = moreKinds(feature);
    if (more != null) {
      return more;
    }
    if (feature.hasTag("natural", "cave_entrance")) {
      // named ones are the known ones. Land managers worry about maps leading people to fragile sites.
      return new Kind("cave", "cave_entrance", nullIfEmpty(feature.getString("name")) != null ? 13 : 14);
    }
    return null;
  }

  /** Things worth knowing about once you're close, or that help place you. */
  private static Kind moreKinds(SourceFeature feature) {
    if (feature.hasTag("amenity", "bear_box")) {
      return new Kind("food_storage", "bear_box", 13);
    }
    if (feature.hasTag("natural", "arch")) {
      return new Kind("landmark", "arch", 13);
    }
    if (feature.hasTag("highway", "rest_area")) {
      return new Kind("rest_area", "rest_area", 13);
    }
    if (feature.isPoint() && feature.hasTag("waterway", "rapids")) {
      return new Kind("rapids", "rapids", 13);
    }
    if (feature.hasTag("man_made", "tower")) {
      if (feature.hasTag("tower:type", "observation")) {
        return new Kind("tower", "observation", 13);
      }
      return feature.hasTag("tower:type", "watchtower") ? new Kind("tower", "watchtower", 14) : null;
    }
    if (feature.hasTag("climbing", "crag")) {
      return new Kind("climbing", "crag", 13);
    }
    if (feature.hasTag("man_made", "mineshaft") || feature.hasTag("historic", "mine_shaft")) {
      return new Kind("mine", "mine_shaft", 14);
    }
    if (feature.hasTag("man_made", "adit") || feature.hasTag("historic", "mine_adit")) {
      return new Kind("mine", "adit", 14);
    }
    if (feature.hasTag("historic", "mine")) {
      return new Kind("mine", "mine", 14);
    }
    if (feature.hasTag("man_made", "cairn") && !feature.hasTag("cairn", "clearance")) {
      // most cairns in OSM are farmers' clearance cairns, not waymarks
      return new Kind("cairn", "cairn", 14);
    }
    if (feature.hasTag("natural", "sinkhole")) {
      return new Kind("landmark", "sinkhole", 14);
    }
    if (feature.hasTag("natural", "rock", "stone") && nullIfEmpty(feature.getString("name")) != null) {
      return new Kind("landmark", feature.getString("natural"), 14);
    }
    if (feature.hasTag("man_made", "survey_point")) {
      return new Kind("landmark", "survey_point", 14);
    }
    if (feature.isPoint() && feature.hasTag("hazard") && HAZARDS.contains(feature.getString("hazard"))) {
      return new Kind("hazard", feature.getString("hazard"), 14);
    }
    if (feature.hasTag("amenity", "shower", "waste_disposal", "hunting_stand")) {
      return new Kind(feature.getString("amenity"), feature.getString("amenity"), 14);
    }
    if (feature.hasTag("leisure", "picnic_table", "firepit", "slipway")) {
      return new Kind(feature.getString("leisure"), feature.getString("leisure"), 14);
    }
    if (feature.hasTag("waterway", "access_point")) {
      return new Kind("water_access", "access_point", 14);
    }
    if (feature.hasTag("emergency", "defibrillator")) {
      return new Kind("emergency", "defibrillator", 14);
    }
    if (feature.isPoint() && feature.hasTag("barrier", "kissing_gate", "swing_gate", "cattle_grid", "hampshire_gate",
      "chain")) {
      return new Kind("gate", feature.getString("barrier"), 14);
    }
    return null;
  }

  /** How early a POI from OpenMapTiles' mappings appears, or {@code null} if this class has no say. */
  static Integer minZoomOfMapped(String rawSubclass, String information, SourceFeature source) {
    return switch (rawSubclass) {
      case "drinking_water" -> drinkable(source) ? MIN_ZOOM_WATER : NO_LOW_ZOOM;
      case "shelter" -> shelterMinZoom(source);
      case "alpine_hut" -> MIN_ZOOM_SHELTER;
      case "toilets" -> source.hasTag("toilets:disposal", "pitlatrine") ? 13 : null;
      case "camp_site" -> 12;
      case "viewpoint" -> 12;
      case "information" -> switch (information == null ? "" : information) {
          case "visitor_centre", "office" -> 12;
          case "guidepost" -> source.hasTag("hiking", "yes") ? 13 : NO_LOW_ZOOM;
          default -> null;
        };
      default -> null;
    };
  }

  private static int shelterMinZoom(SourceFeature source) {
    String type = nullIfEmpty(source.getString("shelter_type"));
    return type != null && NOT_A_REFUGE.contains(type) ? NO_LOW_ZOOM : MIN_ZOOM_SHELTER;
  }

  private static boolean drinkable(SourceFeature source) {
    return !source.hasTag("drinking_water", "no");
  }

  /** Lower comes first, for what stays when a label grid square has too much. Below all of {@code Poi}'s ranks. */
  static int priority(String subclass) {
    return switch (subclass) {
      case "wilderness_hut", "alpine_hut" -> 1;
      case "shelter", "mountain_rescue", "rescue_box" -> 2;
      case "spring", "spring_box" -> 3;
      case "water_point", "water_well" -> 4;
      case "drinking_water" -> 5;
      case "water_tap" -> 6;
      case "trailhead" -> 7;
      case "ranger_station", "fire_lookout" -> 8;
      case "camp_site", "camp_pitch" -> 9;
      default -> 10;
    };
  }

  static boolean hasDetails(String subclass) {
    return WITH_DETAILS_SUBCLASSES.contains(subclass);
  }

  /** Limits a POI per label grid square below the zoom where everything is shown. */
  static void limitWhenThin(FeatureCollector.Feature feature, int minZoom) {
    if (minZoom <= MAX_ZOOM_LIMITED) {
      feature.setPointLabelGridLimit(zoom -> gridLimit(zoom));
    }
  }

  /** How many POIs a label grid square keeps at a zoom, or 0 for all of them. */
  static int gridLimit(int zoom) {
    return zoom <= MAX_ZOOM_LIMITED ? 6 : 0;
  }

  static void addDetails(FeatureCollector.Feature feature, SourceFeature source) {
    for (String key : DETAILS) {
      feature.setAttr(key, nullIfEmpty(source.getString(key)));
    }
    feature.setAttr("seasonal", Utils.seasonal(source.getString("seasonal")));
    Double meters = Parse.meters(source.getString("ele"));
    if (meters != null && Math.abs(meters) < 10_000) {
      feature.setAttr("ele", (int) Math.round(meters));
      feature.setAttr("ele_ft", (int) Math.round(meters * 3.28084));
    }
  }
}
