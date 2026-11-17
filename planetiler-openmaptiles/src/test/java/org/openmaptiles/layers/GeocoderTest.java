package org.openmaptiles.layers;

import static com.onthegomap.planetiler.TestUtils.newPoint;
import static org.junit.jupiter.api.Assertions.assertEquals;

import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.config.Arguments;
import com.onthegomap.planetiler.config.PlanetilerConfig;
import com.onthegomap.planetiler.reader.SimpleFeature;
import com.onthegomap.planetiler.reader.SourceFeature;
import com.onthegomap.planetiler.stats.Stats;
import java.nio.file.Path;
import java.sql.SQLException;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;
import org.openmaptiles.OpenMapTilesProfile;
import org.openmaptiles.util.WofAdminsTest;

class GeocoderTest extends AbstractLayerTest {

  @Override
  PlanetilerConfig profileConfig() {
    return PlanetilerConfig.defaults();
  }

  private List<FeatureCollector.Feature> geocoder(FeatureCollector features) {
    List<FeatureCollector.Feature> found = new ArrayList<>();
    for (var feature : features) {
      if (feature.getLayer().equals(Geocoder.LAYER_NAME)) {
        found.add(feature);
      }
    }
    return found;
  }

  private List<FeatureCollector.Feature> geocoder(SourceFeature source) {
    return geocoder(process(source));
  }

  private FeatureCollector.Feature one(SourceFeature source) {
    var found = geocoder(source);
    assertEquals(1, found.size(), () -> "geocoder features for " + source.tags());
    return found.getFirst();
  }

  private static Map<String, Object> tags(String key, String value, Object... more) {
    Map<String, Object> tags = new HashMap<>(Map.of(key, value, "name", "Named"));
    for (int i = 0; i < more.length; i += 2) {
      tags.put(more[i].toString(), more[i + 1]);
    }
    return tags;
  }

  @ParameterizedTest
  @CsvSource({
    "building,yes,polygon",
    "building,apartments,polygon",
    "landuse,residential,polygon",
    "landuse,industrial,polygon",
    "landuse,military,polygon",
    "landuse,farmyard,polygon",
    "natural,beach,polygon",
    "natural,wood,polygon",
    "natural,bare_rock,polygon",
    "natural,valley,polygon",
    "natural,cape,point",
    "landuse,retail,polygon",
    "place,square,point",
    "place,farm,point",
    "place,plot,polygon",
    "man_made,bridge,polygon",
    "man_made,pier,line",
    "man_made,tower,point",
    "historic,memorial,point",
    "craft,brewery,point",
    "office,yes,polygon",
    "railway,rail,line",
  })
  void takesNamedElementsWithoutTileNames(String key, String value, String type) {
    var tags = tags(key, value);
    var source = switch (type) {
      case "polygon" -> polygonFeature(tags);
      case "line" -> lineFeature(tags);
      default -> pointFeature(tags);
    };
    var feature = one(source);
    assertEquals(14, feature.getMinZoom());
    assertEquals(14, feature.getMaxZoom());
    assertEquals(Map.of(key, value, "name", "Named"), feature.getAttrsAtZoom(14));
  }

  @ParameterizedTest
  @CsvSource({
    // in other layers with their names
    "amenity,restaurant,point",
    "shop,bakery,polygon",
    "office,lawyer,polygon",
    "natural,peak,point",
    "natural,valley,line",
    "natural,glacier,polygon",
    "natural,spring,point",
    "natural,water,polygon",
    "leisure,park,polygon",
    "leisure,nature_reserve,polygon",
    "aeroway,aerodrome,polygon",
    "place,island,polygon",
    "place,locality,point",
    "waterway,river,line",
    "waterway,stream,line",
    "waterway,dam,line",
    "tourism,viewpoint,point",
    // roads, boundaries and administrative places, as airmail's indexer leaves out
    "highway,residential,line",
    "highway,footway,line",
    "highway,pedestrian,polygon",
    "boundary,administrative,polygon",
    "place,city,point",
    "place,neighbourhood,point",
    "admin_level,8,polygon",
    // left out
    "power,tower,point",
    "power,substation,polygon",
  })
  void leavesOut(String key, String value, String type) {
    var tags = tags(key, value);
    var source = switch (type) {
      case "polygon" -> polygonFeature(tags);
      case "line" -> lineFeature(tags);
      default -> pointFeature(tags);
    };
    assertEquals(List.of(), geocoder(source));
  }

