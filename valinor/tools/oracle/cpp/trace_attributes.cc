// Runs mainline 3.9.1's map matching actions on a list of requests, as the reference for
// `valinor-tyr`'s `trace_attributes` serializer and its `trace_route` responses.
//
// Usage:
//   trace_attributes <valhalla.json> <tile_dir> <requests.tsv>
//
// The config's `mjolnir` section is pointed at <tile_dir> (with no tile extract or traffic),
// so the same requests can be run against the fixture tiles.
// Each line of <requests.tsv> is `<id>\t<action>\t<json>`, where the action is
// `trace_attributes` or `trace_route`.
// For each request, this does what the staged chain does:
// `ParseApi` and `loki_worker_t::trace` (Loki), then Thor, then `cleanup()` on each worker
// (as `prime_server` does after a job).
//
// - `trace_attributes`: mainline's Thor serializes the response itself
//   (`thor_worker_t::trace_attributes` calls `tyr::serializeTraceAttributes`),
//   so no message between services carries what the serializer gets.
//   This runs the same steps as `thor_worker_t::trace_attributes`, line for line,
//   in a subclass that can reach the worker's protected members, and dumps the serializer's
//   input: the `Api` (with Thor's `trip` and options) and the match results with their scores.
//   It then runs the worker's own `trace_attributes` on a fresh copy of Loki's output and checks
//   that both responses are the same bytes, so the dump is what mainline's serializer saw.
// - `trace_route`: Thor's `trace_route`, then what Odin's `narrate` does:
//   `DirectionsBuilder::Build` (with a `MarkupFormatter` from the same config, as the worker's)
//   and `tyr::serializeDirections`. The dumped `Api` is the one Odin handed Tyr
//   (with `trip` and `directions`), as in the captures; it is copied before serializing,
//   since the PBF serializer clears what it doesn't send.
//
// Each request prints one line:
//
//   <id>\tok\t<Api>\t<matches>\t<response>
//   <id>\terror\t<code>\t<message>
//
// where <Api> is the serialized `valhalla.Api`, <matches> is the match results as JSON
// (empty for `trace_route`), and <response> is the response body, all base64-encoded.
// The match results are a list with an entry per result
// (`{"confidence_score", "raw_score", "results": [...]}`),
// and each result holds every field of `meili::MatchResult`:
// `lnglat`, `distance_from`, `edge_id` (null when invalid), `distance_along`, `epoch_time`,
// `state_id` (`[time, id]`, null when invalid), `is_break_point`, `begins_discontinuity`,
// `ends_discontinuity`, and `edge_index` (null when unset).
// Doubles are written with 17 significant digits and floats with 9, so they round-trip exactly,
// as strings (`"0.12345678901234566"`), so that parsing them is exact.
#include <valhalla/baldr/rapidjson_utils.h>
#include <valhalla/exceptions.h>
#include <valhalla/loki/worker.h>
#include <valhalla/meili/match_result.h>
#include <valhalla/midgard/logging.h>
#include <valhalla/midgard/util.h>
#include <valhalla/odin/directionsbuilder.h>
#include <valhalla/odin/markup_formatter.h>
#include <valhalla/proto/api.pb.h>
#include <valhalla/thor/worker.h>
#include <valhalla/tyr/serializers.h>
#include <valhalla/worker.h>

#include <boost/property_tree/ptree.hpp>

#include <cmath>
#include <cstdio>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <tuple>
#include <vector>

using namespace valhalla;

namespace {

using map_match_results_t = std::vector<std::tuple<float, float, std::vector<meili::MatchResult>>>;

std::string b64(const std::string& s) {
  return midgard::encode64(s);
}

// A number as a JSON string of its round-tripping digits: JSON parsers don't all round
// decimal numbers correctly (serde_json's default doesn't), but string to float conversions do.
std::string number(double value, int digits) {
  char buffer[64];
  std::snprintf(buffer, sizeof(buffer), "\"%.*g\"", digits, value);
  return buffer;
}

std::string dump(const map_match_results_t& results) {
  std::ostringstream out;
  out << '[';
  for (size_t r = 0; r < results.size(); ++r) {
    const auto& [confidence, raw, matches] = results[r];
    out << (r ? "," : "") << "{\"confidence_score\":" << number(confidence, 9)
        << ",\"raw_score\":" << number(raw, 9) << ",\"results\":[";
    for (size_t m = 0; m < matches.size(); ++m) {
      const auto& match = matches[m];
      out << (m ? "," : "") << "{\"lnglat\":[" << number(match.lnglat.lng(), 17) << ','
          << number(match.lnglat.lat(), 17) << "],\"distance_from\":"
          << number(match.distance_from, 17) << ",\"edge_id\":";
      if (match.edgeid.is_valid()) {
        out << match.edgeid.value;
      } else {
        out << "null";
      }
      out << ",\"distance_along\":" << number(match.distance_along, 17)
          << ",\"epoch_time\":" << number(match.epoch_time, 17) << ",\"state_id\":";
      if (match.HasState()) {
        out << '[' << match.stateid.time() << ',' << match.stateid.id() << ']';
      } else {
        out << "null";
      }
      out << ",\"is_break_point\":" << (match.is_break_point ? "true" : "false")
          << ",\"begins_discontinuity\":" << (match.begins_discontinuity ? "true" : "false")
          << ",\"ends_discontinuity\":" << (match.ends_discontinuity ? "true" : "false")
          << ",\"edge_index\":";
      if (match.edge_index == meili::kInvalidEdgeIndex) {
        out << "null";
      } else {
        out << match.edge_index;
      }
      out << '}';
    }
    out << "]}";
  }
  out << ']';
  return out.str();
}

// Thor's worker, with `thor_worker_t::trace_attributes` repeated step for step,
// keeping what it hands the serializer.
struct oracle_thor_t : public thor::thor_worker_t {
  using thor::thor_worker_t::thor_worker_t;

