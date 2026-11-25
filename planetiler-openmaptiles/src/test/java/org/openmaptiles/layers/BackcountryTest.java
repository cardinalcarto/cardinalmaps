package org.openmaptiles.layers;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;

import com.onthegomap.planetiler.FeatureCollector;
import com.onthegomap.planetiler.reader.SourceFeature;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;

class BackcountryTest extends AbstractLayerTest {

  private List<FeatureCollector.Feature> poi(SourceFeature source) {
    List<FeatureCollector.Feature> found = new ArrayList<>();
    for (var feature : process(source)) {
      if (feature.getLayer().equals("poi")) {
        found.add(feature);
      }
    }
    return found;
  }

  private FeatureCollector.Feature onePoi(Map<String, Object> tags) {
    List<FeatureCollector.Feature> found = poi(pointFeature(tags));
    assertEquals(1, found.size(), () -> "POIs for " + tags);
    return found.getFirst();
  }

  private int minZoom(Map<String, Object> tags) {
    return onePoi(tags).getMinZoom();
  }

  @ParameterizedTest
  @CsvSource({
    "natural,spring,spring,spring,10",
    "man_made,spring_box,spring,spring_box,10",
    "man_made,water_tap,water_tap,water_tap,10",
    "amenity,water_point,water_point,water_point,10",
    "tourism,wilderness_hut,lodging,wilderness_hut,11",
    "emergency,mountain_rescue,emergency,mountain_rescue,11",
    "emergency,rescue_box,emergency,rescue_box,11",
    "highway,trailhead,trailhead,trailhead,11",
    "emergency,phone,emergency,phone,12",
    "emergency,access_point,emergency,access_point,12",
    "highway,emergency_access_point,emergency,access_point,12",
    "emergency,fire_lookout,emergency,fire_lookout,12",
    "amenity,ranger_station,ranger_station,ranger_station,12",
    "natural,hot_spring,hot_spring,hot_spring,12",
    "natural,geyser,hot_spring,geyser,12",
    "waterway,waterfall,waterfall,waterfall,12",
    "ford,yes,ford,ford,13",
    "ford,stepping_stones,ford,stepping_stones,13",
    "tourism,camp_pitch,campsite,camp_pitch,14",
  })
  void addsWhatOpenMapTilesLacks(String key, String value, String clazz, String subclass, int zoom) {
    var feature = onePoi(Map.of(key, value));
    assertEquals(zoom, feature.getMinZoom());
    assertEquals(clazz, feature.getAttrsAtZoom(14).get("class"));
    assertEquals(subclass, feature.getAttrsAtZoom(14).get("subclass"));
  }

  @Test
  void waterAndShelterAppearAtLowZooms() {
    assertEquals(10, minZoom(Map.of("amenity", "drinking_water")));
    assertEquals(11, minZoom(Map.of("amenity", "shelter", "name", "Lake Shelter")));
    assertEquals(11, minZoom(Map.of("amenity", "shelter")));
    assertEquals(11, minZoom(Map.of("amenity", "shelter", "shelter_type", "lean_to")));
    assertEquals(11, minZoom(Map.of("amenity", "shelter", "shelter_type", "basic_hut")));
    assertEquals(11, minZoom(Map.of("tourism", "alpine_hut")));
  }

  @Test
  void busShelterAndPicnicShelterStayCloseUp() {
    assertEquals(14, minZoom(Map.of("amenity", "shelter", "shelter_type", "public_transport")));
    assertEquals(14, minZoom(Map.of("amenity", "shelter", "shelter_type", "picnic_shelter")));
    assertEquals(14, minZoom(Map.of("amenity", "shelter", "shelter_type", "gazebo")));
  }

  @Test
  void waterThatIsntDrinkableStaysCloseUp() {
    assertEquals(14, minZoom(Map.of("amenity", "drinking_water", "drinking_water", "no")));
    assertEquals(14, minZoom(Map.of("man_made", "water_tap", "drinking_water", "no")));
    assertEquals(14, minZoom(Map.of("amenity", "water_point", "drinking_water", "no")));
  }

  @Test
  void springsStaySafetyFeaturesEvenIfUntreatedAndSayWhetherTheyreDrinkable() {
    var spring = onePoi(Map.of("natural", "spring", "drinking_water", "no", "seasonal", "summer", "ele", "1234"));
    assertEquals(10, spring.getMinZoom());
    var attrs = spring.getAttrsAtZoom(14);
    assertEquals("no", attrs.get("drinking_water"));
    assertEquals("summer", attrs.get("seasonal"));
    assertEquals(1234, attrs.get("ele"));
    assertEquals(4049, attrs.get("ele_ft"));
  }

