// Runs mainline 3.9.1's Loki worker on a list of requests, as the reference for `valinor-loki`.
//
// Usage:
//   loki <valhalla.json> <tile_dir> <requests.tsv>
//
// The config's `mjolnir` section is pointed at <tile_dir> (with no tile extract or traffic),
// so the same requests can be run against the fixture tiles.
// Each line of <requests.tsv> is `<id>\t<action>\t<json>`.
// For each request, this does what `loki_worker_t::work` does for an HTTP request
// (`ParseApi`, then the action), followed by `cleanup()` (as `prime_server` does after a job),
// and prints one line:
//
//   <id>\tok\t<Api>                       the request as Loki sends it to Thor
//   <id>\tlocate\t<Api>\t<response>       a locate request (Loki answers it itself)
//   <id>\terror\t<code>\t<message>\t<Api> a request that failed, and its state when it did
//
// where <Api> is the serialized `valhalla.Api` and <response> and <message> are strings,
// all base64-encoded.
// Loki adds a latency statistic to every request, which the tests ignore.
#include <valhalla/baldr/rapidjson_utils.h>
#include <valhalla/exceptions.h>
#include <valhalla/loki/worker.h>
#include <valhalla/midgard/logging.h>
#include <valhalla/midgard/util.h>
#include <valhalla/worker.h>

#include <boost/property_tree/ptree.hpp>

#include <cstdio>
#include <fstream>
#include <iostream>
#include <string>

using namespace valhalla;

namespace {

std::string b64(const std::string& s) {
  return midgard::encode64(s);
}

} // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: loki <valhalla.json> <tile_dir> <requests.tsv>\n";
    return 1;
  }
  boost::property_tree::ptree config;
  rapidjson::read_json(argv[1], config);
  config.put("mjolnir.tile_dir", argv[2]);
  config.put("mjolnir.tile_extract", "");
  config.put("mjolnir.traffic_extract", "");
  config.put("mjolnir.logging.type", "");
  config.put("loki.logging.type", "");
  midgard::logging::Configure({{"type", ""}});

  loki::loki_worker_t worker(config);

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
      if (!Options_Action_Enum_Parse(action_name, &action)) {
        throw std::runtime_error("unknown action " + action_name);
      }
      ParseApi(json, action, request);
      switch (request.options().action()) {
        case Options::route:
        case Options::centroid:
          worker.route(request);
          out = id + "\tok\t" + b64(request.SerializeAsString());
          break;
        case Options::locate: {
          auto response = worker.locate(request);
          out = id + "\tlocate\t" + b64(request.SerializeAsString()) + "\t" + b64(response);
          break;
        }
        case Options::sources_to_targets:
        case Options::optimized_route:
          worker.matrix(request);
          out = id + "\tok\t" + b64(request.SerializeAsString());
          break;
        case Options::trace_attributes:
        case Options::trace_route:
          worker.trace(request);
          out = id + "\tok\t" + b64(request.SerializeAsString());
          break;
        default:
          throw valhalla_exception_t{107};
      }
    } catch (const valhalla_exception_t& e) {
      out = id + "\terror\t" + std::to_string(e.code) + "\t" + b64(e.message) + "\t" +
            b64(request.SerializeAsString());
    } catch (const std::exception& e) {
      out = id + "\terror\t199\t" + b64(std::string(e.what())) + "\t" +
            b64(request.SerializeAsString());
    }
    worker.cleanup();
    std::cout << out << '\n';
  }
  return 0;
}
