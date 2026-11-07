package org.openmaptiles.util;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Path;
import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class WofAdminsTest {

  /** A SpatiaLite polygon blob from rings of x, y pairs. */
  private static byte[] polygon(double[]... rings) {
    int size = 39 + 4 + 4 + 1;
    double minX = Double.MAX_VALUE, minY = Double.MAX_VALUE, maxX = -Double.MAX_VALUE, maxY = -Double.MAX_VALUE;
    for (double[] ring : rings) {
      size += 4 + ring.length * 8;
      for (int i = 0; i < ring.length; i += 2) {
        minX = Math.min(minX, ring[i]);
        maxX = Math.max(maxX, ring[i]);
        minY = Math.min(minY, ring[i + 1]);
        maxY = Math.max(maxY, ring[i + 1]);
      }
    }
    ByteBuffer b = ByteBuffer.allocate(size).order(ByteOrder.LITTLE_ENDIAN);
    b.put((byte) 0).put((byte) 1).putInt(4326);
    b.putDouble(minX).putDouble(minY).putDouble(maxX).putDouble(maxY);
    b.put((byte) 0x7C).putInt(3).putInt(rings.length);
    for (double[] ring : rings) {
      b.putInt(ring.length / 2);
      for (double c : ring) {
        b.putDouble(c);
      }
    }
    b.put((byte) 0xFE);
    return b.array();
  }

  private static double[] square(double west, double south, double east, double north) {
    return new double[]{west, south, east, south, east, north, west, north, west, south};
  }

  private static void add(Connection db, long id, String type, double[] box) throws SQLException {
    add(db, id, type, polygon(box), box);
  }

  private static void add(Connection db, long id, String type, byte[] shard, double[] bounds) throws SQLException {
    try (PreparedStatement place = db.prepareStatement("INSERT INTO place VALUES ('wof', ?, 'admin', ?)");
      PreparedStatement s = db.prepareStatement(
        "INSERT INTO shard (source, id, role, element, geom) VALUES ('wof', ?, 'boundary', 0, ?)");
      PreparedStatement r = db.prepareStatement("INSERT INTO idx_shard_geom VALUES (last_insert_rowid(), ?, ?, ?, ?)");
      PreparedStatement g = db.prepareStatement("INSERT INTO geometry VALUES ('wof', ?, 'envelope', ?)")) {
      place.setString(1, Long.toString(id));
      place.setString(2, type);
      place.execute();
      s.setString(1, Long.toString(id));
      s.setBytes(2, shard);
      s.execute();
      r.setDouble(1, bounds[0]);
      r.setDouble(2, bounds[2]);
      r.setDouble(3, bounds[1]);
      r.setDouble(4, bounds[5]);
      r.execute();
      g.setString(1, Long.toString(id));
      g.setBytes(2, polygon(bounds));
      g.execute();
    }
  }

  private static WofAdmins database(Path dir) throws SQLException {
    Path path = dir.resolve("wof.db");
    try (Connection db = DriverManager.getConnection("jdbc:sqlite:" + path); Statement s = db.createStatement()) {
      s.execute("CREATE TABLE place (source TEXT, id TEXT, class TEXT, type TEXT)");
      s.execute("CREATE TABLE shard (source TEXT, id TEXT, role TEXT, element INTEGER, geom BLOB)");
      s.execute("CREATE VIRTUAL TABLE idx_shard_geom USING rtree(pkid, xmin, xmax, ymin, ymax)");
      s.execute("CREATE TABLE geometry (source TEXT, id TEXT, role TEXT, geom BLOB)");
      s.execute("CREATE TABLE name (source TEXT, id TEXT, lang TEXT, tag TEXT, abbr INTEGER, name TEXT)");
      add(db, 1, "country", square(0, 0, 100, 100));
      // A city, and in it what WOF also calls a locality.
      add(db, 2, "locality", square(10, 10, 20, 20));
      add(db, 3, "locality", square(11, 11, 12, 12));
      // Overlapping neighbourhoods.
      add(db, 4, "neighbourhood", square(11, 11, 13, 13));
      add(db, 5, "neighbourhood", square(11, 11, 11.5, 11.5));
      // Not a placetype kept.
      add(db, 6, "timezone", square(0, 0, 100, 100));
      // A county with a hole in it, and an area with no neighbours.
      add(db, 7, "county", polygon(square(30, 30, 40, 40), square(34, 34, 36, 36)), square(30, 30, 40, 40));
      s.execute("""
        INSERT INTO name VALUES
          ('wof', '2', 'und', 'default', 0, 'Zürich'),
          ('wof', '2', 'und', 'default', 1, 'ZH'),
          ('wof', '2', 'eng', 'preferred', 0, 'Zurich'),
          ('wof', '2', 'deu', 'preferred', 0, 'Zürich'),
          ('wof', '2', 'fra', 'variant', 0, 'Zurique')
        """);
    }
    return new WofAdmins(path);
  }

  @Test
  void oneAreaOfEachPlacetypeByItsRule(@TempDir Path dir) throws SQLException {
    WofAdmins admins = database(dir);
    // The largest locality, and the smallest neighbourhood.
    assertEquals(Map.of("country", 1L, "locality", 2L, "neighbourhood", 5L), admins.at(11.2, 11.2));
    // Only one of each here.
    assertEquals(Map.of("country", 1L, "locality", 2L, "neighbourhood", 4L), admins.at(12.5, 12.5));
    assertEquals(Map.of("country", 1L, "locality", 2L), admins.at(15, 15));
    assertEquals(Map.of("country", 1L), admins.at(50, 50));
    assertEquals(Map.of(), admins.at(-50, -50));
    admins.close();
  }

  @Test
  void pointsAreLookedUpFromTheRTree(@TempDir Path dir) throws SQLException {
    database(dir).close();
    try (Connection db = DriverManager.getConnection("jdbc:sqlite:" + dir.resolve("wof.db"));
      Statement s = db.createStatement();
      // With an index like the real database's, which the planner would otherwise be drawn to.
      ResultSet plan = executePlan(s)) {
      assertTrue(plan.next());
      String first = plan.getString("detail");
      assertTrue(first.contains("VIRTUAL TABLE"), first);
    }
  }

  private static ResultSet executePlan(Statement s) throws SQLException {
    s.execute("CREATE INDEX shard_idx_covering ON shard (source, id)");
    s.execute("CREATE UNIQUE INDEX place_idx_unique ON place (source, id)");
    return s.executeQuery("EXPLAIN QUERY PLAN " + WofAdmins.POINT_QUERY.replace("?1", "0").replace("?2", "0"));
  }

  @Test
  void holesAreOutside(@TempDir Path dir) throws SQLException {
    WofAdmins admins = database(dir);
    assertEquals(Map.of("country", 1L, "county", 7L), admins.at(31, 31));
    assertEquals(Map.of("country", 1L), admins.at(35, 35));
    admins.close();
  }

  @Test
  void placetypesComeSmallestFirst(@TempDir Path dir) throws SQLException {
    WofAdmins admins = database(dir);
    assertEquals(List.of("neighbourhood", "locality", "country"), List.copyOf(admins.at(11.2, 11.2).keySet()));
    admins.close();
  }

  @Test
  void namesAreTheGeocodersAndSkipWhatTheDefaultSays(@TempDir Path dir) throws SQLException {
    WofAdmins admins = database(dir);
    assertEquals(Optional.of("{\"abbr\":\"ZH\",\"eng\":\"Zurich\",\"name\":\"Zürich\"}"), admins.namesJson(2));
    assertEquals(Optional.empty(), admins.namesJson(3));
    admins.close();
  }

  @Test
  void aPlaceWithoutADefaultNameUsesItsEnglishOne() {
    assertEquals(Optional.of("{\"deu\":\"Köln\",\"name\":\"Cologne\"}"), WofAdmins.namesJson(List.of(
      new WofAdmins.Name("deu", "preferred", false, "Köln"),
      new WofAdmins.Name("eng", "preferred", false, "Cologne"))));
  }

  @Test
  void aKnownSizeBeatsAnUnknownOneAndTiesGoToTheLowerId() {
    assertTrue(WofAdmins.isBetter("neighbourhood", 1, "9", 0, "1"));
    assertFalse(WofAdmins.isBetter("neighbourhood", 0, "1", 1, "9"));
    assertTrue(WofAdmins.isBetter("locality", 1, "9", 0, "1"));
    assertTrue(WofAdmins.isBetter("locality", 2, "1", 2, "9"));
    assertFalse(WofAdmins.isBetter("locality", 2, "9", 2, "1"));
  }

  @Test
  void otherGeometriesAreRefused() {
    byte[] blob = polygon(square(0, 0, 1, 1));
    ByteBuffer.wrap(blob).order(ByteOrder.LITTLE_ENDIAN).putInt(39, 6);
    assertThrows(IllegalStateException.class, () -> WofAdmins.contains(blob, 0.5, 0.5));
    assertThrows(IllegalStateException.class, () -> WofAdmins.contains(new byte[60], 0.5, 0.5));
  }

  @Test
  void aMissingDatabaseFailsAtOnce(@TempDir Path dir) {
    assertThrows(IllegalArgumentException.class, () -> new WofAdmins(dir.resolve("none.db")));
  }
}
