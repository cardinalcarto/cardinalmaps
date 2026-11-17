/*
Copyright (c) 2024, MapTiler.com & OpenMapTiles contributors.
All rights reserved.

Code license: BSD 3-Clause License

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the copyright holder nor the names of its
  contributors may be used to endorse or promote products derived from
  this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Design license: CC-BY 4.0

See https://github.com/openmaptiles/openmaptiles/blob/master/LICENSE.md for details on usage
*/
package org.openmaptiles.layers;

import static java.util.Map.entry;
import static org.openmaptiles.util.Utils.coalesce;
import static org.openmaptiles.util.Utils.nullIfEmpty;
import static org.openmaptiles.util.Utils.nullIfLong;
import static org.openmaptiles.util.Utils.nullOrEmpty;

import com.carrotsearch.hppc.LongIntMap;
import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.ForwardingProfile;
import com.onthegomap.planetiler.VectorTile;
import com.onthegomap.planetiler.collection.Hppc;
import com.onthegomap.planetiler.config.PlanetilerConfig;
import com.onthegomap.planetiler.expression.MultiExpression;
import com.onthegomap.planetiler.geo.GeoUtils;
import com.onthegomap.planetiler.geo.GeometryException;
import com.onthegomap.planetiler.geo.TileCoord;
import com.onthegomap.planetiler.reader.SimpleFeature;
import com.onthegomap.planetiler.reader.SourceFeature;
import com.onthegomap.planetiler.stats.Stats;
import com.onthegomap.planetiler.util.Parse;
import com.onthegomap.planetiler.util.Translations;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;
import java.util.function.Consumer;
import org.locationtech.jts.geom.Point;
import org.openmaptiles.OpenMapTilesProfile;
import org.openmaptiles.generated.OpenMapTilesSchema;
import org.openmaptiles.generated.Tables;
import org.openmaptiles.util.AssociatedStreets;
import org.openmaptiles.util.OmtLanguageUtils;
import org.openmaptiles.util.WofAdmins;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Defines the logic for generating map elements for things like shops, parks, and schools in the {@code poi} layer from
 * source features.
 * <p>
 * This class is ported to Java from
 * <a href="https://github.com/openmaptiles/openmaptiles/tree/master/layers/poi">OpenMapTiles poi sql files</a>.
 */
