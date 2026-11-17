package org.openmaptiles.layers;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

import com.onthegomap.planetiler.reader.SourceFeature;
import java.util.HashMap;
import java.util.Map;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;

/**
 * The layers the geocoder reads besides poi, transportation_name and housenumber carry every name at z14, and (but
 * place, which the geocoder takes from Who's On First) the importance poi_service tagged.
 */
class SearchNamesTest extends AbstractLayerTest {

  @ParameterizedTest
  @CsvSource({
    "mountain_peak,natural,peak,point",
    "mountain_peak,natural,ridge,line",
    "mountain_peak,natural,glacier,polygon",
    "water_name,natural,water,polygon",
    "water_name,place,sea,point",
    "place,place,locality,point",
    "place,place,island,point",
    "park,leisure,nature_reserve,polygon",
    "aerodrome_label,aeroway,aerodrome,polygon",
    "waterway,waterway,river,line",
    "waterway,waterway,dam,line",
  })
  void allNamesAtZ14Only(String layer, String key, String value, String type) {
    Map<String, Object> tags = new HashMap<>(Map.of(
      key, value,
      "name", "Named",
      "name:en", "Named",
      "name:fr", "Nommé",
      "alt_name", "Other Name",
      "official_name:en", "Official Name",
      "name:etymology:wikidata", "Q1",
      "cardinal:importance", "0.25"
    ));
    SourceFeature source = switch (type) {
      case "polygon" -> polygonFeatureWithArea(1e-6, tags);
      case "line" -> lineFeature(tags);
      default -> pointFeature(tags);
    };
    int named = 0;
    for (var feature : process(source)) {
      // park outlines keep the renderer's names: their label points carry the rest
      if (!feature.getLayer().equals(layer) || feature.getAttrsAtZoom(14).get("alt_name") == null) {
        continue;
      }
      named++;
      var z14 = feature.getAttrsAtZoom(14);
      assertEquals("Nommé", z14.get("name:fr"));
      assertEquals("Other Name", z14.get("alt_name"));
      assertEquals("Official Name", z14.get("official_name:en"));
      assertNull(z14.get("name:etymology:wikidata"));
      assertEquals(layer.equals("place") ? null : 0.25f, z14.get("cardinal:importance"));
      if (feature.getMinZoom() < 14) {
        assertNull(feature.getAttrsAtZoom(13).get("cardinal:importance"));
        // the renderer's keys only
        var z13 = feature.getAttrsAtZoom(13);
        assertEquals("Named", z13.get("name"));
        assertEquals("Named", z13.get("name:en"));
        assertNull(z13.get("name:fr"));
        assertNull(z13.get("alt_name"));
        assertNull(z13.get("official_name:en"));
      }
    }
    assertEquals(true, named > 0, () -> "no named " + layer + " feature");
  }
}
