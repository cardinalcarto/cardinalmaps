// Prints mainline 3.9.1 costing (Sif) results over a tile set, as the reference for
// `valinor-sif`'s oracle tests.
//
// Usage:
//   sif costing <tile_dir> <options_file> <node_stride> <edge_stride> <special_stride> [focus]
//   sif conditional <count>
//   sif speeds <tile_dir> <tile_level> <tile_id>
//   sif edges <tile_dir>
//   sif timeinfo <date_time> <tz_index> [<seconds_offset> <next_tz_index>]...
//
// `costing` builds each costing in <options_file> (one per line: a name, a costing name, and
// a JSON object of that costing's options, separated by tabs) with mainline's own option
// parsing and `CostFactory`, then dumps, for every tile in sorted order:
//
//   E  edge cost (cost, seconds, and flow sources) for every sampled edge (every edge index
//      divisible by <edge_stride>, plus every edge costing treats specially whose index is
//      divisible by <special_stride>), without a time
//      and, for edges with flow speeds, at several times of week,
//   F  Loki's edge filter (`Allowed(edge, tile, mask)`) for the same edges and several masks,
//   N  the node filter (`Allowed(node)`) for every sampled node,
//   A  `Allowed` and AR `AllowedReverse` for every pair of edges at every sampled node
//      (every node index divisible by <node_stride>, plus every node with something
//      costing treats specially whose index is divisible by <special_stride>),
//      at several times,
//   T  `TransitionCost` and TR `TransitionCostReverse` for the same pairs,
//   U  `TurnType` forward and reverse for the same pairs,
//   R  `Restricted` for every complex restriction, along a path that matches it and
//      one that doesn't, forward and reverse, at several times,
//   X  the costing's A* factor, multipass flag, travel type, and hierarchy limits.
//
// A costing named with `+bidir` has destination-only edges turned off
// (`set_allow_destination_only(false)`, as bidirectional searches do),
// and one named with `+transit` allows transit connections (as multimodal routes do).
//
// The optional focus narrows what counts as special, for large tile sets:
// with `restrictions`, only edges with timed or destination access restrictions,
// or timed complex restrictions, count, and only the access checks of those edges
// (A, AR, and R) are printed; with `elevators`, only elevator nodes and edges count.
//
// Floats are printed as their bits (hex), so the comparison is exact.
// Labels are built with mainline's `EdgeLabel` constructor from the edge, with a state
// derived from the edge's ID (see `label_state`), which the Rust test derives the same way.
//
// `conditional` prints `DateTime::is_conditional_active` for <count> generated time domains,
// each at several times in several time zones.
//
// `speeds` prints `GraphTile::GetSpeed` with `is_truck` set, for every edge of a tile.
//
// `timeinfo` prints `TimeInfo::make` for a local date and time in a time zone,
// then `forward` and `reverse` by each offset into each next time zone.
//
// `edges` lists every edge of a tile set with its speed and use
// (the input for `tools/oracle/predicted-profiles.py`).
#include <valhalla/baldr/datetime.h>
#include <valhalla/baldr/graphconstants.h>
#include <valhalla/baldr/graphreader.h>
#include <valhalla/baldr/rapidjson_utils.h>
#include <valhalla/baldr/timedomain.h>
#include <valhalla/baldr/time_info.h>
#include <valhalla/proto/options.pb.h>
#include <valhalla/sif/costfactory.h>
#include <valhalla/sif/dynamiccost.h>
#include <valhalla/sif/edgelabel.h>

#include <boost/property_tree/ptree.hpp>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <set>
#include <sstream>
#include <string>
#include <vector>

using namespace valhalla;
using namespace valhalla::baldr;
using namespace valhalla::sif;

