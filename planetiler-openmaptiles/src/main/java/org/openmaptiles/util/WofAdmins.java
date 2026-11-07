package org.openmaptiles.util;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;
import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.TreeMap;
import java.util.concurrent.ConcurrentHashMap;
import java.util.stream.Collectors;
import org.sqlite.SQLiteConfig;
import org.sqlite.SQLiteOpenMode;

/**
 * The Who's On First areas containing a point, from a WOF SpatiaLite database read with plain SQLite: its {@code shard}
 * table holds each area's polygons cut into small pieces, with an R-tree over them.
 * <p>
 * A point can be in several areas of one placetype, as WOF's localities include what are really neighbourhoods. Each
 * placetype gets one: the largest for localities and local admins, since that's the city in an address, and otherwise
 * the smallest, as the most specific. The geocoder chooses by the same rule.
 * <p>
 * Instances are safe to share between threads.
 */
public final class WofAdmins {

  /** The placetypes kept, smallest first. */
  public static final List<String> PLACETYPES = List.of("microhood", "neighbourhood", "macrohood", "borough",
    "locality", "localadmin", "county", "macrocounty", "region", "macroregion", "dependency", "country");

  /**
   * Placetypes whose names ship with the app rather than in tiles: they hold most of the bytes, would repeat in every
   * tile, and rarely change.
   */
  public static final Set<String> NAMED_IN_APP = Set.of("region", "macroregion", "dependency", "country");

  private static final Set<String> LARGEST_WINS = Set.of("locality", "localadmin");
  /*
   * CROSS JOIN makes SQLite join in this order, from the R-tree out. Left to itself, it can start from every shard of
   * the source and take seconds for each point rather than microseconds.
   */
  static final String POINT_QUERY = """
    SELECT s.id, p.type, s.geom
    FROM idx_shard_geom r
    CROSS JOIN shard s ON s.rowid = r.pkid
    CROSS JOIN place p ON p.source = s.source AND p.id = s.id
    WHERE r.xmin <= ?1 AND r.xmax >= ?1 AND r.ymin <= ?2 AND r.ymax >= ?2
    AND s.source = 'wof' AND +p.type IN (%s)
    """.formatted(PLACETYPES.stream().map(t -> "'" + t + "'").collect(Collectors.joining(",")));
  private static final String SIZE_QUERY =
    "SELECT geom FROM geometry WHERE source = 'wof' AND id = ? AND role = 'envelope'";
  private static final String NAMES_QUERY =
    "SELECT lang, tag, abbr, name FROM name WHERE source = 'wof' AND id = ?";
  private static final ObjectMapper JSON = new ObjectMapper();

  private final String url;
  private final List<Queries> open = new ArrayList<>();
  private final ThreadLocal<Queries> queries = ThreadLocal.withInitial(this::connect);
  private final Map<String, Double> sizes = new ConcurrentHashMap<>();
  private final Map<Long, Optional<String>> names = new ConcurrentHashMap<>();

  private record Queries(Connection connection, PreparedStatement point, PreparedStatement size,
    PreparedStatement names) {}

  /** Opens the database at {@code path}, failing now if it isn't a WOF SpatiaLite database. */
  public WofAdmins(Path path) {
    if (!Files.isRegularFile(path)) {
      throw new IllegalArgumentException("no Who's On First database at " + path);
    }
    url = "jdbc:sqlite:file:" + path.toAbsolutePath() + "?immutable=1";
    at(0, 0);
  }

  private Queries connect() {
    SQLiteConfig config = new SQLiteConfig();
    config.setReadOnly(true);
    config.setOpenMode(SQLiteOpenMode.OPEN_URI);
    try {
      Connection connection = DriverManager.getConnection(url, config.toProperties());
      Queries result = new Queries(connection, connection.prepareStatement(POINT_QUERY),
        connection.prepareStatement(SIZE_QUERY), connection.prepareStatement(NAMES_QUERY));
      synchronized (open) {
        open.add(result);
      }
      return result;
    } catch (SQLException e) {
      throw new IllegalStateException("opening " + url, e);
    }
  }