  @Test
  void wellsCountOnlyWhenSaidToBeDrinkable() {
    assertEquals(10, minZoom(Map.of("man_made", "water_well", "drinking_water", "yes")));
    assertEquals(0, poi(pointFeature(Map.of("man_made", "water_well"))).size());
  }

  @Test
  void elementsInTheMappingsArentAddedTwice() {
    var both = poi(pointFeature(Map.of("amenity", "drinking_water", "natural", "spring")));
    assertEquals(1, both.size());
    assertEquals("drinking_water", both.getFirst().getAttrsAtZoom(14).get("subclass"));
  }

  @Test
  void campsSayWhatTheyHave() {
    var camp = onePoi(Map.of("tourism", "camp_site", "backcountry", "yes", "fee", "no", "openfire", "no", "tents", "4",
      "capacity", "8", "reservation", "required", "operator", "US Forest Service"));
    assertEquals(12, camp.getMinZoom());
    var attrs = camp.getAttrsAtZoom(14);
    assertEquals("yes", attrs.get("backcountry"));
    assertEquals("no", attrs.get("fee"));
    assertEquals("no", attrs.get("openfire"));
    assertEquals("4", attrs.get("tents"));
    assertEquals("required", attrs.get("reservation"));
    assertEquals("US Forest Service", attrs.get("operator"));
  }

  @Test
  void backcountryPitchesAreEarlierThanCampgroundOnes() {
    assertEquals(13, minZoom(Map.of("tourism", "camp_pitch", "backcountry", "yes")));
    assertEquals(14, minZoom(Map.of("tourism", "camp_pitch", "ref", "12")));
  }

  @Test
  void namedCavesAreEarlierThanUnnamedOnes() {
    assertEquals(13, minZoom(Map.of("natural", "cave_entrance", "name", "Ape Cave")));
    assertEquals(14, minZoom(Map.of("natural", "cave_entrance")));
  }

  @Test
  void urbanThingsStayWhereTheyWere() {
    assertEquals(14, minZoom(Map.of("amenity", "cafe", "name", "Ritual")));
    assertEquals(14, minZoom(Map.of("amenity", "toilets")));
    assertFalse(onePoi(Map.of("amenity", "cafe", "fee", "yes")).getAttrsAtZoom(14).containsKey("fee"));
  }

  @Test
  void mostImportantComesFirstWhenSquaresAreLimited() {
    int hut = onePoi(Map.of("tourism", "wilderness_hut", "name", "x")).getSortKey();
    int spring = onePoi(Map.of("natural", "spring", "name", "x")).getSortKey();
    int fountain = onePoi(Map.of("amenity", "drinking_water", "name", "x")).getSortKey();
    int unnamedSpring = onePoi(Map.of("natural", "spring")).getSortKey();
    assertEquals(true, hut < spring);
    assertEquals(true, spring < fountain);
    assertEquals(true, fountain < unnamedSpring);
  }

  @Test
  void lowZoomsAreLimitedPerSquareButTheHighestIsnt() {
    var spring = onePoi(Map.of("natural", "spring"));
    assertEquals(6, spring.getPointLabelGridLimitAtZoom(10));
    assertEquals(6, spring.getPointLabelGridLimitAtZoom(13));
    assertEquals(0, spring.getPointLabelGridLimitAtZoom(14));
  }

  @Test
  void springAreasAreSpringsNotLakes() {
    var source = polygonFeature(Map.of("natural", "spring", "name", "Big Spring"));
    List<String> layers = new ArrayList<>();
    process(source).forEach(feature -> layers.add(feature.getLayer()));
    assertEquals(List.of("poi"), layers);
    assertNotNull(poi(polygonFeature(Map.of("natural", "spring"))).getFirst());
  }

  @Test
  void viaFerrataIsAPathWithItsScale() {
    var way = process(lineFeature(Map.of("highway", "via_ferrata", "via_ferrata_scale", "3", "name", "Rock Route")));
    List<String> layers = new ArrayList<>();
    way.forEach(feature -> layers.add(feature.getLayer()));
    assertEquals(List.of("transportation", "transportation_name"), layers.stream().sorted().toList());
    for (var feature : way) {
      assertEquals(12, feature.getMinZoom());
      assertEquals("via_ferrata", feature.getAttrsAtZoom(14).get("subclass"));
      if (feature.getLayer().equals("transportation")) {
        assertEquals("3", feature.getAttrsAtZoom(14).get("via_ferrata_scale"));
        assertEquals("official", feature.getAttrsAtZoom(14).get("trail"));
      }
    }
  }

