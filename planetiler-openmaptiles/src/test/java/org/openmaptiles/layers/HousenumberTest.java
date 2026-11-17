package org.openmaptiles.layers;

import static com.onthegomap.planetiler.TestUtils.newLineString;
import static com.onthegomap.planetiler.TestUtils.newPoint;
import static org.junit.jupiter.api.Assertions.assertEquals;

import com.onthegomap.planetiler.config.Arguments;
import com.onthegomap.planetiler.config.PlanetilerConfig;
import com.onthegomap.planetiler.geo.GeometryException;
import com.onthegomap.planetiler.reader.SimpleFeature;
import com.onthegomap.planetiler.reader.SourceFeature;
import com.onthegomap.planetiler.reader.osm.OsmElement;
import com.onthegomap.planetiler.reader.osm.OsmReader;
import com.onthegomap.planetiler.reader.osm.OsmRelationInfo;
import com.onthegomap.planetiler.stats.Stats;
import java.nio.file.Path;
import java.sql.SQLException;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;
import org.openmaptiles.OpenMapTilesProfile;
import org.openmaptiles.util.WofAdminsTest;

class HousenumberTest extends AbstractLayerTest {

  @Test
  void testHousenumber() {
    assertFeatures(14, List.of(Map.of(
      "_layer", "housenumber",
      "_type", "point",
      "_minzoom", 14,
      "_maxzoom", 14,
      "_buffer", 8d
    )), process(pointFeature(Map.of(
      "addr:housenumber", "10"
    ))));
    assertFeatures(14, List.of(Map.of(
      "_layer", "housenumber",
      "_type", "point",
      "_minzoom", 14,
      "_maxzoom", 14,
      "_buffer", 8d
    )), process(polygonFeature(Map.of(
      "addr:housenumber", "10"
    ))));
  }

  @ParameterizedTest
  @CsvSource({
    "1, 1",
    "1;1a;2;2/b;20;3, 1–3",
    "1;1a;2;2/b;20;3;, 1–3",
    "1;2;20;3, 1–20",
    "1;2;20;3;, 1–20",
    ";, ;",
    ";;, ;;",
    "2712;935803935803, 2712–935803935803",
  })
  void testDisplayHousenumber(String outlier, String expected) {
    assertEquals(expected, Housenumber.displayHousenumber(outlier));
  }

  @Test
  void testTempAttrs() {
    assertFeatures(14, List.of(Map.of(
      "_has_name", Boolean.TRUE,
      "_partition", "streetX765/6"
    )), process(polygonFeature(Map.of(
      "addr:housenumber", "765/6",
      "addr:block_number", "X",
      "addr:street", "street",
      "name", "name"
    ))));
  }

  @Test
  void testNonduplicateHousenumber() throws GeometryException {
    var layerName = Housenumber.LAYER_NAME;
    var hn1 = pointFeature(
      layerName,
      Map.of(
        "housenumber", "764/2",
        "_partition", "764/2"
      ),
      1
    );
    var hn2 = pointFeature(
      layerName,
      Map.of(
        "housenumber", "765/6",
        "_partition", "765/6"
      ),
      1
    );

    Assertions.assertEquals(
      2,
      profile.postProcessLayerFeatures(layerName, 14, List.of(hn1, hn2)).size()
    );
  }

  @Test
  void testNonduplicateStreet() throws GeometryException {
    var layerName = Housenumber.LAYER_NAME;
    var housenumber = "765/6";
    var hn1 = pointFeature(
      layerName,
      Map.of(
        "housenumber", housenumber,
        "addr:street", "street 1",
        "_partition", "street 1" + housenumber
      ),
      1
    );
    var hn2 = pointFeature(
      layerName,
      Map.of(
        "housenumber", housenumber,
        "addr:street", "street 2",
        "_partition", "street 2" + housenumber
      ),
      1
    );

    var result = profile.postProcessLayerFeatures(layerName, 14, List.of(hn1, hn2));

    // the same number on different streets are different addresses
    Assertions.assertEquals(2, result.size());
  }