namespace {

uint32_t bits(float f) {
  uint32_t b;
  std::memcpy(&b, &f, sizeof b);
  return b;
}

std::string gid(const GraphId& id) {
  return std::to_string(id.level()) + "/" + std::to_string(id.tileid()) + "/" +
         std::to_string(id.id());
}

// The times (seconds since the epoch, 0 for none) access checks run at.
// A Tuesday morning, a Saturday night, and a Sunday afternoon in summer (all UTC).
const std::vector<uint64_t> kTimes = {0, 1793352600, 1793833200, 1783861200};

// The local times of week (seconds from Sunday 00:00) edge costs run at, besides none.
const std::vector<uint32_t> kSecondsOfWeek = {0, 86400 + 8 * 3600 + 1800,
                                              3 * 86400 + 19 * 3600 + 600,
                                              6 * 86400 + 23 * 3600 + 3540};

// The masks Loki's edge filter runs with.
const std::vector<uint16_t> kMasks = {kDisallowNone, kDisallowShortcut,
                                      kDisallowStartRestriction | kDisallowEndRestriction |
                                          kDisallowSimpleRestriction | kDisallowClosure |
                                          kDisallowShortcut};

struct LabelState {
  uint32_t path_distance;
  bool has_measured_speed;
  InternalTurn internal_turn;
  uint8_t destonly_mask;
};

// A label state derived from the edge ID, so that the Rust side can derive the same one.
LabelState label_state(const GraphId& id) {
  uint64_t n = id.id();
  return {static_cast<uint32_t>((n * 37) % 7919), n % 3 == 0, static_cast<InternalTurn>((n / 3) % 3),
          static_cast<uint8_t>(n % 2 == 0 ? 0 : 0x1e)};
}

EdgeLabel make_label(const GraphId& id, const DirectedEdge* edge, sif::TravelMode mode,
                     uint32_t predecessor = kInvalidLabel) {
  auto s = label_state(id);
  return EdgeLabel(predecessor, id, edge, Cost{}, 0.f, mode, s.path_distance, kInvalidRestriction,
                   false, s.has_measured_speed, s.internal_turn, 0, false, false, s.destonly_mask);
}

// Whether only restricted edges count as special (the `restrictions` focus).
bool g_restrictions_only = false;
// Whether only elevators count as special (the `elevators` focus).
bool g_elevators_only = false;

// Whether a node gets every pair checked regardless of the stride:
// gates, bollards, toll booths, borders, elevators, and so on.
bool special_node(const NodeInfo* node) {
  if (g_restrictions_only)
    return false;
  if (g_elevators_only)
    return node->type() == NodeType::kElevator;
  return node->type() != NodeType::kStreetIntersection || node->private_access() ||
         node->cash_only_toll();
}

// Whether an edge (and its node's pairs) is checked regardless of the strides:
// restrictions, destination-only edges, and rare uses.
bool special_edge(const graph_tile_ptr& tile, const GraphId& id, const DirectedEdge* e) {
  if (g_restrictions_only) {
    if (e->access_restriction()) {
      for (const auto& r : tile->GetAccessRestrictions(id.id(), kAllAccess)) {
        if (r.type() == AccessType::kTimedAllowed || r.type() == AccessType::kTimedDenied ||
            r.type() == AccessType::kDestinationAllowed || r.except_destination())
          return true;
      }
    }
    for (bool forward : {true, false}) {
      if (!(forward ? e->end_restriction() : e->start_restriction()))
        continue;
      for (const auto& cr : tile->GetComplexRestrictions(forward, id, kAllAccess)) {
        if (cr.has_dt())
          return true;
      }
    }
    return false;
  }
  if (g_elevators_only)
    return e->use() == Use::kElevator;
  return e->restrictions() || e->access_restriction() || e->start_restriction() ||
         e->end_restriction() || e->destonly() || e->use() == Use::kFerry ||
         e->use() == Use::kRailFerry || e->use() == Use::kSteps || e->use() == Use::kElevator ||
         e->sac_scale() > SacScale::kMountainHiking || e->is_hov_only() ||
         e->use() == Use::kConstruction || e->bss_connection() || e->surface() == Surface::kImpassable;
}

int costing_mode(const char* tile_dir,
                 const char* options_file,
                 uint32_t stride,
                 uint32_t edge_stride,
                 uint32_t special_stride) {
  boost::property_tree::ptree pt;
  pt.put("tile_dir", tile_dir);
  GraphReader reader(pt);
  auto tileset = reader.GetTileSet();
  std::vector<GraphId> tiles(tileset.begin(), tileset.end());
  std::sort(tiles.begin(), tiles.end());

  std::ifstream in(options_file);
  std::string line;
  CostFactory factory;
  while (std::getline(in, line)) {
    if (line.empty() || line[0] == '#')
      continue;
    std::istringstream ls(line);
    std::string name, costing_name, json;
    std::getline(ls, name, '\t');
    std::getline(ls, costing_name, '\t');
    std::getline(ls, json);
    rapidjson::Document doc;
    std::string wrapped = "{\"costing_options\":{\"" + costing_name + "\":" + json + "}}";
    doc.Parse(wrapped.c_str());
    if (doc.HasParseError()) {
      fprintf(stderr, "bad json for %s\n", name.c_str());
      return 1;
    }
    Costing::Type type;
    if (!Costing_Enum_Parse(costing_name, &type)) {
      fprintf(stderr, "bad costing %s\n", costing_name.c_str());
      return 1;
    }
    Costing costing;
    google::protobuf::RepeatedPtrField<CodedDescription> warnings;
    ParseCosting(doc, "/costing_options/" + costing_name, &costing, warnings, type);
    auto cost = factory.Create(costing);
    // Variants of a costing that Thor configures after building it.
    if (name.find("+bidir") != std::string::npos)
      cost->set_allow_destination_only(false);
    if (name.find("+transit") != std::string::npos)
      cost->SetAllowTransitConnections(true);
    const sif::TravelMode mode = cost->travel_mode();
    const uint32_t access_mask = cost->access_mode();
    printf("O\t%s\t%s\t%s\n", name.c_str(), costing_name.c_str(), json.c_str());

    {
      auto& hl = cost->GetHierarchyLimits();
      printf("X\t%08x\t%d\t%u\t%d\t%d", bits(cost->AStarCostFactor()), cost->AllowMultiPass(),
             cost->travel_type(), cost->DefaultHierarchyLimits(), cost->UseHierarchyLimits());
      for (auto& h : hl)
        printf("\t%u,%08x", h.max_up_transitions(), bits(h.expand_within_dist()));
      printf("\n");
      cost->set_pass(1);
      printf("X1\t%08x\n", bits(cost->AStarCostFactor()));
      cost->set_pass(0);
    }

    auto reader_getter = [&reader]() { return LimitedGraphReader(reader); };

    for (const auto& tile_id : tiles) {
      graph_tile_ptr tile = reader.GetGraphTile(tile_id);
      if (!tile)
        continue;
      const uint32_t edge_count = tile->header()->directededgecount();
      for (uint32_t i = 0; i < edge_count; ++i) {
        GraphId id(tile_id.tileid(), tile_id.level(), i);
        const DirectedEdge* e = tile->directededge(i);
        if (g_restrictions_only ||
            (i % edge_stride != 0 && !(special_edge(tile, id, e) && i % special_stride == 0)))
          continue;
        uint8_t flow = 0;
        Cost c = cost->EdgeCost(e, id, tile, TimeInfo::invalid(), flow);
        printf("E\t%s\t-\t%08x\t%08x\t%u\n", gid(id).c_str(), bits(c.cost), bits(c.secs), flow);
        if (e->has_predicted_speed() || e->constrained_flow_speed() || e->free_flow_speed()) {
          for (auto sow : kSecondsOfWeek) {
            TimeInfo ti{true, 0, 0, sow, 0, false, nullptr};
            flow = 0;
            c = cost->EdgeCost(e, id, tile, ti, flow);
            printf("E\t%s\t%u\t%08x\t%08x\t%u\n", gid(id).c_str(), sow, bits(c.cost),
                   bits(c.secs), flow);
          }
        }
        for (auto mask : kMasks) {
          printf("F\t%s\t%u\t%d\n", gid(id).c_str(), mask, cost->Allowed(e, tile, mask));
        }
      }

      const uint32_t node_count = tile->header()->nodecount();
      for (uint32_t n = 0; n < node_count; ++n) {
        const NodeInfo* node = tile->node(n);
        GraphId node_id(tile_id.tileid(), tile_id.level(), n);
        // Outgoing edges and the opposing (incoming) edge of each.
        std::vector<GraphId> out_ids, in_ids;
        std::vector<const DirectedEdge*> outs, ins;
        std::vector<graph_tile_ptr> in_tiles;
        bool special = special_node(node);
        for (uint32_t k = 0; k < node->edge_count(); ++k) {
          GraphId out_id(tile_id.tileid(), tile_id.level(), node->edge_index() + k);
          const DirectedEdge* out = tile->directededge(node->edge_index() + k);
          graph_tile_ptr in_tile = tile;
          const DirectedEdge* in_edge = nullptr;
          GraphId in_id = reader.GetOpposingEdgeId(out_id, in_edge, in_tile);
          if (!in_id.is_valid() || !in_edge)
            continue;
          special = special || special_edge(tile, out_id, out) || special_edge(in_tile, in_id, in_edge);
          out_ids.push_back(out_id);
          outs.push_back(out);
          in_ids.push_back(in_id);
          ins.push_back(in_edge);
          in_tiles.push_back(in_tile);
        }
        if (n % stride != 0 && !(special && n % special_stride == 0))
          continue;
        printf("N\t%s\t%d\n", gid(node_id).c_str(), cost->Allowed(node));
        const uint32_t tz = node->timezone();
        for (size_t j = 0; j < ins.size(); ++j) {
          EdgeLabel pred = make_label(in_ids[j], ins[j], mode);
          for (size_t i = 0; i < outs.size(); ++i) {
            const bool check_forward =
                !g_restrictions_only || special_edge(tile, out_ids[i], outs[i]);
            const bool check_reverse =
                !g_restrictions_only || special_edge(in_tiles[i], in_ids[i], ins[i]);
            for (auto t : kTimes) {
              if (!check_forward)
                break;
              uint8_t ridx = kInvalidRestriction;
              uint8_t mask = pred.destonly_access_restr_mask();
              bool destonly = false;
              bool ok = cost->Allowed(outs[i], pred, tile, out_ids[i], t, tz, ridx, mask, &destonly);
              printf("A\t%s\t%s\t%lu\t%u\t%d\t%u\t%u\t%d\n", gid(in_ids[j]).c_str(),
                     gid(out_ids[i]).c_str(), t, tz, ok, ridx, mask, destonly);
            }
            for (auto t : kTimes) {
              if (!check_reverse)
                break;
              uint8_t ridx = kInvalidRestriction;
              uint8_t mask = pred.destonly_access_restr_mask();
              bool destonly = false;
              bool ok = cost->AllowedReverse(outs[i], pred, ins[i], in_tiles[i], in_ids[i], t, tz, ridx,
                                        mask, &destonly);
              printf("AR\t%s\t%s\t%s\t%lu\t%u\t%d\t%u\t%u\t%d\n", gid(in_ids[j]).c_str(),
                     gid(out_ids[i]).c_str(), gid(in_ids[i]).c_str(), t, tz, ok, ridx, mask,
                     destonly);
            }
            if (g_restrictions_only)
              continue;
            for (int destonly = 0; destonly <= 1; ++destonly) {
              Cost c = cost->TransitionCost(outs[i], node, pred, tile, reader_getter, destonly);
              printf("T\t%s\t%s\t%s\t%d\t%08x\t%08x\n", gid(in_ids[j]).c_str(),
                     gid(out_ids[i]).c_str(), gid(node_id).c_str(), destonly, bits(c.cost),
                     bits(c.secs));
            }
            const auto s = label_state(out_ids[i]);
            for (int destonly = 0; destonly <= 1; ++destonly) {
              Cost c = cost->TransitionCostReverse(outs[i]->localedgeidx(), node, ins[i], outs[j],
                                                   pred, tile, reader_getter, s.has_measured_speed,
                                                   s.internal_turn, destonly);
              printf("TR\t%s\t%s\t%s\t%s\t%s\t%d\t%d\t%d\t%08x\t%08x\n", gid(in_ids[j]).c_str(),
                     gid(out_ids[i]).c_str(), gid(in_ids[i]).c_str(), gid(out_ids[j]).c_str(),
                     gid(node_id).c_str(), s.has_measured_speed,
                     static_cast<int>(s.internal_turn), destonly, bits(c.cost), bits(c.secs));
            }
            printf("U\t%s\t%s\t%s\t%d\t%d\n", gid(in_ids[j]).c_str(), gid(out_ids[i]).c_str(),
                   gid(node_id).c_str(),
                   static_cast<int>(cost->TurnType(pred.opp_local_idx(), node, outs[i])),
                   static_cast<int>(
                       cost->TurnType(outs[i]->localedgeidx(), node, ins[i], outs[j])));
          }
        }
      }

      // Complex restrictions, along matching and diverging paths.
      for (uint32_t i = 0; i < edge_count; ++i) {
        GraphId id(tile_id.tileid(), tile_id.level(), i);
        const DirectedEdge* e = tile->directededge(i);
        for (bool forward : {true, false}) {
          if (!((forward ? e->end_restriction() : e->start_restriction()) & access_mask))
            continue;
          // The node the edge leaves, for its time zone.
          uint32_t tz = 0;
          {
            graph_tile_ptr opp_tile = tile;
            const DirectedEdge* opp = nullptr;
            reader.GetOpposingEdgeId(id, opp, opp_tile);
            if (opp) {
              auto end = opp->endnode();
              graph_tile_ptr nt = reader.GetGraphTile(end);
              if (nt)
                tz = nt->node(end)->timezone();
            }
          }
          for (const auto& cr : tile->GetComplexRestrictions(forward, id, access_mask)) {
            std::vector<GraphId> vias;
            cr.WalkVias([&vias](const GraphId* via) {
              vias.push_back(*via);
              return WalkingVia::KeepWalking;
            });
            GraphId end = forward ? cr.from_graphid() : cr.to_graphid();
            // Matching: [end, vias reversed], predecessor vias[0] (or end without vias).
            // Diverging: the same without the end.
            for (bool matching : {true, false}) {
              std::vector<GraphId> chain;
              if (matching)
                chain.push_back(end);
              for (auto it = vias.rbegin(); it != vias.rend(); ++it)
                chain.push_back(*it);
              if (chain.empty())
                continue;
              std::vector<EdgeLabel> labels;
              bool missing = false;
              for (size_t k = 0; k < chain.size(); ++k) {
                graph_tile_ptr t = tile;
                const DirectedEdge* ce = reader.directededge(chain[k], t);
                if (!ce) {
                  missing = true;
                  break;
                }
                labels.push_back(
                    make_label(chain[k], ce, mode, k == 0 ? kInvalidLabel : static_cast<uint32_t>(k - 1)));
              }
              if (missing)
                continue;
              std::string ids;
              for (auto& c : chain)
                ids += (ids.empty() ? "" : ",") + gid(c);
              for (auto t : kTimes) {
                bool r = cost->Restricted(e, labels.back(), labels, tile, id, forward, nullptr, t, tz);
                printf("R\t%s\t%d\t%lu\t%u\t%s\t%d\n", gid(id).c_str(), forward, t, tz, ids.c_str(),
                       r);
              }
            }
          }
        }
      }
    }
  }
  return 0;
}

// A small deterministic generator (the Rust test doesn't need to reproduce it,
// since every input is printed).
struct Lcg {
  uint64_t state;
  uint32_t next() {
    state = state * 6364136223846793005ULL + 1442695040888963407ULL;
    return static_cast<uint32_t>(state >> 33);
  }
  uint32_t below(uint32_t n) {
    return next() % n;
  }
};

int conditional_mode(uint32_t count) {
  Lcg rng{12345};
  // Andorra, Los Angeles, Sydney (southern hemisphere DST), Kolkata (half hour offset),
  // Chatham (45 minute offset), and an unassigned index.
  const std::vector<uint32_t> zones = {293, 94, 257, 198, 354, 0, 600};
  for (uint32_t k = 0; k < count; ++k) {
    TimeDomain td(0);
    // Mostly plausible fields, with some out-of-range ones.
    bool nth = rng.below(2);
    uint64_t v = 0;
    auto set = [&v](uint64_t value, int shift, int width) {
      v |= (value & ((1ULL << width) - 1)) << shift;
    };
    set(nth, 0, 1);
    set(rng.below(4) == 0 ? 0 : rng.below(128), 1, 7);
    set(rng.below(3) == 0 ? 0 : rng.below(24), 8, 5);
    set(rng.below(3) == 0 ? 0 : rng.below(60), 13, 6);
    set(rng.below(3) == 0 ? 0 : 1 + rng.below(12), 19, 4);
    set(rng.below(3) == 0 ? 0 : (nth ? 1 + rng.below(7) : 1 + rng.below(31)), 23, 5);
    set(nth ? (rng.below(3) == 0 ? 0 : 1 + rng.below(5)) : 0, 28, 3);
    set(rng.below(3) == 0 ? 0 : rng.below(24), 31, 5);
    set(rng.below(3) == 0 ? 0 : rng.below(60), 36, 6);
    set(rng.below(3) == 0 ? 0 : 1 + rng.below(12), 42, 4);
    set(rng.below(3) == 0 ? 0 : (nth ? 1 + rng.below(7) : 1 + rng.below(31)), 46, 5);
    set(nth ? (rng.below(3) == 0 ? 0 : 1 + rng.below(5)) : 0, 51, 3);
    td = TimeDomain(v);
    for (int i = 0; i < 6; ++i) {
      // Times from 2024 through 2027.
      uint64_t t = 1704067200ULL + static_cast<uint64_t>(rng.next()) % (4ULL * 366 * 86400);
      uint32_t tz = zones[rng.below(zones.size())];
      bool active = DynamicCost::IsConditionalActive(v, t, tz);
      printf("C\t%lu\t%lu\t%u\t%d\n", v, t, tz, active);
    }
  }
  return 0;
}

int speeds_mode(const char* tile_dir, uint32_t level, uint32_t tile_index) {
  boost::property_tree::ptree pt;
  pt.put("tile_dir", tile_dir);
  GraphReader reader(pt);
  GraphId base(tile_index, level, 0);
  auto tile = reader.GetGraphTile(base);
  if (!tile)
    return 1;
  for (uint32_t i = 0; i < tile->header()->directededgecount(); ++i) {
    const DirectedEdge* e = tile->directededge(i);
    for (uint8_t mask : {kDefaultFlowMask, kFreeFlowMask, kConstrainedFlowMask, kNoFlowMask}) {
      uint8_t sources = 0;
      uint32_t s = tile->GetSpeed(e, mask, kInvalidSecondsOfWeek, true, &sources);
      printf("S\t%u\t%u\t-\t%u\t%u\n", i, mask, s, sources);
      for (uint32_t sow : kSecondsOfWeek) {
        sources = 0;
        s = tile->GetSpeed(e, mask, sow, true, &sources);
        printf("S\t%u\t%u\t%u\t%u\t%u\n", i, mask, sow, s, sources);
      }
    }
  }
  return 0;
}

void print_time_info(const char* what, const TimeInfo& ti) {
  printf("%s\t%d\t%lu\t%lu\t%lu\t%lu\t%d\n", what, static_cast<int>(ti.valid),
         static_cast<unsigned long>(ti.timezone_index), static_cast<unsigned long>(ti.local_time),
         static_cast<unsigned long>(ti.second_of_week),
         static_cast<unsigned long>(ti.seconds_from_now),
         static_cast<int>(ti.negative_seconds_from_now));
}

int timeinfo_mode(int argc, char** argv) {
  std::string date_time = argv[2];
  TimeInfo ti = TimeInfo::make(date_time, std::stoi(argv[3]));
  print_time_info("make", ti);
  for (int i = 4; i + 1 < argc; i += 2) {
    float offset = std::stof(argv[i]);
    int tz = std::stoi(argv[i + 1]);
    print_time_info("forward", ti.forward(offset, tz));
    print_time_info("reverse", ti.reverse(offset, tz));
  }
  return 0;
}

int edges_mode(const char* tile_dir) {
  boost::property_tree::ptree pt;
  pt.put("tile_dir", tile_dir);
  GraphReader reader(pt);
  auto tileset = reader.GetTileSet();
  std::vector<GraphId> tiles(tileset.begin(), tileset.end());
  std::sort(tiles.begin(), tiles.end());
  for (const auto& tile_id : tiles) {
    auto tile = reader.GetGraphTile(tile_id);
    if (!tile)
      continue;
    for (uint32_t i = 0; i < tile->header()->directededgecount(); ++i) {
      const DirectedEdge* e = tile->directededge(i);
      GraphId id(tile_id.tileid(), tile_id.level(), i);
      printf("%s\t%u\t%u\t%d\n", gid(id).c_str(), e->speed(), static_cast<unsigned>(e->use()),
             e->is_shortcut());
    }
  }
  return 0;
}

} // namespace