  @Test
  void aboriginalLandsAreTheirOwnClass() {
    assertEquals("aboriginal_lands", parkClass(Map.of("boundary", "aboriginal_lands")));
  }

  @Test
  void parkClassesAreASmallSetAndDetailsAreSeparate() {
    assertEquals("protected_area", parkClass(Map.of("boundary", "protected_area", "protection_title", "Water")));
    assertEquals("protected_area", parkClass(Map.of("boundary", "protected_area", "protect_class", "97")));
    assertEquals("conservation", parkClass(Map.of("boundary", "protected_area", "protect_class", "1")));
    assertEquals("sustainable", parkClass(Map.of("boundary", "protected_area", "protect_class", "6")));
    assertEquals("wilderness_preserve",
      parkClass(Map.of("boundary", "protected_area", "protected_area", "wilderness")));

    var attrs = parkFeature(Map.of("boundary", "protected_area", "protect_class", "6", "protection_title",
      "National Forest", "operator", "US Forest Service", "ownership", "national", "access", "permit"))
        .getAttrsAtZoom(10);
    assertEquals("6", attrs.get("protect_class"));
    assertEquals("National Forest", attrs.get("protection_title"));
    assertEquals("US Forest Service", attrs.get("operator"));
    assertEquals("national", attrs.get("ownership"));
    assertEquals("permit", attrs.get("access"));
  }

  private FeatureCollector.Feature parkFeature(Map<String, Object> tags) {
    for (var feature : process(polygonFeature(tags))) {
      if (feature.getLayer().equals("park")) {
        return feature;
      }
    }
    throw new AssertionError("no park feature");
  }

  private Object parkClass(Map<String, Object> tags) {
    return parkFeature(tags).getAttrsAtZoom(10).get("class");
  }

  @ParameterizedTest
  @CsvSource(value = {
    "bog,bog", "marsh,marsh", "swamp,swamp", "tidalflat,tidalflat", "fen,wetland", "null,wetland"
  }, nullValues = "null")
  void wetlandsSayWhatKind(String kind, String expected) {
    Map<String, Object> tags =
      kind == null ? Map.of("natural", "wetland") : Map.of("natural", "wetland", "wetland", kind);
    for (var feature : process(polygonFeature(tags))) {
      if (feature.getLayer().equals("landcover")) {
        assertEquals("wetland", feature.getAttrsAtZoom(14).get("class"));
        assertEquals(expected, feature.getAttrsAtZoom(14).get("subclass"));
        return;
      }
    }
    throw new AssertionError("no landcover feature");
  }

  @Test
  void seasonalWaterSaysSo() {
    var stream = process(lineFeature(Map.of("waterway", "stream", "seasonal", "summer"))).iterator().next();
    assertEquals("summer", stream.getAttrsAtZoom(14).get("seasonal"));
    var list = process(lineFeature(Map.of("waterway", "stream", "seasonal", "spring;summer"))).iterator().next();
    assertEquals("yes", list.getAttrsAtZoom(14).get("seasonal"));
    var unknown = process(lineFeature(Map.of("waterway", "stream", "seasonal", "maybe"))).iterator().next();
    assertEquals(null, unknown.getAttrsAtZoom(14).get("seasonal"));
    for (var lake : process(polygonFeature(Map.of("natural", "water", "seasonal", "yes")))) {
      if (lake.getLayer().equals("water")) {
        assertEquals("yes", lake.getAttrsAtZoom(14).get("seasonal"));
      }
    }
  }