  std::string
  trace_attributes_dumping(Api& request, Api& serializer_api, map_match_results_t& serializer_results) {
    // Everything from here to the serializer call is `src/thor/trace_attributes_action.cc`.
    auto _ = measure_scope_time(request);

    adjust_locations(request);
    parse_costing(request);
    parse_measurements(request);
    const Options options = request.options();
    controller = baldr::AttributesController(options, true);

    map_match_results_t map_match_results;

    switch (options.shape_match()) {
      case ShapeMatch::edge_walk:
        try {
          route_match(request);
          map_match_results.emplace_back(1.0f, 0.0f, std::vector<meili::MatchResult>{});
        } catch (const std::exception& e) {
          throw valhalla_exception_t{
              443, ShapeMatch_Enum_Name(options.shape_match()) +
                       " algorithm failed to find exact route match.  Try using "
                       "shape_match:'walk_or_snap' to fallback to map-matching algorithm"};
        }
        break;
      case ShapeMatch::map_snap:
        try {
          map_match_results = map_match(request);
        } catch (const std::exception& e) {
          throw valhalla_exception_t{
              444, ShapeMatch_Enum_Name(options.shape_match()) +
                       " algorithm failed to snap the shape points to the correct shape."};
        }
        break;
      case ShapeMatch::walk_or_snap:
        try {
          route_match(request);
          map_match_results.emplace_back(1.0f, 0.0f, std::vector<meili::MatchResult>{});
        } catch (...) {
          try {
            request.Clear();
            request.mutable_options()->CopyFrom(options);

            map_match_results = map_match(request);
          } catch (const std::exception& e) {
            throw valhalla_exception_t{
                444, ShapeMatch_Enum_Name(options.shape_match()) +
                         " algorithm failed to snap the shape points to the correct shape."};
          }
        }
        break;
      default:
        throw valhalla_exception_t{400, "Invalid shape_match value"};
    }

    // What the serializer gets (it adds the matched points to the trip itself).
    serializer_api = request;
    serializer_results = map_match_results;
    return tyr::serializeTraceAttributes(request, controller, map_match_results);
  }
};

} // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: trace_attributes <valhalla.json> <tile_dir> <requests.tsv>\n";
    return 1;
  }
  boost::property_tree::ptree config;
  rapidjson::read_json(argv[1], config);
  config.put("mjolnir.tile_dir", argv[2]);
  config.put("mjolnir.tile_extract", "");
  config.put("mjolnir.traffic_extract", "");
  config.put("mjolnir.logging.type", "");
  config.put("loki.logging.type", "");
  config.put("thor.logging.type", "");
  config.put("odin.logging.type", "");
  midgard::logging::Configure({{"type", ""}});

  loki::loki_worker_t loki(config);
  oracle_thor_t thor(config);
  const odin::MarkupFormatter markup_formatter(config);

  int mismatches = 0;
  std::ifstream in(argv[3]);
  std::string line;
  while (std::getline(in, line)) {
    if (line.empty()) {
      continue;
    }
    auto tab1 = line.find('\t');
    auto tab2 = line.find('\t', tab1 + 1);
    std::string id = line.substr(0, tab1);
    std::string action_name = line.substr(tab1 + 1, tab2 - tab1 - 1);
    std::string json = line.substr(tab2 + 1);

    Api request;
    std::string out;
    try {
      Options::Action action;
      if (!Options_Action_Enum_Parse(action_name, &action) ||
          (action != Options::trace_attributes && action != Options::trace_route)) {
        throw std::runtime_error("unsupported action " + action_name);
      }
      ParseApi(json, action, request);
      loki.trace(request);
      loki.cleanup();
      if (action == Options::trace_attributes) {
        Api fresh = request;
        Api serializer_api;
        map_match_results_t results;
        auto response = thor.trace_attributes_dumping(request, serializer_api, results);
        thor.cleanup();
        // The worker's own action on the same input must answer the same.
        auto reference = thor.trace_attributes(fresh);
        thor.cleanup();
        if (reference != response) {
          ++mismatches;
          std::cerr << id << ": the repeated steps answer differently from trace_attributes\n";
        }
        out = id + "\tok\t" + b64(serializer_api.SerializeAsString()) + "\t" + b64(dump(results)) +
              "\t" + b64(response);
      } else {
        thor.trace_route(request);
        thor.cleanup();
        std::string response;
        try {
          odin::DirectionsBuilder().Build(request, markup_formatter);
        } catch (const std::exception& e) { throw valhalla_exception_t{202, e.what()}; }
        Api serializer_api = request;
        response = tyr::serializeDirections(request);
        out = id + "\tok\t" + b64(serializer_api.SerializeAsString()) + "\t\t" + b64(response);
      }
    } catch (const valhalla_exception_t& e) {
      out = id + "\terror\t" + std::to_string(e.code) + "\t" + b64(e.message);
    } catch (const std::exception& e) {
      out = id + "\terror\t499\t" + b64(std::string(e.what()));
    }
    loki.cleanup();
    thor.cleanup();
    std::cout << out << '\n';
  }
  if (mismatches) {
    std::cerr << mismatches << " responses differ from the worker's own\n";
    return 2;
  }
  return 0;
}
