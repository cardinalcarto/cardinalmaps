// Runs mainline 3.9.1's Odin (DirectionsBuilder) on Api messages, for illuvatar's fixtures.
//
// Usage: odin [--markup] <in.bin> <out.bin> [<in.bin> <out.bin> ...]
//
// Each input is a serialized valhalla.Api with options and a trip (what Thor sends Odin).
// Each output is the same Api after DirectionsBuilder::Build, so it has the directions
// and the trip with Odin's in-place updates, which is what a PBF response selecting both shows.
// If Odin throws, the output instead holds "error: <what>" as text, as the worker would wrap it.
// With --markup, phoneme markup is enabled with mainline's default template.
#include <valhalla/odin/directionsbuilder.h>
#include <valhalla/odin/markup_formatter.h>
#include <valhalla/proto/api.pb.h>

#include <boost/property_tree/ptree.hpp>

#include <cstdio>
#include <cstring>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>

int main(int argc, char** argv) {
  int first = 1;
  boost::property_tree::ptree config;
  if (argc > 1 && std::strcmp(argv[1], "--markup") == 0) {
    config.put("odin.markup_formatter.markup_enabled", true);
    config.put("odin.markup_formatter.phoneme_format",
               "<TEXTUAL_STRING> (<span class=<QUOTES>phoneme<QUOTES>>/<VERBAL_STRING>/</span>)");
    first = 2;
  }
  valhalla::odin::MarkupFormatter markup_formatter(config);
  for (int i = first; i + 1 < argc; i += 2) {
    std::ifstream in(argv[i], std::ios::binary);
    std::stringstream buffer;
    buffer << in.rdbuf();
    valhalla::Api api;
    if (!api.ParseFromString(buffer.str())) {
      std::cerr << "failed to parse " << argv[i] << "\n";
      return 1;
    }
    std::ofstream out(argv[i + 1], std::ios::binary);
    try {
      valhalla::odin::DirectionsBuilder().Build(api, markup_formatter);
      api.SerializeToOstream(&out);
    } catch (const std::exception& e) { out << "error: " << e.what(); }
  }
  return 0;
}
