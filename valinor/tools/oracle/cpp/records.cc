// Prints mainline 3.9.1 accessor values for the directed edges, nodes, and node transitions
// of every tile in the valhalla-graphtile fixtures, one whitespace-separated row per record.
//
// Usage: records <tile_dir> [all]
//
// Tiles missing from <tile_dir> are skipped.
//
// By default, each tile contributes a sample: every record whose index is a multiple of a
// per-tile stride (so roughly 100 per tile), plus every record that shows a column value
// not seen earlier in that tile (so rare flags and enum values are always covered).
// Columns listed in `kNoCoverage` (offsets, lengths, ids, and the like) don't count towards
// coverage, since nearly every value is new.
// Pass `all` to print every record instead.
//
// The output is the expected-values fixture for the Rust tests in
// `valhalla-graphtile/src/graph_tile/{directed_edge,node}.rs`.
#include <valhalla/baldr/graphconstants.h>
#include <valhalla/baldr/graphreader.h>

#include <boost/property_tree/ptree.hpp>

#include <cstdio>
#include <cstring>
#include <set>
#include <string>
#include <utility>
#include <vector>

using namespace valhalla;
using namespace valhalla::baldr;

namespace {

struct Row {
  std::vector<std::string> names;
  std::vector<std::string> values;
  void add(const char* name, const std::string& value) {
    names.emplace_back(name);
    values.push_back(value);
  }
  void add(const char* name, unsigned long long value) {
    add(name, std::to_string(value));
  }
  void add_signed(const char* name, long long value) {
    add(name, std::to_string(value));
  }
  void add_float(const char* name, double value) {
    char buf[64];
    snprintf(buf, sizeof buf, "%.7f", value);
    add(name, std::string(buf));
  }
};

const std::set<std::string> kNoCoverage = {
    "idx",         "end_node",   "edge_info_offset", "length",        "line_id",
    "opp_index",   "lng",        "lat",              "edge_index",    "transition_index",
    "headings",    "elevation",  "stop_impact",      "turn_type",     "edge_to_left",
    "edge_to_right", "speed",    "name_consistency", "local_edge_idx", "opp_local_idx",
    "end_node_value",
};

// Encodes a per-local-index accessor (0..=7) as a string of 8 digits, index 0 first.
template <typename F> std::string per_index(F f) {
  std::string s;
  for (uint32_t i = 0; i <= kMaxLocalEdgeIndex; ++i)
    s += std::to_string(static_cast<unsigned>(f(i)));
  return s;
}

Row edge_row(uint32_t idx, const DirectedEdge* e) {
  Row r;
  r.add("idx", idx);
  r.add("end_node", e->endnode().value);
  r.add("restrictions", e->restrictions());
  r.add("opp_index", e->opp_index());
  r.add("forward", e->forward());
  r.add("leaves_tile", e->leaves_tile());
  r.add("country_crossing", e->ctry_crossing());
  r.add("edge_info_offset", e->edgeinfo_offset());
  r.add("access_restriction", e->access_restriction());
  r.add("start_restriction", e->start_restriction());
  r.add("end_restriction", e->end_restriction());
  r.add("complex_restriction", e->part_of_complex_restriction());
  r.add("dest_only", e->destonly());
  r.add("not_thru", e->not_thru());
  r.add("speed", e->speed());
  r.add("free_flow_speed", e->free_flow_speed());
  r.add("constrained_flow_speed", e->constrained_flow_speed());
  r.add("truck_speed", e->truck_speed());
  r.add("name_consistency", e->name_consistency());
  r.add("use", static_cast<unsigned>(e->use()));
  r.add("lane_count", e->lanecount());
  r.add("density", e->density());
  r.add("classification", static_cast<unsigned>(e->classification()));
  r.add("surface", static_cast<unsigned>(e->surface()));
  r.add("toll", e->toll());
  r.add("roundabout", e->roundabout());
  r.add("truck_route", e->truck_route());
  r.add("has_predicted_speed", e->has_predicted_speed());
  r.add("forward_access", e->forwardaccess());
  r.add("reverse_access", e->reverseaccess());
  r.add_signed("max_up_slope", e->max_up_slope());
  r.add_signed("max_down_slope", e->max_down_slope());
  r.add("sac_scale", static_cast<unsigned>(e->sac_scale()));
  r.add("cycle_lane", static_cast<unsigned>(e->cyclelane()));
  r.add("bike_network", e->bike_network());
  r.add("use_sidepath", e->use_sidepath());
  r.add("dismount", e->dismount());
  r.add("sidewalk_left", e->sidewalk_left());
  r.add("sidewalk_right", e->sidewalk_right());
  r.add("shoulder", e->shoulder());
  r.add("lane_connectivity", e->laneconnectivity());
  r.add("turn_lanes", e->turnlanes());
  r.add("sign", e->sign());
  r.add("internal", e->internal());
  r.add("tunnel", e->tunnel());
  r.add("bridge", e->bridge());
  r.add("traffic_signal", e->traffic_signal());
  r.add("dead_end", e->deadend());
  r.add("bss_connection", e->bss_connection());
  r.add("stop_sign", e->stop_sign());
  r.add("yield_sign", e->yield_sign());
  r.add("hov_type", static_cast<unsigned>(e->hov_type()));
  r.add("indoor", e->indoor());
  r.add("lit", e->lit());
  r.add("dest_only_hgv", e->destonly_hgv());
  r.add("turn_type", per_index([&](uint32_t i) { return static_cast<unsigned>(e->turntype(i)); }));
  r.add("edge_to_left", per_index([&](uint32_t i) { return e->edge_to_left(i); }));
  r.add("length", e->length());
  r.add("weighted_grade", e->weighted_grade());
  r.add("curvature", e->curvature());
  r.add("stop_impact", per_index([&](uint32_t i) { return e->stopimpact(i); }));
  r.add("edge_to_right", per_index([&](uint32_t i) { return e->edge_to_right(i); }));
  r.add("line_id", e->lineid());
  r.add("local_edge_idx", e->localedgeidx());
  r.add("opp_local_idx", e->opp_local_idx());
  r.add("shortcut", e->shortcut());
  r.add("superseded", e->superseded());
  r.add("is_shortcut", e->is_shortcut());
  r.add("speed_type", static_cast<unsigned>(e->speed_type()));
  r.add("named", e->named());
  r.add("link", e->link());
  // Derived predicates.
  r.add("is_road", e->is_road());
  r.add("is_transit_line", e->IsTransitLine());
  r.add("can_form_shortcut", e->can_form_shortcut());
  r.add("unpaved", e->unpaved());
  r.add("is_hov_only", e->is_hov_only());
  r.add("has_flow_speed", e->has_flow_speed());
  return r;
}

Row node_row(uint32_t idx, const NodeInfo* n, const midgard::PointLL& base) {
  Row r;
  r.add("idx", idx);
  auto ll = n->latlng(base);
  r.add_float("lng", ll.lng());
  r.add_float("lat", ll.lat());
  r.add("edge_index", n->edge_index());
  r.add("edge_count", n->edge_count());
  r.add("access", n->access());
  r.add("intersection", static_cast<unsigned>(n->intersection()));
  r.add("admin_index", n->admin_index());
  r.add("timezone", n->timezone());
  r.add("local_driveability",
        per_index([&](uint32_t i) { return static_cast<unsigned>(n->local_driveability(i)); }));
  r.add("density", n->density());
  r.add("type", static_cast<unsigned>(n->type()));
  r.add("local_edge_count", n->local_edge_count());
  r.add("drive_on_right", n->drive_on_right());
  r.add_float("elevation", n->elevation());
  r.add("tagged_access", n->tagged_access());
  r.add("private_access", n->private_access());
  r.add("cash_only_toll", n->cash_only_toll());
  r.add("mode_change", n->mode_change());
  r.add("named_intersection", n->named_intersection());
  r.add("traffic_signal", n->traffic_signal());
  r.add("transition_index", n->transition_index());
  r.add("transition_count", n->transition_count());
  std::string headings;
  for (uint32_t i = 0; i <= kMaxLocalEdgeIndex; ++i) {
    if (i)
      headings += ",";
    headings += std::to_string(n->heading(i));
  }
  r.add("headings", headings);
  r.add("can_contract", n->can_contract());
  r.add("is_transit", n->is_transit());
  return r;
}

Row transition_row(uint32_t idx, const NodeTransition* t) {
  Row r;
  r.add("idx", idx);
  r.add("end_node_value", t->endnode().value);
  r.add("up", t->up());
  return r;
}

// Prints the header once per kind, then the sampled rows.
struct Printer {
  const char* kind;
  bool all;
  bool header_printed = false;
  void print(const GraphId& tile, const std::vector<Row>& rows) {
    if (rows.empty())
      return;
    if (!header_printed) {
      printf("# %s level tile", kind);
      for (const auto& name : rows[0].names)
        printf(" %s", name.c_str());
      printf("\n");
      header_printed = true;
    }
    size_t stride = std::max<size_t>(1, rows.size() / 100);
    std::set<std::pair<size_t, std::string>> seen;
    for (size_t i = 0; i < rows.size(); ++i) {
      const auto& row = rows[i];
      bool keep = all || i % stride == 0;
      for (size_t c = 0; c < row.values.size(); ++c) {
        if (kNoCoverage.count(row.names[c]))
          continue;
        keep |= seen.insert({c, row.values[c]}).second;
      }
      if (!keep)
        continue;
      printf("%s %u %u", kind, tile.level(), tile.tileid());
      for (const auto& v : row.values)
        printf(" %s", v.c_str());
      printf("\n");
    }
  }
};

} // namespace

