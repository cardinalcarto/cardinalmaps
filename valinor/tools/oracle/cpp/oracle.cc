// Prints mainline 3.9.1 reference values for the valhalla-graphtile fixtures.
// Usage:
//   oracle <tile_dir>
//       Values from the Andorra tiles (shortcuts, access restrictions, bins, circles, ...).
//   oracle <tile_dir> speeds
//       GraphTile::GetSpeed for edges of tile 0/3015 over flow masks, times, and truck flags.
//   oracle compare <tile_dir_a> <tile_dir_b> <level> <tile_id>
//       Reads one tile from each directory through mainline's accessors
//       and prints every value that differs.
#include <valhalla/baldr/graphconstants.h>
#include <valhalla/baldr/graphreader.h>
#include <valhalla/baldr/tilehierarchy.h>
#include <valhalla/midgard/distanceapproximator.h>

#include <boost/property_tree/ptree.hpp>

#include <cstdio>
#include <cstring>
#include <map>
#include <set>
#include <string>
#include <vector>

using namespace valhalla;
using namespace valhalla::baldr;
using namespace valhalla::midgard;

namespace {

std::string hex(const void* data, size_t size) {
  std::string out;
  char buf[3];
  for (size_t i = 0; i < size; ++i) {
    snprintf(buf, sizeof(buf), "%02x", static_cast<const unsigned char*>(data)[i]);
    out += buf;
  }
  return out;
}

template <typename... Args> std::string fmt(const char* format, Args... args) {
  char buf[512];
  snprintf(buf, sizeof(buf), format, args...);
  return buf;
}

// GetSpeed for a few edges of tile 0/3015, one line per call:
//   speed <edge> <flow mask> <seconds> <is truck> <kph> <flow sources>
int speeds(GraphReader& reader) {
  auto tile = reader.GetGraphTile(GraphId(3015, 0, 0));
  // Edges 0, 7, and 42 have the speeds in tools/oracle/predicted-speeds; edge 1 has none.
  // (Edge 0 also has a truck speed, equal to its speed;
  // no edge in this tile has a truck speed below its speed.)
  const std::vector<uint32_t> edges = {0, 1, 7, 42};
  for (uint32_t i : edges) {
    const auto* de = tile->directededge(i);
    printf("edge %u speed %u truck_speed %u free_flow %u constrained_flow %u predicted %d\n", i,
           de->speed(), de->truck_speed(), de->free_flow_speed(), de->constrained_flow_speed(),
           de->has_predicted_speed());
  }
  const uint64_t times[] = {0,
                            1,
                            299,
                            300,
                            25199,
                            25200,
                            25201,
                            43200,
                            68399,
                            68400,
                            68401,
                            86399,
                            86400,
                            86400 + 25201,
                            604799,
                            604800,
                            604801,
                            604800 + 43200,
                            kInvalidSecondsOfWeek - 1,
                            kInvalidSecondsOfWeek,
                            kInvalidSecondsOfWeek + 1,
                            (1ull << 32) + 5,
                            (1ull << 40) + 25201};
  for (uint32_t i : edges) {
    const auto* de = tile->directededge(i);
    for (uint32_t mask = 0; mask < 16; ++mask) {
      for (uint64_t t : times) {
        for (int truck = 0; truck < 2; ++truck) {
          uint8_t sources = 0;
          uint32_t kph = tile->GetSpeed(de, mask, t, truck, &sources);
          printf("speed %u %u %llu %d %u %u\n", i, mask, (unsigned long long)t, truck, kph,
                 sources);
        }
      }
    }
  }
  // Every bucket of the week, at a varying offset into the bucket, for the predicted edges.
  for (uint32_t i : {7u, 42u}) {
    const auto* de = tile->directededge(i);
    for (uint64_t b = 0; b < kBucketsPerWeek; ++b) {
      uint64_t t = b * kSpeedBucketSizeSeconds + (b * 97) % kSpeedBucketSizeSeconds;
      uint8_t sources = 0;
      uint32_t kph = tile->GetSpeed(de, kPredictedFlowMask, t, false, &sources);
      printf("speed %u %u %llu %d %u %u\n", i, kPredictedFlowMask, (unsigned long long)t, 0, kph,
             sources);
    }
  }
  return 0;
}

// Everything mainline reads from one tile, one value per line.
std::vector<std::string> dump(GraphReader& reader, GraphId base) {
  std::vector<std::string> out;
  auto tile = reader.GetGraphTile(base);
  if (!tile) {
    out.push_back("tile missing or rejected");
    return out;
  }
  const GraphTileHeader* h = tile->header();
  out.push_back("header version " + h->version());
  out.push_back(fmt("header dataset_id %llu", (unsigned long long)h->dataset_id()));
  out.push_back(fmt("header density %u quality %u %u %u", h->density(), h->name_quality(),
                    h->speed_quality(), h->exit_quality()));
  out.push_back(fmt("header has_elevation %d has_ext_directededge %d", h->has_elevation(),
                    h->has_ext_directededge()));
  out.push_back(fmt("header base_ll %.9g %.9g", h->base_ll().lng(), h->base_ll().lat()));
  out.push_back(fmt("header counts nodes %u edges %u predicted %u transitions %u turnlanes %u",
                    h->nodecount(), h->directededgecount(), h->predictedspeeds_count(),
                    h->transitioncount(), h->turnlane_count()));
  out.push_back(fmt("header counts signs %u access %u admins %u", h->signcount(),
                    h->access_restriction_count(), h->admincount()));
  out.push_back(fmt("header counts transit departures %u stops %u routes %u schedules %u "
                    "transfers %u",
                    h->departurecount(), h->stopcount(), h->routecount(), h->schedulecount(),
                    h->transfercount()));
  out.push_back(fmt("header date_created %u has_bounding_circles %d", h->date_created(),
                    h->has_bounding_circles()));
  out.push_back(fmt("header tile_checksum %llu build_id %u end_offset %u",
                    (unsigned long long)h->tile_checksum(), h->build_id(), h->end_offset()));

  for (uint32_t i = 0; i < h->admincount(); ++i) {
    auto a = tile->admininfo(i);
    out.push_back(fmt("admin %u ", i) + a.country_text() + "|" + a.state_text() + "|" +
                  a.country_iso() + "|" + a.state_iso());
  }
  for (uint32_t i = 0; i < h->nodecount(); ++i) {
    const NodeInfo* node = tile->node(i);
    auto ll = node->latlng(h->base_ll());
    out.push_back(fmt("node %u %.9g %.9g ", i, ll.lng(), ll.lat()) + hex(node, sizeof(NodeInfo)));
    for (uint32_t t = 0; t < node->transition_count(); ++t) {
      const auto* tr = tile->transition(node->transition_index() + t);
      out.push_back(fmt("node %u transition %u ", i, t) + hex(tr, sizeof(NodeTransition)));
    }
    if (node->named_intersection()) {
      for (const auto& sign : tile->GetSigns(i, true))
        out.push_back(fmt("node %u sign %d ", i, static_cast<int>(sign.type())) + sign.text());
    }
  }
  for (uint32_t i = 0; i < h->directededgecount(); ++i) {
    const DirectedEdge* de = tile->directededge(i);
    GraphId id(base.tileid(), base.level(), i);
    out.push_back(fmt("edge %u ", i) + hex(de, sizeof(DirectedEdge)));
    if (h->has_ext_directededge())
      out.push_back(fmt("edge %u ext ", i) +
                    hex(tile->ext_directededge(i), sizeof(DirectedEdgeExt)));
    auto ei = tile->edgeinfo(de);
    std::string names;
    for (const auto& name : ei.GetNames())
      names += "[" + name + "]";
    out.push_back(fmt("edge %u info way %llu speed_limit %u bike_network %u mean_elevation %.9g "
                      "names ",
                      i, (unsigned long long)ei.wayid(), ei.speed_limit(), ei.bike_network(),
                      ei.mean_elevation()) +
                  names);
    out.push_back(fmt("edge %u shape ", i) + ei.encoded_shape());
    if (de->sign()) {
      for (const auto& sign : tile->GetSigns(i))
        out.push_back(fmt("edge %u sign %d ", i, static_cast<int>(sign.type())) + sign.text());
    }
    if (de->laneconnectivity()) {
      for (const auto& lc : tile->GetLaneConnectivity(i))
        out.push_back(fmt("edge %u lanes from %llu to %u ", i, (unsigned long long)lc.from(),
                          lc.to()) +
                      lc.from_lanes() + "|" + lc.to_lanes());
    }
    if (de->turnlanes()) {
      std::string masks;
      for (auto m : tile->turnlanes(i))
        masks += fmt(" %u", m);
      out.push_back(fmt("edge %u turnlanes", i) + masks);
    }
    for (const auto& r : tile->GetAccessRestrictions(i, kAllAccess))
      out.push_back(fmt("edge %u access type %d modes %u value %llu", i,
                        static_cast<int>(r.type()), r.modes(), (unsigned long long)r.value()));
    for (bool forward : {true, false}) {
      for (const auto& cr : tile->GetComplexRestrictions(forward, id, kAllAccess))
        out.push_back(fmt("edge %u complex %d ", i, forward) + hex(&cr, cr.SizeOf()));
    }
    std::string speeds;
    if (de->has_predicted_speed()) {
      for (uint32_t b = 0; b < kBucketsPerWeek; ++b)
        speeds += fmt(" %u", tile->GetSpeed(de, kPredictedFlowMask, b * kSpeedBucketSizeSeconds));
    }
    out.push_back(fmt("edge %u get_speed default %u night %u day %u predicted", i,
                      tile->GetSpeed(de, kDefaultFlowMask),
                      tile->GetSpeed(de, kDefaultFlowMask, 3600),
                      tile->GetSpeed(de, kDefaultFlowMask, 43200)) +
                  speeds);
  }
  for (size_t bin = 0; bin < kBinCount; ++bin) {
    std::string edges;
    for (const auto& e : tile->GetBin(bin))
      edges += fmt(" %u/%u/%u", e.level(), e.tileid(), e.id());
    out.push_back(fmt("bin %zu", bin) + edges);
    std::string circles;
    for (const auto& c : tile->GetBoundingCircles(bin))
      circles += fmt(" %u,%u,%u", c.x_offset(), c.y_offset(), c.radius_index());
    out.push_back(fmt("bin %zu circles", bin) + circles);
  }
  return out;
}

int compare(const char* dir_a, const char* dir_b, uint32_t level, uint32_t tile_id) {
  boost::property_tree::ptree pt_a, pt_b;
  pt_a.put("tile_dir", dir_a);
  pt_b.put("tile_dir", dir_b);
  GraphReader reader_a(pt_a), reader_b(pt_b);
  GraphId base(tile_id, level, 0);
  auto a = dump(reader_a, base);
  auto b = dump(reader_b, base);
  printf("compare %u/%u: %zu and %zu values\n", level, tile_id, a.size(), b.size());
  size_t differences = 0;
  const std::string none = "(none)";
  for (size_t i = 0; i < std::max(a.size(), b.size()); ++i) {
    const auto& x = i < a.size() ? a[i] : none;
    const auto& y = i < b.size() ? b[i] : none;
    if (x != y) {
      ++differences;
      printf("- %s\n+ %s\n", x.c_str(), y.c_str());
    }
  }
  printf("%zu differences\n", differences);
  return 0;
}

} // namespace

int main(int argc, char** argv) {
  if (argc == 6 && strcmp(argv[1], "compare") == 0) {
    return compare(argv[2], argv[3], std::stoul(argv[4]), std::stoul(argv[5]));
  }
  if (argc == 3 && strcmp(argv[2], "speeds") == 0) {
    boost::property_tree::ptree pt;
    pt.put("tile_dir", argv[1]);
    GraphReader reader(pt);
    return speeds(reader);
  }
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
