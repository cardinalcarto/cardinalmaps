// Runs mainline 3.9.1's Thor worker on Loki's output, as the reference for `valinor-thor`.
//
// Usage:
//   thor <valhalla.json> <tile_dir> <loki_results.tsv> [trip]
//
// The config's `mjolnir` section is pointed at <tile_dir> (with no tile extract or traffic),
// so the same requests can be run against the fixture tiles.
// <loki_results.tsv> is the output of the `loki` helper (loki.cc): each `ok` line holds
// the `valhalla.Api` that Loki sends to Thor, base64-encoded; other lines are skipped.
// For each, this does what `thor_worker_t::work` does (the `route` or `optimized_route`
// action), followed by `cleanup()` (as `prime_server` does after a job), and prints one line:
//
//   <id>\t<base64 of the summary below>
//
// The summary is plain text, one record per line:
//
//   error <code> <message>                 the request failed (nothing else follows)
//   ok                                     the request succeeded
//   warning <code>                         each warning in the response, in order
//   location <i> <date_time>|<offset>|<name>   each location after Thor (times it set)
//   route                                  the start of a route (alternates follow the first)
//   leg <algorithms>                       the start of a leg, with its algorithms (comma-separated)
//   edge <id> <secs> <cost> <tsecs> <tcost>    each path edge: its graph ID, the elapsed time
//                                          and cost at its end, and the transition onto it
//
// The edges are mainline's `PathInfo`s, read back from the trip that `TripLegBuilder` made:
// node i holds edge i and the transition onto it, and node i + 1 the elapsed cost at its end.
// Floats are printed with 17 significant digits, so they round-trip exactly.
//
// With `trip`, each line instead holds the whole `valhalla.Api` that Thor sends to Odin
// (the options with Thor's changes, the warnings, and the `trip` that `TripLegBuilder` made),
// which is the reference for trip building:
//
//   <id>\tok\t<base64 of the serialized Api>
//   <id>\terror\t<code> <message>
#include <valhalla/baldr/rapidjson_utils.h>
#include <valhalla/exceptions.h>
#include <valhalla/midgard/logging.h>
#include <valhalla/midgard/util.h>
#include <valhalla/proto/api.pb.h>
#include <valhalla/thor/worker.h>
#include <valhalla/worker.h>

#include <boost/property_tree/ptree.hpp>

#include <cstdio>
#include <fstream>
#include <iostream>
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

std::string summarize(const Api& api) {
  std::ostringstream out;
  out << "ok\n";
  for (const auto& warning : api.info().warnings()) {
    out << "warning " << warning.code() << '\n';
  }
  for (int i = 0; i < api.options().locations_size(); ++i) {
    const auto& location = api.options().locations(i);
    out << "location " << i << ' ' << location.date_time() << '|' << location.time_zone_offset()
        << '|' << location.time_zone_name() << '\n';
  }
  for (const auto& route : api.trip().routes()) {
    out << "route\n";
    for (const auto& leg : route.legs()) {
      out << "leg ";
      for (int a = 0; a < leg.algorithms_size(); ++a) {
        out << (a ? "," : "") << leg.algorithms(a);
      }
      out << '\n';
      for (int n = 0; n + 1 < leg.node_size(); ++n) {
        const auto& node = leg.node(n);
        const auto& next = leg.node(n + 1);
        out << "edge " << node.edge().id() << ' '
            << number(next.cost().elapsed_cost().seconds()) << ' '
            << number(next.cost().elapsed_cost().cost()) << ' '
            << number(node.cost().transition_cost().seconds()) << ' '
            << number(node.cost().transition_cost().cost()) << '\n';
      }
    }
  }
  return out.str();
}

} // namespace

int main(int argc, char** argv) {
  const bool trip_mode = argc == 5 && std::string(argv[4]) == "trip";
  if (argc != 4 && !trip_mode) {
    std::cerr << "usage: thor <valhalla.json> <tile_dir> <loki_results.tsv> [trip]\n";
    return 1;
  }
  boost::property_tree::ptree config;
  rapidjson::read_json(argv[1], config);
  config.put("mjolnir.tile_dir", argv[2]);
  config.put("mjolnir.tile_extract", "");
  config.put("mjolnir.traffic_extract", "");
  config.put("mjolnir.logging.type", "");
  config.put("thor.logging.type", "");
  midgard::logging::Configure({{"type", ""}});

  thor::thor_worker_t worker(config);

  std::ifstream in(argv[3]);
  std::string line;
  while (std::getline(in, line)) {
    std::vector<std::string> fields;
    std::istringstream split(line);
    std::string field;
    while (std::getline(split, field, '\t')) {
      fields.push_back(field);
    }
    if (fields.size() < 3 || fields[1] != "ok") {
      continue;
    }
    Api request;
    if (!request.ParseFromString(midgard::decode64(fields[2]))) {
      std::cerr << "could not parse the Api of " << fields[0] << '\n';
      return 1;
    }
    std::string summary;
    std::string error;
    try {
      switch (request.options().action()) {
        case Options::route:
          worker.route(request);
          break;
        case Options::optimized_route:
          worker.optimized_route(request);
          break;
        default:
          worker.cleanup();
          continue;
      }
      summary = trip_mode ? request.SerializeAsString() : summarize(request);
    } catch (const valhalla_exception_t& e) {
      error = std::to_string(e.code) + ' ' + e.message;
    } catch (const std::exception& e) {
      error = "499 " + std::string(e.what());
    }
    worker.cleanup();
    if (trip_mode) {
      std::cout << fields[0] << '\t' << (error.empty() ? "ok\t" + midgard::encode64(summary)
                                                       : "error\t" + error)
                << '\n';
    } else {
      std::cout << fields[0] << '\t'
                << midgard::encode64(error.empty() ? summary : "error " + error + '\n') << '\n';
    }
  }
  return 0;
}