  @Test
  void testMergesTrueDuplicates() throws GeometryException {
    var layerName = Housenumber.LAYER_NAME;
    var housenumber = "765/6";
    Map<String, Object> attrs = Map.of(
      "housenumber", housenumber,
      "addr:housenumber", housenumber,
      "addr:street", "street"
    );
    var hn1 = pointFeature(layerName, attrs, 1);
    var hn2 = pointFeature(layerName, attrs, 1);
    // in different partitions, as by block number, but otherwise the same
    hn1.tags().put("_partition", "streetX" + housenumber);
    hn2.tags().put("_partition", "street" + housenumber);

    var result = profile.postProcessLayerFeatures(layerName, 14, List.of(hn1, hn2));

    Assertions.assertEquals(
      1, // same attributes => two points merged into one multipoint
      result.size()
    );
    Assertions.assertEquals(
      5, // two point in multipoint => 5 commands
      result.getFirst().geometry().commands().length);
  }

  private static SourceFeature node(long id, Map<String, Object> tags) {
    return SimpleFeature.create(newPoint(15, 15), new HashMap<>(tags), OpenMapTilesProfile.OSM_SOURCE, null, id);
  }

  private static SourceFeature way(long id, Map<String, Object> tags, String role, List<OsmRelationInfo> relations) {
    return SimpleFeature.createFakeOsmFeature(newLineString(15, 15, 15.001, 15, 15.001, 15.001, 15, 15.001, 15, 15),
      new HashMap<>(tags), OpenMapTilesProfile.OSM_SOURCE, null, id,
      relations.stream().map(r -> new OsmReader.RelationMember<>(role, r)).toList());
  }

  private List<OsmRelationInfo> associatedStreet(String type, String name, OsmElement.Relation.Member... members) {
    var relations = profile.preprocessOsmRelation(new OsmElement.Relation(9,
      new HashMap<>(Map.of("type", type, "name", name)), List.of(members)));
    return relations == null ? List.of() : relations;
  }

  private static OsmElement.Relation.Member member(OsmElement.Type type, long id, String role) {
    return new OsmElement.Relation.Member(type, id, role);
  }

  @Test
  void testKeepsAddressTags() {
    assertFeatures(14, List.of(Map.of(
      "housenumber", "1–3",
      "addr:housenumber", "1;3",
      "addr:street", "Main Street",
      "addr:unit", "B",
      "addr:postcode", "98101",
      "addr:city", "Seattle",
      "_partition", "Main Street1–3\u001fB"
    )), process(pointFeature(Map.of(
      "addr:housenumber", "1;3",
      "addr:street", "Main Street",
      "addr:unit", "B",
      "addr:postcode", "98101",
      "addr:city", "Seattle"
    ))));
  }

  /** The z14 housenumber features of elements with these tags, after the layer's post-processing. */
  private List<com.onthegomap.planetiler.VectorTile.Feature> postProcessed(List<Map<String, Object>> elements)
    throws GeometryException {
    List<com.onthegomap.planetiler.VectorTile.Feature> features = new java.util.ArrayList<>();
    for (var tags : elements) {
      for (var feature : process(pointFeature(tags))) {
        features.add(pointFeature(Housenumber.LAYER_NAME, feature.getAttrsAtZoom(14), 1));
      }
    }
    return profile.postProcessLayerFeatures(Housenumber.LAYER_NAME, 14, features);
  }

  @Test
  void testUnitsAreDifferentAddresses() throws GeometryException {
    var result = postProcessed(List.of(
      Map.of("addr:housenumber", "5", "addr:street", "Main Street", "addr:unit", "1"),
      Map.of("addr:housenumber", "5", "addr:street", "Main Street", "addr:unit", "2"),
      Map.of("addr:housenumber", "5", "addr:street", "Main Street"),
      Map.of("addr:housenumber", "51", "addr:street", "Main Street")
    ));
    assertEquals(4, result.size());
    assertEquals(java.util.Set.of("1", "2", "<none>"), result.stream()
      .filter(f -> "5".equals(f.tags().get("housenumber")))
      .map(f -> String.valueOf(java.util.Objects.requireNonNullElse(f.tags().get("addr:unit"), "<none>")))
      .collect(java.util.stream.Collectors.toSet()));
  }