  /** The area of each placetype containing a point in degrees, by placetype. */
  public Map<String, Long> at(double lng, double lat) {
    Map<String, List<String>> containing = new HashMap<>();
    try {
      PreparedStatement query = queries.get().point();
      query.setDouble(1, lng);
      query.setDouble(2, lat);
      try (ResultSet rows = query.executeQuery()) {
        while (rows.next()) {
          String id = rows.getString(1);
          String placetype = rows.getString(2);
          List<String> ids = containing.computeIfAbsent(placetype, t -> new ArrayList<>());
          if (!ids.contains(id) && contains(rows.getBytes(3), lng, lat)) {
            ids.add(id);
          }
        }
      }
    } catch (SQLException e) {
      throw new IllegalStateException("looking up " + lng + "," + lat, e);
    }
    Map<String, Long> result = new LinkedHashMap<>();
    for (String placetype : PLACETYPES) {
      List<String> ids = containing.get(placetype);
      if (ids != null && !ids.isEmpty()) {
        result.put(placetype, Long.parseLong(choose(placetype, ids)));
      }
    }
    return result;
  }

  private String choose(String placetype, List<String> ids) {
    if (ids.size() == 1) {
      return ids.getFirst();
    }
    String best = null;
    for (String id : ids) {
      if (best == null || isBetter(placetype, size(id), id, size(best), best)) {
        best = id;
      }
    }
    return best;
  }

  /**
   * Whether an area of a placetype beats another: see the class documentation. The size of an area with no known size
   * is zero, which never beats a known one. Ties go to the lower ID, so that builds are repeatable.
   */
  static boolean isBetter(String placetype, double size, String id, double otherSize, String otherId) {
    if (size != otherSize) {
      if (size == 0 || otherSize == 0) {
        return otherSize == 0;
      }
      return LARGEST_WINS.contains(placetype) ? size > otherSize : size < otherSize;
    }
    return Long.parseLong(id) < Long.parseLong(otherId);
  }

  /** The area of a place's bounding box in square degrees, or zero if it has none. */
  private double size(String id) {
    return sizes.computeIfAbsent(id, key -> {
      try {
        PreparedStatement query = queries.get().size();
        query.setString(1, key);
        try (ResultSet rows = query.executeQuery()) {
          return rows.next() ? boundsArea(rows.getBytes(1)) : 0d;
        }
      } catch (SQLException e) {
        throw new IllegalStateException("reading the size of " + key, e);
      }
    });
  }

  /**
   * An area's names as JSON: {@code name} is its default name, {@code abbr} its abbreviation if it has one, and other
   * keys are ISO 639-3 language codes with its preferred name in that language, where that differs from the default.
   * Empty if it has no names.
   */
  public Optional<String> namesJson(long id) {
    return names.computeIfAbsent(id, key -> {
      try {
        PreparedStatement query = queries.get().names();
        query.setString(1, Long.toString(key));
        List<Name> rows = new ArrayList<>();
        try (ResultSet result = query.executeQuery()) {
          while (result.next()) {
            rows.add(new Name(result.getString(1), result.getString(2), result.getBoolean(3), result.getString(4)));
          }
        }
        return namesJson(rows);
      } catch (SQLException e) {
        throw new IllegalStateException("reading the names of " + key, e);
      }
    });
  }

  record Name(String lang, String tag, boolean abbr, String name) {}

