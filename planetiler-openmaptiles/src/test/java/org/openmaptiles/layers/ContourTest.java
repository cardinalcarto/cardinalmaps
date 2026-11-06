package org.openmaptiles.layers;

import static com.onthegomap.planetiler.TestUtils.newLineString;
import static org.junit.jupiter.api.Assertions.assertEquals;

import com.onthegomap.planetiler.geo.GeometryException;
import com.onthegomap.planetiler.reader.SimpleFeature;
import java.util.Map;
import org.junit.jupiter.api.Test;
import org.openmaptiles.OpenMapTilesProfile;

class ContourTest extends AbstractLayerTest {

  private com.onthegomap.planetiler.FeatureCollector contour(Map<String, Object> tags) throws GeometryException {
    return process(SimpleFeature.create(newLineString(0, 0, 1, 1), tags, OpenMapTilesProfile.OSM_SOURCE, null, 0));
  }

  @Test
  void metricLinesAppearAtTheirIntervalZoom() throws GeometryException {
    var tags = Map.<String, Object>of("contour", "elevation", "contour:unit", "m", "ele", "1000");
    assertFeatures(14, java.util.List.of(Map.of(
      "_layer", "contour_m",
      "_minzoom", 8,
      "ele", 1000L,
      "idx", 1
    )), contour(tags));
    assertEquals(14, minZoomOf(Map.of("contour", "elevation", "contour:unit", "m", "ele", "130")));
    assertEquals(12, minZoomOf(Map.of("contour", "elevation", "contour:unit", "m", "ele", "120")));
    assertEquals(11, minZoomOf(Map.of("contour", "elevation", "contour:unit", "m", "ele", "150")));
    assertEquals(10, minZoomOf(Map.of("contour", "elevation", "contour:unit", "m", "ele", "300")));
    assertEquals(9, minZoomOf(Map.of("contour", "elevation", "contour:unit", "m", "ele", "400")));
  }

  @Test
  void imperialLinesUseFeetAndTheirOwnLayer() throws GeometryException {
    var tags = Map.<String, Object>of("contour", "elevation", "contour:unit", "ft", "ele", "1219.20", "ele:ft", "4000");
    assertFeatures(14, java.util.List.of(Map.of(
      "_layer", "contour_ft",
      "_minzoom", 8,
      "ele", 4000L,
      "idx", 1
    )), contour(tags));
    assertEquals(14, minZoomOf(Map.of("contour", "elevation", "contour:unit", "ft", "ele", "1", "ele:ft", "120")));
    assertEquals(12, minZoomOf(Map.of("contour", "elevation", "contour:unit", "ft", "ele", "1", "ele:ft", "160")));
    assertEquals(11, minZoomOf(Map.of("contour", "elevation", "contour:unit", "ft", "ele", "1", "ele:ft", "200")));
  }

  @Test
  void indexContoursDependOnZoom() throws GeometryException {
    var at50 = contour(Map.of("contour", "elevation", "contour:unit", "m", "ele", "50")).iterator().next();
    assertEquals(1, at50.getAttrsAtZoom(14).get("idx"));
    assertEquals(null, at50.getAttrsAtZoom(11).get("idx"));
    var at250 = contour(Map.of("contour", "elevation", "contour:unit", "m", "ele", "250")).iterator().next();
    assertEquals(1, at250.getAttrsAtZoom(11).get("idx"));
    assertEquals(1, at250.getAttrsAtZoom(14).get("idx"));
  }

  @Test
  void otherFeaturesAreIgnored() throws GeometryException {
    assertFeatures(14, java.util.List.of(), contour(Map.of("contour", "elevation", "ele", "100")));
    assertFeatures(14, java.util.List.of(), contour(Map.of("contour:unit", "m", "ele", "100")));
    assertFeatures(14, java.util.List.of(), contour(Map.of("contour", "elevation", "contour:unit", "m", "ele", "x")));
  }

  private int minZoomOf(Map<String, Object> tags) throws GeometryException {
    return contour(tags).iterator().next().getMinZoom();
  }
}