  @ParameterizedTest
  @CsvSource({
    "amenity,bear_box,bear_box,13",
    "natural,arch,arch,13",
    "highway,rest_area,rest_area,13",
    "waterway,rapids,rapids,13",
    "climbing,crag,crag,13",
    "man_made,mineshaft,mine_shaft,14",
    "historic,mine_shaft,mine_shaft,14",
    "man_made,adit,adit,14",
    "historic,mine,mine,14",
    "man_made,cairn,cairn,14",
    "natural,sinkhole,sinkhole,14",
    "man_made,survey_point,survey_point,14",
    "hazard,rockfall,rockfall,14",
    "amenity,shower,shower,14",
    "amenity,hunting_stand,hunting_stand,14",
    "leisure,picnic_table,picnic_table,14",
    "leisure,firepit,firepit,14",
    "waterway,access_point,access_point,14",
    "emergency,defibrillator,defibrillator,14",
    "barrier,kissing_gate,kissing_gate,14",
    "barrier,cattle_grid,cattle_grid,14",
  })
  void addsSmallerFeatures(String key, String value, String subclass, int zoom) {
    var feature = onePoi(Map.of(key, value));
    assertEquals(zoom, feature.getMinZoom());
    assertEquals(subclass, feature.getAttrsAtZoom(14).get("subclass"));
  }

  @Test
  void clearanceCairnsAndRoadSignHazardsArentWanted() {
    assertEquals(0, poi(pointFeature(Map.of("man_made", "cairn", "cairn", "clearance"))).size());
    assertEquals(0, poi(pointFeature(Map.of("hazard", "animal_crossing"))).size());
    assertEquals(0, poi(pointFeature(Map.of("natural", "rock"))).size());
    assertEquals(14, minZoom(Map.of("natural", "rock", "name", "Big Rock")));
  }

  @Test
  void observationTowersButNotOtherTowers() {
    assertEquals(13, minZoom(Map.of("man_made", "tower", "tower:type", "observation")));
    assertEquals(14, minZoom(Map.of("man_made", "tower", "tower:type", "watchtower")));
    assertEquals(0, poi(pointFeature(Map.of("man_made", "tower", "tower:type", "communication"))).size());
  }

  @Test
  void pitToiletsAreEarlierAndGatesSayIfTheyreLocked() {
    assertEquals(13, minZoom(Map.of("amenity", "toilets", "toilets:disposal", "pitlatrine")));
    assertEquals("yes", onePoi(Map.of("barrier", "gate", "locked", "yes")).getAttrsAtZoom(14).get("locked"));
    assertEquals("yes", onePoi(Map.of("amenity", "parking", "fee", "yes")).getAttrsAtZoom(14).get("fee"));
  }

  private List<FeatureCollector.Feature> inLayer(String layer, SourceFeature source) {
    List<FeatureCollector.Feature> found = new ArrayList<>();
    for (var feature : process(source)) {
      if (feature.getLayer().equals(layer)) {
        found.add(feature);
      }
    }
    return found;
  }

  private List<com.onthegomap.planetiler.reader.osm.OsmReader.RelationMember<com.onthegomap.planetiler.reader.osm.OsmRelationInfo>> members(
    com.onthegomap.planetiler.reader.osm.OsmElement.Relation... relations) {
    List<com.onthegomap.planetiler.reader.osm.OsmReader.RelationMember<com.onthegomap.planetiler.reader.osm.OsmRelationInfo>> found =
      new ArrayList<>();
    for (var relation : relations) {
      for (var info : profile.preprocessOsmRelation(relation)) {
        found.add(new com.onthegomap.planetiler.reader.osm.OsmReader.RelationMember<>("", info));
      }
    }
    return found;
  }

  private com.onthegomap.planetiler.reader.osm.OsmElement.Relation route(long id, String type, String network,
    String name) {
    var relation = new com.onthegomap.planetiler.reader.osm.OsmElement.Relation(id);
    relation.setTag("type", "route");
    relation.setTag("route", type);
    if (network != null) {
      relation.setTag("network", network);
    }
    relation.setTag("name", name);
    relation.setTag("osmc:symbol", "red:white:red_bar");
    return relation;
  }

  private SourceFeature wayInRelations(Map<String, Object> tags,
    com.onthegomap.planetiler.reader.osm.OsmElement.Relation... relations) {
    return com.onthegomap.planetiler.reader.SimpleFeature.createFakeOsmFeature(
      com.onthegomap.planetiler.TestUtils.newLineString(0, 0, 1, 1), tags,
      org.openmaptiles.OpenMapTilesProfile.OSM_SOURCE, null, 7, members(relations));
  }