public class Poi implements
  OpenMapTilesSchema.Poi,
  OpenMapTilesProfile.OsmAllProcessor,
  Tables.OsmPoiPoint.Handler,
  Tables.OsmPoiPolygon.Handler,
  ForwardingProfile.LayerPostProcessor,
  ForwardingProfile.TilePostProcessor,
  ForwardingProfile.FinishHandler {

  /*
   * process() creates the raw POI feature from OSM elements and postProcess()
   * assigns the feature rank from order in the tile at render-time.
   */

  private static final Logger LOGGER = LoggerFactory.getLogger(Poi.class);
  static final String POI_ID_TAG = "cardinal:poi_id";
  static final String CARDINAL_VERSION = "cardinal:version";
  /** A POI's importance for search, from 0 to 1, which poi_service's {@code patch} command adds. */
  static final String IMPORTANCE_TAG = "cardinal:importance";
  /** The attribute of a POI holding the ID of the Who's On First area of a placetype it's in, after this prefix. */
  static final String ADMIN_PREFIX = "cardinal:admin:";
  /**
   * A layer of one feature for each area POIs in the tile are in, by ID, with their names in {@value #ADMIN_NAMES}. It
   * leaves out {@link WofAdmins#NAMED_IN_APP}.
   */
  static final String ADMINS_LAYER = "admins";
  static final String ADMIN_NAMES = "names";
  private static final Map<String, Integer> CLASS_RANKS = Map.ofEntries(
    entry(FieldValues.CLASS_HOSPITAL, 20),
    entry(FieldValues.CLASS_RAILWAY, 40),
    entry(FieldValues.CLASS_BUS, 50),
    entry(FieldValues.CLASS_ATTRACTION, 70),
    entry(FieldValues.CLASS_HARBOR, 75),
    entry(FieldValues.CLASS_COLLEGE, 80),
    entry(FieldValues.CLASS_SCHOOL, 85),
    entry(FieldValues.CLASS_STADIUM, 90),
    entry("zoo", 95),
    entry(FieldValues.CLASS_TOWN_HALL, 100),
    entry(FieldValues.CLASS_CAMPSITE, 110),
    entry(FieldValues.CLASS_CEMETERY, 115),
    entry(FieldValues.CLASS_PARK, 120),
    entry(FieldValues.CLASS_LIBRARY, 130),
    entry("police", 135),
    entry(FieldValues.CLASS_POST, 140),
    entry(FieldValues.CLASS_GOLF, 150),
    entry(FieldValues.CLASS_SHOP, 400),
    entry(FieldValues.CLASS_GROCERY, 500),
    entry(FieldValues.CLASS_FAST_FOOD, 600),
    entry(FieldValues.CLASS_CLOTHING_STORE, 700),
    entry(FieldValues.CLASS_BAR, 800)
  );
  private static final Set<String> UNIVERSITY_POI_SUBCLASSES = Set.of("university", "college");
  private static final List<String> AGG_STOP_SUBCLASS_ORDER = List.of(
    "subway",
    "tram_stop",
    "bus_station",
    "bus_stop"
  );
  private static final Comparator<Tables.OsmPoiPoint> BY_SUBCLASS = Comparator
    .comparingInt(s -> AGG_STOP_SUBCLASS_ORDER.indexOf(s.subclass()));
  private static final Set<String> BRAND_OPERATOR_REF_SUBCLASSES = Set.of("charging_station", "parcel_locker");
  private final MultiExpression.Index<String> classMapping;
  private final Translations translations;
  private final Stats stats;
  private final Map<String, List<Tables.OsmPoiPoint>> aggStops = new HashMap<>();
  /** The areas POIs are in, or null if there's no Who's On First database to tell. */
  private final WofAdmins admins;
  private AssociatedStreets streets = new AssociatedStreets();

  public Poi(Translations translations, PlanetilerConfig config, Stats stats) {
    this.classMapping = FieldMappings.Class.index();
    this.translations = translations;
    this.stats = stats;
    String wofDb = config.arguments().getString("wof_db",
      "Who's On First SpatiaLite database, to give POIs and house numbers the areas they're in", "");
    this.admins = wofDb.isEmpty() ? null : new WofAdmins(Path.of(wofDb));
  }

  /** The areas POIs are in, which {@link Housenumber} shares, or null if there's no Who's On First database. */
  public WofAdmins admins() {
    return admins;
  }

  /** Uses {@code streets}, which the profile shares between layers and registers to see relations, for addresses. */
  public void useAssociatedStreets(AssociatedStreets streets) {
    this.streets = streets;
  }

  static int poiClassRank(String clazz) {
    return CLASS_RANKS.getOrDefault(clazz, 1_000);
  }

  private String poiClass(String subclass, String mappingKey) {
    // Special case subclass collision between office=university and amenity=university
    if ("amenity".equals(mappingKey) && "university".equals(subclass)) {
      return FieldValues.CLASS_COLLEGE;
    }

    subclass = coalesce(subclass, "");
    return classMapping.getOrElse(Map.of(
      "subclass", subclass,
      "mapping_key", coalesce(mappingKey, "")
    ), subclass);
  }

  private int minzoom(String subclass, String mappingKey) {
    boolean lowZoom = ("station".equals(subclass) && "railway".equals(mappingKey)) ||
      "halt".equals(subclass) || "ferry_terminal".equals(subclass);
    return lowZoom ? 12 : 14;
  }

  @Override
  public void release() {
    aggStops.clear();
  }

  @Override
  public void process(Tables.OsmPoiPoint element, FeatureCollector features) {
    if (element.uicRef() != null && AGG_STOP_SUBCLASS_ORDER.contains(element.subclass())) {
      // multiple threads may update this concurrently
      String aggStopKey = element.uicRef()
        .concat(coalesce(nullIfEmpty(element.name()), ""))
        .concat(coalesce(nullIfEmpty(element.network()), ""))
        .concat(coalesce(nullIfEmpty(element.operator()), ""));
      synchronized (this) {
        aggStops.computeIfAbsent(aggStopKey, key -> new ArrayList<>()).add(element);
      }
    } else {
      setupPoiFeature(element, features.point(LAYER_NAME), null);
    }
  }

  private void processAggStop(Tables.OsmPoiPoint element, FeatureCollector.Factory featureCollectors,
    Consumer<FeatureCollector.Feature> emit, Integer aggStop) {
    try {
      var features =
        featureCollectors.get(SimpleFeature.fromWorldGeometry(element.source().worldGeometry(), element.source().id()));
      setupPoiFeature(element, features.point(LAYER_NAME), aggStop);
      for (var feature : features) {
        emit.accept(feature);
      }
    } catch (GeometryException e) {
      e.log(stats, "agg_stop_geometry_2",
        "Error getting geometry for the stop " + element.source().id() + " (agg_stop)");
    }
  }

  /**
   * We've put aside some stops for {@code agg_stop} processing and we do that processing here.
   * <p>
   * The main point is to group together stops with same {@code uid_ref} and then order them first based on subclass
   * (see {@code AGG_STOP_ORDER}) and then based on distance from centroid (calculated from all the stops). The first
   * one gets {@code agg_stop=1}, the rest will be "normal" (e.g. no {@code agg_stop} attribute).
   * <p>
   * ref: <a href=
   * "https://github.com/openmaptiles/openmaptiles/blob/master/layers/poi/poi_stop_agg.sql#L26,L28">poi_stop_agg.sql</a>
   */
  @Override
  public void finish(String sourceName, FeatureCollector.Factory featureCollectors,
    Consumer<FeatureCollector.Feature> emit) {
    if (OpenMapTilesProfile.OSM_SOURCE.equals(sourceName)) {
      var timer = stats.startStage("agg_stop");
      LOGGER.info("Processing {} agg_stop sets", aggStops.size());

      for (var aggStopSet : aggStops.values()) {
        if (aggStopSet.size() == 1) {
          processAggStop(aggStopSet.getFirst(), featureCollectors, emit, 1);
          continue;
        }
        Tables.OsmPoiPoint nearest = null;
        try {
          // find most important stops based on subclass
          var firstSubclass = aggStopSet.stream().min(BY_SUBCLASS).get().subclass();
          var topAggStops =
            aggStopSet.stream().filter(s -> firstSubclass.equals(s.subclass())).toArray(Tables.OsmPoiPoint[]::new);

          // calculate the centroid and ...
          List<Point> aggStopPoints = new ArrayList<>(aggStopSet.size());
          for (var aggStop : aggStopSet) {
            aggStopPoints.add(aggStop.source().worldGeometry().getCentroid());
          }
          var aggStopCentroid = GeoUtils.combinePoints(aggStopPoints).getCentroid();

          // ... find one stop nearest to the centroid
          double minDistance = Double.MAX_VALUE;
          for (var aggStop : topAggStops) {
            double distance = aggStopCentroid.distance(aggStop.source().worldGeometry());
            if (distance < minDistance || nearest == null ||
              (distance == minDistance && aggStop.source().id() < nearest.source().id())) {
              minDistance = distance;
              nearest = aggStop;
            }
          }
        } catch (GeometryException e) {
          e.log(stats, "agg_stop_geometry_1",
            "Error getting geometry for some of the stops with UIC ref. " + aggStopSet.getFirst().uicRef() +
              " (agg_stop)");
          // we're not able to calculate agg_stop, so simply dump the stops as they are
          nearest = null;
        }

        // now emit the stops
        final Tables.OsmPoiPoint nearestFinal = nearest; // final needed for lambda
        aggStopSet
          .forEach(s -> processAggStop(s, featureCollectors, emit, s == nearestFinal ? 1 : null));
      }

      timer.stop();
    }
  }

  /** Adds what {@link Backcountry} has beyond the mappings of {@link Tables.OsmPoiPoint} and its polygon equivalent. */
  @Override
  public void processAllOsm(SourceFeature source, FeatureCollector features) {
    if (source.tags().isEmpty()) {
      return;
    }
    Backcountry.Kind kind = Backcountry.kindOf(source);
    if (kind == null || Tables.OsmPoiPoint.MAPPING.evaluate(source) || Tables.OsmPoiPolygon.MAPPING.evaluate(source)) {
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
    String name = nullIfEmpty(source.getString("name"));
    OmtLanguageUtils.putNames(feature, source.tags(), translations);
    feature.setBufferPixels(BUFFER_SIZE)
      .setAttr(Fields.CLASS, kind.clazz())
      .setAttr(Fields.SUBCLASS, kind.subclass())
      .setAttr(IMPORTANCE_TAG, importance(source))
      .setPointLabelGridPixelSize(14, 64)
      .setSortKey(Backcountry.priority(kind.subclass()) + (name == null ? 2000 : 0))
      .setMinZoom(kind.minZoom());
    Backcountry.limitWhenThin(feature, kind.minZoom());
    long poiId = poiId(source);
    feature.setId(poiId);
    if (poiId != VectorTile.NO_FEATURE_ID) {
      feature.setAttr(CARDINAL_VERSION, Parse.parseIntOrNull(source.getTag(CARDINAL_VERSION)));
      addAdmins(feature, source);
    }
    addIncludedTags(feature, source);
    Backcountry.addDetails(feature, source);
  }

  @Override
  public void process(Tables.OsmPoiPolygon element, FeatureCollector features) {
    setupPoiFeature(element, features.centroidIfConvex(LAYER_NAME), null);
  }

  private <T extends Tables.WithSubclass & Tables.WithStation & Tables.WithFunicular & Tables.WithSport & Tables.WithInformation & Tables.WithReligion & Tables.WithMappingKey & Tables.WithName & Tables.WithIndoor & Tables.WithLayer & Tables.WithSource & Tables.WithOperator & Tables.WithNetwork & Tables.WithBrand & Tables.WithRef> void setupPoiFeature(
    T element, FeatureCollector.Feature output, Integer aggStop) {
    String rawSubclass = element.subclass();
    if ("station".equals(rawSubclass) && "subway".equals(element.station())) {
      rawSubclass = "subway";
    }
    if ("station".equals(rawSubclass) && "yes".equals(element.funicular())) {
      rawSubclass = "halt";
    }

    String street = streets.streetOf(element.source());

    // ATM names fall back to operator, or else network
    String name = element.name();
    var tags = element.source().tags();
    if ("atm".equals(rawSubclass) && nullOrEmpty(name)) {
      name = coalesce(nullIfEmpty(element.operator()), nullIfEmpty(element.network()));
      if (name != null) {
        tags.put("name", name);
      }
    }

    // Parcel locker without name: use either brand or operator and add ref if present
    if (BRAND_OPERATOR_REF_SUBCLASSES.contains(rawSubclass) && nullOrEmpty(name)) {
      name = coalesce(nullIfEmpty(element.brand()), nullIfEmpty(element.operator()));
      String ref = nullIfEmpty(element.ref());
      if (ref != null) {
        name = name == null ? ref : (name + " " + ref);
      }
      if (name != null) {
        tags.put("name", name);
      }
    }

    String subclass = switch (rawSubclass) {
      case "information" -> nullIfEmpty(element.information());
      case "place_of_worship" -> nullIfEmpty(element.religion());
      case "pitch" -> nullIfEmpty(element.sport());
      default -> rawSubclass;
    };
    String poiClass = poiClass(rawSubclass, element.mappingKey());
    int poiClassRank = poiClassRank(poiClass);
    Integer backcountryMinZoom = Backcountry.minZoomOfMapped(rawSubclass, element.information(), element.source());
    if (backcountryMinZoom != null) {
      poiClassRank = Math.min(poiClassRank, Backcountry.priority(rawSubclass));
    }
    int rankOrder = poiClassRank + ((nullOrEmpty(name)) ? 2000 : 0);

    int minzoom = backcountryMinZoom != null ? backcountryMinZoom : minzoom(element.subclass(), element.mappingKey());
    if (UNIVERSITY_POI_SUBCLASSES.contains(rawSubclass)) {
      // universities that are at least 10% of a tile may appear from Z10
      output.setMinPixelSizeBelowZoom(13, 80); // 80x80px is ~10% of a 256x256px tile
      minzoom = 10;
    }

    final String streetTag = "addr:street";
    FeatureCollector.Feature feature = output.setBufferPixels(BUFFER_SIZE)
      .setAttr(Fields.CLASS, poiClass)
      .setAttr(Fields.SUBCLASS, subclass)
      .setAttr(Fields.LAYER, nullIfLong(element.layer(), 0))
      .setAttr(Fields.LEVEL, Parse.parseLongOrNull(element.source().getTag("level")))
      .setAttr(Fields.INDOOR, element.indoor() ? 1 : null)
      .setAttr(Fields.AGG_STOP, aggStop)
      .setAttr(streetTag, street) // Special-case for associated streets.
      .setAttr(IMPORTANCE_TAG, importance(element.source()))
      .setPointLabelGridPixelSize(14, 64)
      .setSortKey(rankOrder)
      .setMinZoom(minzoom);
    OmtLanguageUtils.putNames(feature, element.source().tags(), translations);
    if (backcountryMinZoom != null) {
      Backcountry.limitWhenThin(feature, minzoom);
    }
    if (Backcountry.hasDetails(subclass == null ? rawSubclass : subclass)) {
      Backcountry.addDetails(feature, element.source());
    }
    long poiId = poiId(element.source());
    feature.setId(poiId);
    if (poiId != VectorTile.NO_FEATURE_ID) {
      // The OSM version, which poi_service's patch command adds alongside the poi_id. It tells clients whether a POI's
      // data is fresher than what they cached, since each poi_id belongs to a single element.
      feature.setAttr(CARDINAL_VERSION, Parse.parseIntOrNull(element.source().getTag(CARDINAL_VERSION)));
      addAdmins(feature, element.source());
    }

    addIncludedTags(feature, element.source());
  }

  private static void addIncludedTags(FeatureCollector.Feature feature, SourceFeature source) {
    final String streetTag = "addr:street";
    final String addressPrefix = "addr:";
    for (String key : source.tags().keySet()) {
      if (key.startsWith(addressPrefix) && !key.equals(streetTag)) {
        feature.setAttr(key, source.getTag(key));
      }
    }

    final List<String> includedTags =
      List.of("amenity", "shop", "emergency", "craft", "healthcare", "office", "tourism", "natural", "cuisine", "phone",
        "website", "wikidata", "brand", "building", "wheelchair", "elevator", "opening_hours", "access");
    final List<String> includedTagPrefixes = List.of("diet:", "brand:", "toilets:");
    for (String key : includedTags) {
      feature.setAttr(key, source.getTag(key));
    }
    for (String key : source.tags().keySet()) {
      for (String keyPrefix : includedTagPrefixes) {
        if (key.startsWith(keyPrefix)) {
          feature.setAttr(key, source.getTag(key));
          break;
        }
      }
    }
  }

  /** The importance {@code poi_service} gave a POI, or null if it gave none. */
  static Float importance(SourceFeature source) {
    Double importance = Parse.parseDoubleOrNull(source.getTag(IMPORTANCE_TAG));
    return importance == null || !Double.isFinite(importance) ? null : importance.floatValue();
  }

  /** The areas a POI is in, for its place card. Only POIs with IDs have cards. */
  private void addAdmins(FeatureCollector.Feature feature, SourceFeature source) {
    addAdmins(admins, stats, feature, source);
  }

  /**
   * Gives {@code feature} the ID of the area of each placetype that the center of {@code source} is in, as
   * {@value #ADMIN_PREFIX}{@code <placetype>}, unless {@code admins} is null.
   */
  static void addAdmins(WofAdmins admins, Stats stats, FeatureCollector.Feature feature, SourceFeature source) {
    if (admins == null) {
      return;
    }
    try {
      var center = source.latLonGeometry().getCentroid();
      admins.at(center.getX(), center.getY())
        .forEach((placetype, id) -> feature.setAttr(ADMIN_PREFIX + placetype, id));
    } catch (GeometryException e) {
      e.log(stats, "poi_admins", "Unable to find the center of " + source);
    }
  }

  @Override
  public Map<String, List<VectorTile.Feature>> postProcessTile(TileCoord tile,
    Map<String, List<VectorTile.Feature>> layers) {
    List<VectorTile.Feature> pois = layers.get(LAYER_NAME);
    if (admins == null || pois == null) {
      return layers;
    }
    Set<Long> ids = new TreeSet<>();
    for (VectorTile.Feature poi : pois) {
      for (String placetype : WofAdmins.PLACETYPES) {
        if (!WofAdmins.NAMED_IN_APP.contains(placetype) &&
          poi.tags().get(ADMIN_PREFIX + placetype)instanceof Number id) {
          ids.add(id.longValue());
        }
      }
    }
    if (ids.isEmpty()) {
      return layers;
    }
    // An unstyled layer, so where its points are doesn't matter.
    var geometry = VectorTile.encodeGeometry(GeoUtils.point(128, 128));
    List<VectorTile.Feature> features = new ArrayList<>(ids.size());
    for (long id : ids) {
      admins.namesJson(id).ifPresent(names -> features
        .add(new VectorTile.Feature(ADMINS_LAYER, id, geometry, Map.of(ADMIN_NAMES, names))));
    }
    Map<String, List<VectorTile.Feature>> result = new HashMap<>(layers);
    result.put(ADMINS_LAYER, features);
    return result;
  }

  /**
   * Cardinal's stable identifier for a POI, from the {@code cardinal:poi_id} tag that poi_service's {@code patch}
   * command adds. POIs without one get no ID rather than an OSM-derived one, so every ID in this layer is a poi_id.
   */
  static long poiId(SourceFeature source) {
    Long poiId = Parse.parseLongOrNull(source.getTag(POI_ID_TAG));
    return poiId != null && poiId > 0 ? poiId : VectorTile.NO_FEATURE_ID;
  }

  @Override
  public List<VectorTile.Feature> postProcess(int zoom, List<VectorTile.Feature> items) {
    // infer the "rank" field from the order of features within each label grid square
    LongIntMap groupCounts = Hppc.newLongIntHashMap();
    for (VectorTile.Feature feature : items) {
      int gridrank = groupCounts.getOrDefault(feature.group(), 1);
      groupCounts.put(feature.group(), gridrank + 1);
      if (!feature.tags().containsKey(Fields.RANK)) {
        feature.tags().put(Fields.RANK, gridrank);
      }
    }
    return items;
  }
}