  /** As the geocoder's indexer chooses a place's names. */
  static Optional<String> namesJson(List<Name> rows) {
    List<Name> full = rows.stream().filter(n -> !n.abbr && n.name != null && !n.name.isEmpty()).toList();
    Optional<String> fallback = full.stream()
      .filter(n -> n.lang.equals("eng") && n.tag.equals("preferred"))
      .findFirst()
      .or(() -> full.stream().findFirst())
      .map(Name::name);
    Optional<String> name = full.stream()
      .filter(n -> n.lang.equals("und") && n.tag.equals("default"))
      .findFirst()
      .map(Name::name)
      .or(() -> fallback);
    if (name.isEmpty()) {
      return Optional.empty();
    }
    Map<String, String> json = new TreeMap<>();
    json.put("name", name.get());
    for (Name n : full) {
      if (n.tag.equals("preferred") && !n.lang.equals("und") && !n.name.equals(name.get())) {
        json.putIfAbsent(n.lang, n.name);
      }
    }
    rows.stream()
      .filter(n -> n.abbr && n.name != null && !n.name.isEmpty())
      .min((a, b) -> abbreviationRank(a) - abbreviationRank(b))
      .ifPresent(n -> json.put("abbr", n.name));
    try {
      return Optional.of(JSON.writeValueAsString(json));
    } catch (JsonProcessingException e) {
      throw new IllegalStateException(e);
    }
  }

  private static int abbreviationRank(Name name) {
    return switch (name.lang) {
      case "eng" -> 0;
      case "und" -> 1;
      default -> 2;
    };
  }

  /*
   * A SpatiaLite geometry blob: 0x00, byte order (1 is little endian), SRID (int), minimum x and y and maximum x and y
   * (doubles), 0x7C, class (int), the geometry, 0xFE. Shards are always uncompressed polygons, class 3: a ring count,
   * and for each ring a point count and its x and y doubles.
   */
  private static final int HEADER = 39;
  private static final int POLYGON = 3;

  private static ByteBuffer blob(byte[] bytes) {
    if (bytes.length < HEADER + 5 || bytes[0] != 0 || bytes[HEADER - 1] != 0x7C ||
      bytes[bytes.length - 1] != (byte) 0xFE) {
      throw new IllegalStateException("not a SpatiaLite geometry");
    }
    return ByteBuffer.wrap(bytes).order(bytes[1] == 1 ? ByteOrder.LITTLE_ENDIAN : ByteOrder.BIG_ENDIAN);
  }

  /** The area of a geometry's bounding box, whatever its class. */
  static double boundsArea(byte[] bytes) {
    ByteBuffer buffer = blob(bytes);
    return (buffer.getDouble(22) - buffer.getDouble(6)) * (buffer.getDouble(30) - buffer.getDouble(14));
  }

  /** Whether a polygon in a SpatiaLite blob contains a point: it's inside an odd number of its rings. */
  static boolean contains(byte[] bytes, double x, double y) {
    ByteBuffer buffer = blob(bytes);
    if (x < buffer.getDouble(6) || y < buffer.getDouble(14) || x > buffer.getDouble(22) || y > buffer.getDouble(30)) {
      return false;
    }
    int geometryClass = buffer.getInt(HEADER);
    if (geometryClass != POLYGON) {
      throw new IllegalStateException("expected a polygon, but its class is " + geometryClass);
    }
    int offset = HEADER + 4;
    int rings = buffer.getInt(offset);
    offset += 4;
    boolean inside = false;
    for (int ring = 0; ring < rings; ring++) {
      int points = buffer.getInt(offset);
      offset += 4;
      double previousX = buffer.getDouble(offset + (points - 1) * 16);
      double previousY = buffer.getDouble(offset + (points - 1) * 16 + 8);
      for (int point = 0; point < points; point++) {
        double pointX = buffer.getDouble(offset);
        double pointY = buffer.getDouble(offset + 8);
        offset += 16;
        if ((pointY > y) != (previousY > y) &&
          x < pointX + (y - pointY) / (previousY - pointY) * (previousX - pointX)) {
          inside = !inside;
        }
        previousX = pointX;
        previousY = pointY;
      }
    }
    return inside;
  }

  /** Closes every thread's connection. */
  public void close() {
    synchronized (open) {
      for (Queries q : open) {
        try {
          q.connection().close();
        } catch (SQLException e) {
          // Only finishing up; nothing depends on it.
        }
      }
      open.clear();
    }
  }
}