  @Test
  void routesCoverUnnamedWaysWithAFeaturePerRoute() {
    var found = inLayer("route", wayInRelations(Map.of("highway", "path"),
      route(1, "hiking", "nwn", "Pacific Crest Trail"), route(2, "horse", "rwn", "Horse Loop")));
    assertEquals(2, found.size());
    var hiking =
      found.stream().filter(f -> "hiking".equals(f.getAttrsAtZoom(14).get("route"))).findFirst().orElseThrow();
    assertEquals(8, hiking.getMinZoom());
    assertEquals("Pacific Crest Trail", hiking.getAttrsAtZoom(14).get("name"));
    assertEquals("red:white:red_bar", hiking.getAttrsAtZoom(14).get("osmc_symbol"));
    var horse = found.stream().filter(f -> "horse".equals(f.getAttrsAtZoom(14).get("route"))).findFirst().orElseThrow();
    assertEquals(10, horse.getMinZoom());
    assertEquals(12, inLayer("route", wayInRelations(Map.of("highway", "path"), route(3, "foot", null, "Local")))
      .getFirst().getMinZoom());
  }

  @Test
  void otherRoutesAreNotInTheRouteLayerAndFootRoutesAreOfficial() {
    assertEquals(0,
      inLayer("route", wayInRelations(Map.of("highway", "primary"), route(1, "road", "US:I", "I 5"))).size());
    assertEquals("official", inLayer("transportation",
      wayInRelations(Map.of("highway", "path", "informal", "yes"), route(1, "foot", null, "x"))).getFirst()
        .getAttrsAtZoom(14).get("trail"));
  }

  @Test
  void pistesKeepTheirKindAndDifficulty() {
    var piste = inLayer("piste", lineFeature(Map.of("piste:type", "nordic;skitour", "piste:difficulty", "easy", "name",
      "Loop"))).getFirst();
    assertEquals(11, piste.getMinZoom());
    assertEquals("nordic", piste.getAttrsAtZoom(14).get("piste_type"));
    assertEquals("easy", piste.getAttrsAtZoom(14).get("piste_difficulty"));
    assertEquals(0, inLayer("piste", lineFeature(Map.of("piste:type", "nonsense"))).size());
  }

  @Test
  void landmarkLinesAndBarriers() {
    var power = inLayer("landmark_line", lineFeature(Map.of("power", "line"))).getFirst();
    assertEquals(13, power.getMinZoom());
    assertEquals("power", power.getAttrsAtZoom(14).get("class"));
    assertEquals("cutline", inLayer("landmark_line", lineFeature(Map.of("man_made", "cutline"))).getFirst()
      .getAttrsAtZoom(14).get("class"));
    var fence = inLayer("barrier", lineFeature(Map.of("barrier", "fence"))).getFirst();
    assertEquals(14, fence.getMinZoom());
    assertEquals("wall", inLayer("barrier", lineFeature(Map.of("barrier", "retaining_wall"))).getFirst()
      .getAttrsAtZoom(14).get("class"));
    assertEquals(0, inLayer("barrier", lineFeature(Map.of("barrier", "hedge"))).size());
  }

  @Test
  void ladderIsAPathAndTrailRefsArentRoads() {
    assertEquals("ladder", inLayer("transportation", lineFeature(Map.of("highway", "ladder"))).getFirst()
      .getAttrsAtZoom(14).get("subclass"));
    var name = inLayer("transportation_name", lineFeature(Map.of("highway", "path", "ref", "12"))).getFirst();
    assertEquals(null, name.getAttrsAtZoom(14).get("network"));
    var road = inLayer("transportation_name", lineFeature(Map.of("highway", "primary", "ref", "12"))).getFirst();
    assertEquals("road", road.getAttrsAtZoom(14).get("network"));
  }

  @Test
  void trailsSayHowTheyCross() {
    var attrs = inLayer("transportation", lineFeature(Map.of("highway", "path", "ford", "stepping_stones", "portage",
      "yes"))).getFirst().getAttrsAtZoom(14);
    assertEquals("stepping_stones", attrs.get("ford_type"));
    assertEquals(1, attrs.get("portage"));
    assertEquals("suspension", inLayer("transportation", lineFeature(Map.of("highway", "path", "bridge", "yes",
      "bridge:structure", "suspension"))).getFirst().getAttrsAtZoom(14).get("bridge_structure"));
  }

