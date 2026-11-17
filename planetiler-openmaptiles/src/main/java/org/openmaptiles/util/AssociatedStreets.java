package org.openmaptiles.util;

import static com.onthegomap.planetiler.util.MemoryEstimator.CLASS_HEADER_BYTES;

import com.carrotsearch.hppc.LongObjectHashMap;
import com.onthegomap.planetiler.ForwardingProfile;
import com.onthegomap.planetiler.reader.SourceFeature;
import com.onthegomap.planetiler.reader.osm.OsmElement;
import com.onthegomap.planetiler.reader.osm.OsmRelationInfo;
import com.onthegomap.planetiler.util.MemoryEstimator;
import java.util.List;

/**
 * The street of an element from {@code addr:street}, or else from the {@code associatedStreet} or {@code street}
 * relation it's a house in, as airmail's indexer does: a named relation's {@code house} and {@code address} members.
 * <p>
 * Planetiler gives ways and relations the relations they're members of, but not nodes, so this keeps the streets of
 * node members itself. One instance serves every layer that needs streets, so relations are only noted once.
 */
public final class AssociatedStreets implements ForwardingProfile.OsmRelationPreprocessor {

  private static final String STREET_TAG = "addr:street";

  private final LongObjectHashMap<String> nodeStreets = new LongObjectHashMap<>();

  private static boolean isHouse(String role) {
    return "house".equals(role) || "address".equals(role);
  }

  @Override
  public List<OsmRelationInfo> preprocessOsmRelation(OsmElement.Relation relation) {
    if (!relation.hasTag("type", "associatedStreet") && !relation.hasTag("type", "street")) {
      return null;
    }
    String name = relation.getString("name");
    if (name == null || name.isBlank()) {
      return null;
    }
    boolean hasWaysOrRelations = false;
    for (var member : relation.members()) {
      if (!isHouse(member.role())) {
        continue;
      }
      if (member.type() == OsmElement.Type.NODE) {
        synchronized (nodeStreets) {
          nodeStreets.put(member.ref(), name);
        }
      } else {
        hasWaysOrRelations = true;
      }
    }
    return hasWaysOrRelations ? List.of(new StreetRelation(relation.id(), name)) : null;
  }

  /** The street of {@code source}: its {@code addr:street}, or else that of a relation it's a house in, or null. */
  public String streetOf(SourceFeature source) {
    String street = source.getString(STREET_TAG);
    if (street != null && !street.isBlank()) {
      return street;
    }
    if (source.isPoint()) {
      // Only nodes are points. Pass 2 reads this map once pass 1 has finished filling it, so it needs no lock.
      return nodeStreets.get(source.id());
    }
    for (var member : source.relationInfo(StreetRelation.class)) {
      if (isHouse(member.role())) {
        return member.relation().street();
      }
    }
    return street;
  }

  @Override
  public void release() {
    synchronized (nodeStreets) {
      nodeStreets.clear();
      nodeStreets.release();
    }
  }

  record StreetRelation(long id, String street) implements OsmRelationInfo {

    @Override
    public long estimateMemoryUsageBytes() {
      return CLASS_HEADER_BYTES + MemoryEstimator.estimateSizeLong(id) + MemoryEstimator.estimateSize(street);
    }
  }
}