int main(int argc, char** argv) {
  if (argc == 8 && strcmp(argv[1], "costing") == 0) {
    g_restrictions_only = strcmp(argv[7], "restrictions") == 0;
    g_elevators_only = strcmp(argv[7], "elevators") == 0;
    if (!g_restrictions_only && !g_elevators_only) {
      fprintf(stderr, "unknown focus %s\n", argv[7]);
      return 2;
    }
  }
  if ((argc == 7 || argc == 8) && strcmp(argv[1], "costing") == 0)
    return costing_mode(argv[2], argv[3], std::stoul(argv[4]), std::stoul(argv[5]),
                        std::stoul(argv[6]));
  if (argc == 3 && strcmp(argv[1], "conditional") == 0)
    return conditional_mode(std::stoul(argv[2]));
  if (argc >= 4 && strcmp(argv[1], "timeinfo") == 0)
    return timeinfo_mode(argc, argv);
  if (argc == 3 && strcmp(argv[1], "edges") == 0)
    return edges_mode(argv[2]);
  if (argc == 5 && strcmp(argv[1], "speeds") == 0)
    return speeds_mode(argv[2], std::stoul(argv[3]), std::stoul(argv[4]));
  fprintf(stderr, "usage: sif costing <tile_dir> <options_file> <node_stride> <edge_stride> "
                  "<special_stride>\n"
                  "       sif conditional <count>\n"
                  "       sif speeds <tile_dir> <level> <tile_id>\n"
                  "       sif edges <tile_dir>\n"
                  "       sif timeinfo <date_time> <tz_index> [<offset> <next_tz_index>]...\n");
  return 2;
}
