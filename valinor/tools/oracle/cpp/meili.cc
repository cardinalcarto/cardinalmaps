// Runs mainline 3.9.1's map matcher (Meili), as the reference for `valinor-meili`.
//
// Usage:
//   meili <valhalla.json> <tile_dir> match <requests.jsonl> [columns]
//   meili <valhalla.json> <tile_dir> walks <count> <seed>
//
// The config's `mjolnir` section is pointed at <tile_dir> (with no tile extract or traffic).
//
// `match` reads trace requests, one JSON object per line: {"name", "action", "body"}, where
// `action` is `trace_route` or `trace_attributes` and `body` the request body (lines whose body
// is null are skipped). For each, it does what Thor does before it forms paths:
// `ParseApi` (the front), `MapMatcherFactory::Create` and the measurements of
// `thor_worker_t::parse_measurements`, and `MapMatcher::OfflineMatch` with k = alternates + 1
// for `trace_attributes` and 1 for `trace_route`, with one factory for the whole run
// (`ClearFullCache` after each request, as `cleanup()` does). It prints, per request:
//
//   case <name>
//   error <code> <message>          a failure (code `exception` for other C++ exceptions)
//   match <score>                   each MatchResults, best first, then its results and segments:
//   result <lng> <lat> <distance_from> <edge>|- <distance_along> <epoch_time> <time>/<id>|-
//          <is_break_point> <begins_discontinuity> <ends_discontinuity> <candidate_edges>|-
//   segment <edge> <source> <target> <first_match_idx> <last_match_idx> <restriction_idx>
//           <discontinuity>
//   end
//
// With `columns`, each case's matches are preceded by a line per column of the search,
// `column <edge>[,<edge>] ...`, the edges of each candidate in the order mainline found them
// (its hash order, which also picks the edge that stands for a candidate at a node),
// for diagnosing differences that come from that order.
//
// (each record is one line). `candidate_edges` is the number of correlated edges of the
// result's state (what OSRM's `alternatives_count` reads). Doubles are printed with 17
// significant digits and the float score with 9, so they round-trip exactly.
//
// `walks` prints <count> random walks along the graph, one JSON object per line:
// {"mode": "auto"|"pedestrian"|"bicycle", "shape": [[lng, lat], ...]}, following edges the mode
// may use forward, without U-turns, for up to about 3 km. `meili-corpus.py` turns them into
// noisy traces.
#include <algorithm>
#include <cstdio>
#include <fstream>
#include <functional>
#include <iostream>
#include <memory>
#include <random>
#include <sstream>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

// The `columns` diagnostics read the matcher's top-k layers, which are private.
#define private public
#include <valhalla/meili/map_matcher.h>
#include <valhalla/meili/topk_search.h>
#undef private

#include <valhalla/baldr/graphreader.h>
#include <valhalla/baldr/rapidjson_utils.h>
#include <valhalla/exceptions.h>
#include <valhalla/meili/map_matcher.h>
#include <valhalla/meili/map_matcher_factory.h>
#include <valhalla/meili/measurement.h>
#include <valhalla/midgard/logging.h>
#include <valhalla/proto/api.pb.h>
#include <valhalla/worker.h>

#include <boost/property_tree/ptree.hpp>

#include <cstdio>
#include <fstream>
#include <iostream>
#include <random>
#include <sstream>
#include <string>
#include <vector>

using namespace valhalla;