  @Test
  void onlyNamed() {
    assertEquals(List.of(), geocoder(polygonFeature(Map.of("building", "yes"))));
    assertEquals(List.of(), geocoder(polygonFeature(Map.of("building", "yes", "name", ""))));
    assertEquals(List.of(), geocoder(polygonFeature(Map.of("building", "yes", "name:etymology", "Someone"))));
    one(polygonFeature(Map.of("building", "yes", "alt_name", "Old Mill")));
    one(polygonFeature(Map.of("building", "yes", "name:fr", "Le Moulin")));
  }

  @Test
  void addressedBuildingsAreHouseNumbers() {
    // the house number has its names
    assertEquals(List.of(), geocoder(polygonFeature(tags("building", "yes", "addr:housenumber", "5"))));
  }

  @Test
  void definingTagIsTheMostTelling() {
    assertEquals(Map.of("historic", "memorial", "name", "Named"),
      one(polygonFeature(tags("building", "yes", "historic", "memorial"))).getAttrsAtZoom(14));
    // without one, just the names
    assertEquals(Map.of("name", "Named"), one(polygonFeature(tags("area", "yes"))).getAttrsAtZoom(14));
  }

  @Test
  void attributes() {
    var source = SimpleFeature.createFakeOsmFeature(newPoint(0, 0), new HashMap<>(Map.ofEntries(
      Map.entry("man_made", "lighthouse"),
      Map.entry("name", "West Point Lighthouse"),
      Map.entry("name:en", "West Point Lighthouse"),
      Map.entry("alt_name", "Discovery Park Light"),
      Map.entry("official_name:en", "West Point Light Station"),
      Map.entry("name:etymology:wikidata", "Q1"),
      Map.entry("addr:city", "Seattle"),
      Map.entry("addr:postcode", "98199"),
      Map.entry("cardinal:poi_id", "42"),
      Map.entry("cardinal:importance", "0.75"),
      Map.entry("height", "8")
    )), OpenMapTilesProfile.OSM_SOURCE, null, 7, List.of());
    var feature = one(source);
    assertEquals(42L, feature.getId());
    assertEquals(Map.of(
      "man_made", "lighthouse",
      "name", "West Point Lighthouse",
      "name:en", "West Point Lighthouse",
      "alt_name", "Discovery Park Light",
      "official_name:en", "West Point Light Station",
      "addr:city", "Seattle",
      "addr:postcode", "98199",
      "cardinal:importance", 0.75f
    ), feature.getAttrsAtZoom(14));
    // no poi_id, no ID
    assertEquals(0L, one(pointFeature(tags("man_made", "lighthouse"))).getId());
  }

  @Test
  void admins(@TempDir Path dir) throws SQLException {
    WofAdminsTest.database(dir).close();
    var config = PlanetilerConfig.from(Arguments.of(Map.of("wof_db", dir.resolve("wof.db").toString())));
    var withAdmins = new OpenMapTilesProfile(translations, config, Stats.inMemory());
    var source = SimpleFeature.create(newPoint(15, 15), tags("man_made", "tower"), OpenMapTilesProfile.OSM_SOURCE,
      null, 1);
    var features = featureCollectorFactory.get(source);
    withAdmins.processFeature(source, features);
    var attrs = geocoder(features).getFirst().getAttrsAtZoom(14);
    assertEquals(1L, attrs.get("cardinal:admin:country"));
    assertEquals(2L, attrs.get("cardinal:admin:locality"));
    withAdmins.release();
  }
}