  @Test
  void terrainFeatures() {
    var pass = inLayer("mountain_peak", pointFeature(Map.of("mountain_pass", "yes", "name", "X", "ele", "1000")))
      .getFirst();
    assertEquals("pass", pass.getAttrsAtZoom(14).get("class"));
    assertEquals(7, pass.getMinZoom());
    assertEquals(1, pass.getAttrsAtZoom(14).get("mountain_pass"));
    var saddle =
      inLayer("mountain_peak", pointFeature(Map.of("natural", "saddle", "mountain_pass", "yes", "name", "X")));
    assertEquals(1, saddle.size());
    assertEquals("saddle", saddle.getFirst().getAttrsAtZoom(14).get("class"));
    assertEquals(1, saddle.getFirst().getAttrsAtZoom(14).get("mountain_pass"));
    var peak = inLayer("mountain_peak", pointFeature(Map.of("natural", "peak", "name", "X", "prominence", "300",
      "summit:cross", "yes"))).getFirst().getAttrsAtZoom(14);
    assertEquals(300, peak.get("prominence"));
    assertEquals(1, peak.get("summit_cross"));

    assertEquals(11,
      inLayer("mountain_peak", pointFeature(Map.of("natural", "hill", "name", "X"))).getFirst().getMinZoom());
    assertEquals(0,
      inLayer("mountain_peak", pointFeature(Map.of("natural", "hill", "name", "X", "place", "locality"))).size());
    assertEquals(11,
      inLayer("mountain_peak", lineFeature(Map.of("natural", "valley", "name", "X"))).getFirst().getMinZoom());
    assertEquals(8,
      inLayer("mountain_peak", pointFeature(Map.of("natural", "mountain_range", "name", "X"))).getFirst().getMinZoom());
    assertEquals(14, inLayer("mountain_peak", lineFeature(Map.of("natural", "crevasse"))).getFirst().getMinZoom());
    assertEquals(10,
      inLayer("mountain_peak", lineFeature(Map.of("natural", "ridge", "name", "X"))).getFirst().getMinZoom());
    assertEquals(12, inLayer("mountain_peak", lineFeature(Map.of("natural", "ridge"))).getFirst().getMinZoom());
    assertEquals(12, inLayer("mountain_peak", lineFeature(Map.of("natural", "cliff"))).getFirst().getMinZoom());
    assertEquals("glacier",
      inLayer("mountain_peak", polygonFeature(Map.of("natural", "glacier", "name", "Emmons Glacier")))
        .getFirst().getAttrsAtZoom(14).get("class"));
  }

  @Test
  void landStatusAndGround() {
    assertEquals("forest", parkClass(Map.of("boundary", "forest")));
    assertEquals("conservation", parkClass(Map.of("landuse", "conservation")));
    assertEquals(1, inLayer("park", polygonFeature(Map.of("boundary", "forest", "leisure", "nature_reserve"))).size());
    assertEquals("danger_area", inLayer("landuse", polygonFeature(Map.of("military", "danger_area"))).getFirst()
      .getAttrsAtZoom(14).get("class"));
    assertEquals("winter_sports", inLayer("landuse", polygonFeature(Map.of("landuse", "winter_sports"))).getFirst()
      .getAttrsAtZoom(14).get("class"));
    assertEquals("Base", inLayer("landuse", polygonFeature(Map.of("landuse", "military", "name", "Base"))).getFirst()
      .getAttrsAtZoom(14).get("name"));
    assertEquals("shingle", inLayer("landcover", polygonFeature(Map.of("natural", "shingle"))).getFirst()
      .getAttrsAtZoom(14).get("subclass"));
    assertEquals("mud", inLayer("landcover", polygonFeature(Map.of("natural", "mud"))).getFirst()
      .getAttrsAtZoom(14).get("subclass"));
    assertEquals("trees", inLayer("landcover", polygonFeature(Map.of("landcover", "trees"))).getFirst()
      .getAttrsAtZoom(14).get("subclass"));
  }

  @Test
  void damsRapidsAndLocalities() {
    assertEquals("dam", inLayer("waterway", lineFeature(Map.of("waterway", "dam"))).getFirst()
      .getAttrsAtZoom(14).get("class"));
    assertEquals("rapids", inLayer("waterway", lineFeature(Map.of("waterway", "rapids"))).getFirst()
      .getAttrsAtZoom(14).get("class"));
    var locality = inLayer("place", pointFeature(Map.of("place", "locality", "name", "Old Camp"))).getFirst();
    assertEquals("locality", locality.getAttrsAtZoom(14).get("class"));
    assertEquals(13, locality.getMinZoom());
    assertEquals(0, inLayer("place", pointFeature(Map.of("place", "locality"))).size());
  }
}
