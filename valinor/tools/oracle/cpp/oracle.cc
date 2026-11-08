// Prints mainline 3.9.1 reference values for the valhalla-graphtile fixtures.
// Usage: oracle <tile_dir>
#include <valhalla/baldr/graphconstants.h>
#include <valhalla/baldr/graphreader.h>
#include <valhalla/baldr/tilehierarchy.h>
#include <valhalla/midgard/distanceapproximator.h>

#include <boost/property_tree/ptree.hpp>

#include <cstdio>
#include <map>
#include <set>

using namespace valhalla;
using namespace valhalla::baldr;
using namespace valhalla::midgard;

int main(int argc, char** argv) {
  boost::property_tree::ptree pt;
  pt.put("tile_dir", argv[1]);
  GraphReader reader(pt);

  // Shortcuts in the level 0 tile: which edges each shortcut contains.
  GraphId base(3015, 0, 0);
  auto tile = reader.GetGraphTile(base);
  std::map<uint64_t, uint64_t> edge_to_shortcut;
  size_t shortcuts = 0;
  for (uint32_t i = 0; i < tile->header()->directededgecount(); ++i) {
    GraphId id(3015, 0, i);
    const DirectedEdge* edge = tile->directededge(i);
    if (edge->is_shortcut()) {
      auto members = reader.RecoverShortcut(id);
      // RecoverShortcut returns the shortcut itself when recovery fails.
      if (members.size() == 1 && members[0] == id) {
        printf("shortcut %u unrecoverable\n", i);
        continue;
      }
      ++shortcuts;
      printf("shortcut %u:", i);
      for (auto m : members) {
        printf(" %u/%u/%u", m.level(), m.tileid(), m.id());
        edge_to_shortcut[m.value] = id.value;
      }
      printf("\n");
    }
  }
  size_t level0_members = 0;
  std::set<uint64_t> unique_shortcuts;
  for (auto [edge, sc] : edge_to_shortcut) {
    if (GraphId(edge).tile_base() == base) {
      ++level0_members;
      unique_shortcuts.insert(sc);
    }
  }
  printf("shortcut count %zu, member edges in tile %zu, unique shortcuts with members in tile %zu\n",
         shortcuts, level0_members, unique_shortcuts.size());

  // Access restrictions in the level 0 tile.
  size_t all = 0, auto_or_golf = 0, truck = 0;
  for (uint32_t i = 0; i < tile->header()->directededgecount(); ++i) {
    for (const auto& r : tile->GetAccessRestrictions(i, kAllAccess)) {
      (void)r;
      ++all;
    }
    for (const auto& r : tile->GetAccessRestrictions(i, kAutoAccess | 2048)) {
      (void)r;
      ++auto_or_golf;
    }
    for (const auto& r : tile->GetAccessRestrictions(i, kTruckAccess)) {
      (void)r;
      ++truck;
    }
  }
  printf("access restrictions: all %zu, auto|golf %zu, truck %zu\n", all, auto_or_golf, truck);

  // Misc values for the level 0 tile.
  {
    // The first access restriction belongs to the lowest edge index that has any.
    for (uint32_t i = 0; i < tile->header()->directededgecount(); ++i) {
      auto rs = tile->GetAccessRestrictions(i);
      if (!rs.first.empty()) {
        const auto& r = rs.first.front();
        printf("first access restriction: edge %u type %d modes %u value %llu\n", r.edgeindex(),
               static_cast<int>(r.type()), r.modes(), (unsigned long long)r.value());
        break;
      }
    }
    auto ll = tile->node(0)->latlng(tile->header()->base_ll());
    printf("node 0 %.17g %.17g\n", ll.lng(), ll.lat());
    printf("node 0 access %u\n", tile->node(0)->access());
    uint32_t n = tile->header()->directededgecount();
    for (uint32_t i : {0u, 2000u, n - 1}) {
      auto ei = tile->edgeinfo(tile->directededge(i));
      printf("edge %u way %llu names:", i, (unsigned long long)ei.wayid());
      for (const auto& name : ei.GetNames())
        printf(" [%s]", name.c_str());
      printf("\n");
    }
    {
      size_t shown = 0;
      for (uint32_t i = 0; i < n && shown < 4; ++i) {
        auto ei = tile->edgeinfo(tile->directededge(i));
        auto names = ei.GetNames();
        if (names.size() >= 2) {
          ++shown;
          printf("multi-name edge %u way %llu:", i, (unsigned long long)ei.wayid());
          for (const auto& name : names)
            printf(" [%s]", name.c_str());
          printf("\n");
        }
      }
    }
    // Opposing edge indices within the tile, as the Valinor test computes them.
    uint64_t acc = 0;
    size_t count = 0;
    for (uint32_t i = 0; i < n; ++i) {
      auto opp = reader.GetOpposingEdgeId(GraphId(3015, 0, i));
      if (opp.is_valid() && opp.tile_base() == base) {
        acc = acc * 31 + opp.id();
        ++count;
        if (i < 5)
          printf("opp %u -> %u\n", i, opp.id());
      }
    }
    printf("opp count %zu hash %llu\n", count, (unsigned long long)acc);
    for (uint32_t i = 0; i < tile->header()->admincount(); ++i) {
      auto a = tile->admininfo(i);
      printf("admin %u %s %s\n", i, a.country_iso().c_str(), a.state_iso().c_str());
    }
  }

  // Bounding circles of the level 2 tiles, decoded the way loki does.
  const auto& tiles = TileHierarchy::levels().back().tiles;
  for (uint32_t tileid : {762485u, 763927u}) {
    GraphId l2(tileid, 2, 0);
    auto t = reader.GetGraphTile(l2);
    printf("tile %u has_bounding_circles %d\n", tileid, t->header()->has_bounding_circles());
    auto minx = tiles.TileBounds(tileid).minx();
    auto miny = tiles.TileBounds(tileid).miny();
    for (size_t bin = 0; bin < kBinCount; ++bin) {
      auto edges = t->GetBin(bin);
      auto circles = t->GetBoundingCircles(bin);
      auto lat_offset = (bin / kBinsDim) * tiles.SubdivisionSize() + tiles.SubdivisionSize() / 2;
      auto lng_offset = (bin % kBinsDim) * tiles.SubdivisionSize() + tiles.SubdivisionSize() / 2;
      PointLL bin_center(minx + lng_offset, miny + lat_offset);
      DistanceApproximator<PointLL> approx(bin_center);
      printf("bin %zu center %.17g %.17g edges %zu circles %zu\n", bin, bin_center.lng(),
             bin_center.lat(), edges.size(), circles.size());
      for (size_t i = 0; i < circles.size() && i < 3; ++i) {
        const auto& c = circles[i];
        auto decoded = c.get(approx, bin_center);
        printf("  %zu edge %u/%u/%u x=%u y=%u r=%u valid=%d center %.17g %.17g radius %.17g\n", i,
               edges[i].level(), edges[i].tileid(), edges[i].id(), c.x_offset(), c.y_offset(),
               c.radius_index(), c.is_valid(), decoded.first.lng(), decoded.first.lat(),
               decoded.second);
      }
      size_t invalid = 0;
      for (const auto& c : circles)
        invalid += !c.is_valid();
      printf("  invalid %zu\n", invalid);
    }
  }
  return 0;
}