  @Test
  void testSameUnitStillDeduplicated() throws GeometryException {
    var result = postProcessed(List.of(
      Map.of("addr:housenumber", "5", "addr:street", "Main Street", "addr:unit", "1"),
      Map.of("addr:housenumber", "5", "addr:street", "Main Street", "addr:unit", "1", "name", "Named"),
      Map.of("addr:housenumber", "5", "addr:street", "Main Street", "addr:unit", "1")
    ));
    // the true duplicates merge into one multipoint, drawn; the named one stays for the geocoder, not drawn
    assertEquals(2, result.size());
    var drawn = result.stream().filter(f -> f.tags().get("name") == null).toList();
    assertEquals(1, drawn.size());
    assertEquals("5", drawn.getFirst().tags().get("housenumber"));
    var named = result.stream().filter(f -> "Named".equals(f.tags().get("name"))).toList();
    assertEquals(1, named.size());
    assertEquals(null, named.getFirst().tags().get("housenumber"));
    assertEquals("5", named.getFirst().tags().get("addr:housenumber"));
    assertEquals(null, named.getFirst().tags().get("_search_named"));
  }

  @Test
  void testNamedElementsAtOneNumberAllStay() throws GeometryException {
    // a campus: its buildings share one address, and the geocoder finds each by its name
    var result = postProcessed(List.of(
      Map.of("addr:housenumber", "1900", "addr:street", "Campus Drive"),
      Map.of("addr:housenumber", "1900", "addr:street", "Campus Drive", "name", "Building 43", "building", "yes",
        "cardinal:importance", "0.1"),
      Map.of("addr:housenumber", "1900", "addr:street", "Campus Drive", "name", "Building 44", "building", "yes")
    ));
    assertEquals(3, result.size());
    assertEquals(1, result.stream().filter(f -> f.tags().get("housenumber") != null).count());
    var building = result.stream().filter(f -> "Building 43".equals(f.tags().get("name"))).findFirst().orElseThrow();
    assertEquals(0.1f, building.tags().get("cardinal:importance"));
  }

  @Test
  void testAssociatedStreetNode() {
    associatedStreet("associatedStreet", "Rue de la Paix", member(OsmElement.Type.NODE, 1, "house"),
      member(OsmElement.Type.NODE, 2, "street"), member(OsmElement.Type.NODE, 3, "address"));
    associatedStreet("street", "Rue Royale", member(OsmElement.Type.NODE, 4, "house"));
    // only houses get the street
    assertFeatures(14, List.of(Map.of("addr:street", "Rue de la Paix", "_partition", "Rue de la Paix5")),
      process(node(1, Map.of("addr:housenumber", "5"))));
    assertFeatures(14, List.of(Map.of("addr:street", "<null>", "_partition", "5")),
      process(node(2, Map.of("addr:housenumber", "5"))));
    assertFeatures(14, List.of(Map.of("addr:street", "Rue de la Paix")),
      process(node(3, Map.of("addr:housenumber", "5"))));
    assertFeatures(14, List.of(Map.of("addr:street", "Rue Royale")),
      process(node(4, Map.of("addr:housenumber", "5"))));
    // addr:street wins
    assertFeatures(14, List.of(Map.of("addr:street", "Other Street")),
      process(node(1, Map.of("addr:housenumber", "5", "addr:street", "Other Street"))));
    // not a member
    assertFeatures(14, List.of(Map.of("addr:street", "<null>")),
      process(node(5, Map.of("addr:housenumber", "5"))));
  }