namespace {

std::string number(double value) {
  char buffer[64];
  std::snprintf(buffer, sizeof(buffer), "%.17g", value);
  return buffer;
}

std::string number32(float value) {
  char buffer[64];
  std::snprintf(buffer, sizeof(buffer), "%.9g", value);
  return buffer;
}

std::string summarize(const meili::MapMatcher& matcher,
                      const std::vector<meili::MatchResults>& all,
                      bool columns) {
  std::ostringstream out;
  // With `columns`, each column's candidates (by their edges), in libstdc++'s hash order;
  // the Rust test can replay them to check that a difference comes from that order alone.
  const auto& container = matcher.state_container();
  for (uint32_t t = 0; columns && t < container.size(); ++t) {
    out << "column";
    for (const auto& state : container.column(t)) {
      out << ' ';
      const auto& edges = state.candidate().correlation().edges();
      for (int e = 0; e < edges.size(); ++e) {
        out << (e ? "," : "") << edges.Get(e).graph_id();
      }
    }
    out << '\n';
  }
  // And the paths the top-k search cloned (one layer per path searched, duplicates
  // included), and the states it removed as redundant.
  for (size_t i = 0; columns && i < matcher.ts_.evss_.size(); ++i) {
    std::vector<std::pair<uint32_t, uint32_t>> path;
    for (const auto& kv : matcher.ts_.evss_[i]->clone_) {
      const auto origin = matcher.ts_.GetOrigin(kv.first, kv.first);
      path.emplace_back(origin.time(), origin.id());
    }
    std::sort(path.begin(), path.end());
    out << "layer";
    for (const auto& p : path) {
      out << ' ' << p.first << '/' << p.second;
    }
    out << '\n';
  }
  // MEILI_DEBUG_STATE=<time>/<id>: the routes from that state to the next column.
  if (const char* debug = std::getenv("MEILI_DEBUG_STATE")) {
    unsigned t = 0, i = 0;
    std::sscanf(debug, "%u/%u", &t, &i);
    const meili::StateId from(t, i);
    for (uint32_t j = 0; t + 1 < container.size() && j < container.column(t + 1).size(); ++j) {
      std::vector<meili::EdgeSegment> route;
      meili::MatchResult dummy{};
      dummy.edgeid = baldr::GraphId(0, 0, 0);
      dummy.distance_along = 0;
      bool found = false;
      try {
        found = meili::MergeRoute(container.state(from), container.state(meili::StateId(t + 1, j)),
                                  route, dummy);
      } catch (...) {}
      out << "debug route " << t << '/' << i << " -> " << t + 1 << '/' << j << ' ' << found;
      for (const auto& s : route) {
        out << ' ' << s.edgeid.value << ':' << number(s.source) << '-' << number(s.target);
      }
      out << '\n';
    }
  }
  if (columns) {
    std::vector<std::pair<uint32_t, uint32_t>> removed;
    for (const auto& s : matcher.ts_.removed_origins_) {
      removed.emplace_back(s.time(), s.id());
    }
    std::sort(removed.begin(), removed.end());
    out << "removed";
    for (const auto& p : removed) {
      out << ' ' << p.first << '/' << p.second;
    }
    out << '\n';
  }
  for (const auto& m : all) {
    out << "match " << number32(m.score) << '\n';
    for (const auto& r : m.results) {
      out << "result " << number(r.lnglat.lng()) << ' ' << number(r.lnglat.lat()) << ' '
          << number(r.distance_from) << ' ';
      if (r.edgeid.is_valid()) {
        out << r.edgeid.value;
      } else {
        out << '-';
      }
      out << ' ' << number(r.distance_along) << ' ' << number(r.epoch_time) << ' ';
      if (r.stateid.IsValid()) {
        out << r.stateid.time() << '/' << r.stateid.id();
      } else {
        out << '-';
      }
      out << ' ' << r.is_break_point << ' ' << r.begins_discontinuity << ' '
          << r.ends_discontinuity << ' ';
      if (r.stateid.IsValid()) {
        out << matcher.state_container().state(r.stateid).candidate().correlation().edges_size();
      } else {
        out << '-';
      }
      out << '\n';
    }
    for (const auto& s : m.segments) {
      out << "segment " << s.edgeid.value << ' ' << number(s.source) << ' ' << number(s.target)
          << ' ' << s.first_match_idx << ' ' << s.last_match_idx << ' '
          << static_cast<int>(s.restriction_idx) << ' ' << s.discontinuity << '\n';
    }
  }
  return out.str();
}

int match(const boost::property_tree::ptree& config, const char* path, bool columns) {
  meili::MapMatcherFactory factory(config);
  std::ifstream in(path);
  std::string line;
  while (std::getline(in, line)) {
    if (line.empty()) {
      continue;
    }
    rapidjson::Document doc;
    doc.Parse(line.c_str());
    if (doc.HasParseError() || !doc.IsObject() || !doc.HasMember("body") || doc["body"].IsNull()) {
      continue;
    }
    const std::string name = doc["name"].GetString();
    const std::string action_name = doc["action"].GetString();
    rapidjson::StringBuffer buffer;
    rapidjson::Writer<rapidjson::StringBuffer> writer(buffer);
    doc["body"].Accept(writer);
    const std::string body = buffer.GetString();

    std::cout << "case " << name << '\n';
    try {
      Options::Action action;
      if (!Options_Action_Enum_Parse(action_name, &action)) {
        throw std::runtime_error("unknown action " + action_name);
      }
      Api request;
      ParseApi(body, action, request);
      const auto& options = request.options();
      std::unique_ptr<meili::MapMatcher> matcher;
      try {
        matcher.reset(factory.Create(options));
      } catch (const std::invalid_argument& ex) { throw std::runtime_error(std::string(ex.what())); }
      std::vector<meili::Measurement> trace;
      try {
        const auto& mconfig = matcher->config();
        for (const auto& pt : options.shape()) {
          trace.emplace_back(meili::Measurement{{pt.ll().lng(), pt.ll().lat()},
                                                pt.has_accuracy_case()
                                                    ? pt.accuracy()
                                                    : mconfig.emission_cost.gps_accuracy_meters,
                                                pt.has_radius_case()
                                                    ? pt.radius()
                                                    : mconfig.candidate_search.search_radius_meters,
                                                pt.time(), pt.type()});
        }
      } catch (...) { throw valhalla_exception_t{424}; }
      const uint32_t k =
          options.action() == Options::trace_attributes ? options.alternates() + 1 : 1;
      const auto results = matcher->OfflineMatch(trace, k);
      std::cout << summarize(*matcher, results, columns);
    } catch (const valhalla_exception_t& e) {
      std::cout << "error " << e.code << ' ' << e.message << '\n';
    } catch (const std::exception& e) {
      std::cout << "error exception " << e.what() << '\n';
    }
    std::cout << "end\n";
    factory.ClearFullCache();
  }
  return 0;
}

int walks(const boost::property_tree::ptree& config, int count, unsigned seed) {
  baldr::GraphReader reader(config.get_child("mjolnir"));
  std::vector<baldr::GraphId> tiles;
  for (const auto& level : baldr::TileHierarchy::levels()) {
    for (const auto& id : reader.GetTileSet(level.level)) {
      tiles.push_back(id);
    }
  }
  std::mt19937 rng(seed);
  const char* modes[] = {"auto", "auto", "pedestrian", "bicycle"};
  const uint32_t access[] = {baldr::kAutoAccess, baldr::kAutoAccess, baldr::kPedestrianAccess,
                             baldr::kBicycleAccess};
  int printed = 0;
  while (printed < count) {
    const int m = rng() % 4;
    auto tile = reader.GetGraphTile(tiles[rng() % tiles.size()]);
    if (!tile || tile->header()->directededgecount() == 0) {
      continue;
    }
    baldr::GraphId edge_id = tile->id();
    edge_id.set_id(rng() % tile->header()->directededgecount());
    std::vector<midgard::PointLL> shape;
    float length = 0.f;
    const float target = 300.f + (rng() % 2700);
    for (int steps = 0; steps < 200 && length < target; ++steps) {
      const auto* edge = reader.directededge(edge_id, tile);
      if (!edge || edge->is_shortcut() || edge->use() == baldr::Use::kTransitConnection ||
          !(edge->forwardaccess() & access[m])) {
        break;
      }
      auto points = tile->edgeinfo(edge).shape();
      if (!edge->forward()) {
        std::reverse(points.begin(), points.end());
      }
      for (size_t i = shape.empty() ? 0 : 1; i < points.size(); ++i) {
        shape.push_back(points[i]);
      }
      length += edge->length();
      // Continue on a random edge at the end node that isn't a U-turn.
      auto end_tile = reader.GetGraphTile(edge->endnode());
      if (!end_tile) {
        break;
      }
      const auto* node = end_tile->node(edge->endnode());
      std::vector<baldr::GraphId> next;
      for (uint32_t i = 0; i < node->edge_count(); ++i) {
        if (i == edge->opp_local_idx() && node->edge_count() > 1) {
          continue;
        }
        const auto* out = end_tile->directededge(node->edge_index() + i);
        if (out->is_shortcut() || !(out->forwardaccess() & access[m])) {
          continue;
        }
        baldr::GraphId id = edge->endnode();
        id.set_id(node->edge_index() + i);
        next.push_back(id);
      }
      if (next.empty()) {
        break;
      }
      edge_id = next[rng() % next.size()];
      tile = end_tile;
    }
    if (shape.size() < 2 || length < 200.f) {
      continue;
    }
    std::cout << "{\"mode\": \"" << modes[m] << "\", \"shape\": [";
    for (size_t i = 0; i < shape.size(); ++i) {
      std::cout << (i ? ", " : "") << '[' << number(shape[i].lng()) << ", "
                << number(shape[i].lat()) << ']';
    }
    std::cout << "]}\n";
    ++printed;
  }
  return 0;
}

} // namespace

int main(int argc, char** argv) {
  if (argc < 5) {
    std::cerr << "usage: meili <valhalla.json> <tile_dir> match <requests.jsonl>\n"
                 "       meili <valhalla.json> <tile_dir> walks <count> <seed>\n";
    return 1;
  }
  boost::property_tree::ptree config;
  rapidjson::read_json(argv[1], config);
  config.put("mjolnir.tile_dir", argv[2]);
  config.put("mjolnir.tile_extract", "");
  config.put("mjolnir.traffic_extract", "");
  config.put("mjolnir.logging.type", "");
  config.put("meili.logging.type", "");
  midgard::logging::Configure({{"type", ""}});

  const std::string mode = argv[3];
  if (mode == "match") {
    return match(config, argv[4], argc == 6 && std::string(argv[5]) == "columns");
  }
  if (mode == "walks" && argc == 6) {
    return walks(config, std::stoi(argv[4]), static_cast<unsigned>(std::stoul(argv[5])));
  }
  std::cerr << "unknown mode " << mode << '\n';
  return 1;
}