int main(int argc, char** argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: records <tile_dir> [all]\n");
    return 2;
  }
  bool all = argc > 2 && strcmp(argv[2], "all") == 0;
  boost::property_tree::ptree pt;
  pt.put("tile_dir", argv[1]);
  GraphReader reader(pt);

  const std::vector<GraphId> tiles = {
      GraphId(3015, 0, 0),   GraphId(47701, 1, 0),  GraphId(762485, 2, 0), GraphId(762486, 2, 0),
      GraphId(763925, 2, 0), GraphId(763926, 2, 0), GraphId(763927, 2, 0),
  };
  Printer edges{"edge", all}, nodes{"node", all}, transitions{"transition", all};
  for (const auto& id : tiles) {
    auto tile = reader.GetGraphTile(id);
    if (!tile) {
      // The traffic fixture directory only holds the level 0 tile.
      fprintf(stderr, "skipping missing tile %u/%u\n", id.level(), id.tileid());
      continue;
    }
    const auto* header = tile->header();
    std::vector<Row> rows;
    for (uint32_t i = 0; i < header->directededgecount(); ++i)
      rows.push_back(edge_row(i, tile->directededge(i)));
    edges.print(id, rows);
    rows.clear();
    for (uint32_t i = 0; i < header->nodecount(); ++i)
      rows.push_back(node_row(i, tile->node(i), header->base_ll()));
    nodes.print(id, rows);
    rows.clear();
    for (uint32_t i = 0; i < header->transitioncount(); ++i)
      rows.push_back(transition_row(i, tile->transition(i)));
    transitions.print(id, rows);
    fprintf(stderr, "tile %u/%u: %u edges, %u nodes, %u transitions, has_elevation %d\n",
            id.level(), id.tileid(), header->directededgecount(), header->nodecount(),
            header->transitioncount(), header->has_elevation());
  }
  return 0;
}