  @Test
  void testAssociatedStreetWay() {
    var relations = associatedStreet("associatedStreet", "Rue de la Paix", member(OsmElement.Type.WAY, 1, "house"));
    assertEquals(1, relations.size());
    assertFeatures(14,
      List.of(Map.of("_layer", "building"),
        Map.of("_layer", "housenumber", "addr:street", "Rue de la Paix", "_partition", "Rue de la Paix5")),
      process(way(1, Map.of("addr:housenumber", "5", "building", "yes"), "house", relations)));
    assertFeatures(14, List.of(Map.of("_layer", "building"), Map.of("_layer", "housenumber", "addr:street", "<null>")),
      process(way(1, Map.of("addr:housenumber", "5", "building", "yes"), "street", relations)));
    // a relation without a name, or of another type, gives no street
    assertEquals(List.of(), associatedStreet("multipolygon", "Rue", member(OsmElement.Type.WAY, 1, "house")));
    assertEquals(List.of(), associatedStreet("associatedStreet", " ", member(OsmElement.Type.WAY, 1, "house")));
  }

  @Test
  void testNamesUnlessInPoiLayer() {
    assertFeatures(14, List.of(Map.of("_layer", "building"), Map.of("_layer", "housenumber",
      "name", "Smith Tower",
      "name:en", "Smith Tower",
      "alt_name", "Smith Building",
      "official_name:en", "L. C. Smith Building",
      "name:etymology:wikidata", "<null>",
      "name_en", "<null>",
      "_has_name", true
    )), process(polygonFeature(Map.of(
      "addr:housenumber", "506",
      "building", "yes",
      "name", "Smith Tower",
      "name:en", "Smith Tower",
      "alt_name", "Smith Building",
      "official_name:en", "L. C. Smith Building",
      "name:etymology:wikidata", "Q123"
    ))));
    // the poi layer has the names of a named POI, so its house number doesn't: the geocoder makes one POI of it
    var features = process(pointFeature(Map.of(
      "addr:housenumber", "506",
      "amenity", "restaurant",
      "name", "Diner",
      "alt_name", "The Diner"
    )));
    assertFeatures(14, List.of(
      Map.of("_layer", "housenumber", "name", "<null>", "alt_name", "<null>", "_has_name", true),
      Map.of("_layer", "poi", "name", "Diner", "alt_name", "The Diner")
    ), features);
  }

  @Test
  void testAdmins(@TempDir Path dir) throws SQLException {
    WofAdminsTest.database(dir).close();
    var config = PlanetilerConfig.from(Arguments.of(Map.of("wof_db", dir.resolve("wof.db").toString())));
    var withAdmins = new OpenMapTilesProfile(translations, config, Stats.inMemory());
    var source = node(1, Map.of("addr:housenumber", "5", "addr:street", "Main Street"));
    var features = featureCollectorFactory.get(source);
    withAdmins.processFeature(source, features);
    // every house number, with a poi_id or not
    assertFeatures(14, List.of(Map.of(
      "cardinal:admin:country", 1L,
      "cardinal:admin:locality", 2L,
      "cardinal:admin:neighbourhood", "<null>"
    )), features);
    withAdmins.release();
  }

  @Test
  void testDuplicateHousenumber() throws GeometryException {
    var layerName = Housenumber.LAYER_NAME;
    var housenumber = "765/6";
    var hn1 = pointFeature(
      layerName,
      Map.of(
        "housenumber", housenumber + " (no name)",
        "_has_name", false,
        "_partition", housenumber
      ),
      1
    );
    var hn2 = pointFeature(
      layerName,
      Map.of(
        "housenumber", housenumber + " (with name)",
        "_has_name", true,
        "_partition", housenumber
      ),
      1
    );

    var result = profile.postProcessLayerFeatures(layerName, 14, List.of(hn1, hn2));

    Assertions.assertEquals(List.of(
      pointFeature(
        layerName,
        Map.of("housenumber", "765/6 (no name)"),
        1
      )
    ), result);
    Assertions.assertEquals(
      3, // only one point in multipoint => 3 commands
      result.getFirst().geometry().commands().length);
  }
}
