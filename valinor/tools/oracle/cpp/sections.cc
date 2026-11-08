// Dumps the variable-length sections of every tile in a tile directory
// (edge info, signs, turn lanes, access restrictions, complex restrictions,
// lane connectivity, and admins) using mainline 3.9.1's own readers.
// The output is the reference for the valhalla-graphtile section tests,
// which format the same dump from Valinor's decoders and compare it line by line.
//
// With --synthetic instead of a tile directory, it builds records that the fixtures lack
// (landmarks, conditional speeds, OSM node IDs, fractional levels, timed complex restrictions,
// lane connectivity, and so on) with mainline's own encoders where they are public,
// and prints their bytes followed by mainline's decoding of them.
// Usage: sections <tile_dir> | sections --synthetic
#include <valhalla/baldr/accessrestriction.h>
#include <valhalla/baldr/complexrestriction.h>
#include <valhalla/baldr/conditional_speed_limit.h>
#include <valhalla/baldr/edgeinfo.h>
#include <valhalla/baldr/graphconstants.h>
#include <valhalla/baldr/graphreader.h>
#include <valhalla/baldr/laneconnectivity.h>
#include <valhalla/baldr/timedomain.h>
#include <valhalla/baldr/landmark.h>
#include <valhalla/baldr/turnlanes.h>
#include <valhalla/midgard/encoded.h>
#include <valhalla/mjolnir/complexrestrictionbuilder.h>

#include <boost/property_tree/ptree.hpp>

#include <algorithm>
#include <cstdio>
#include <set>
#include <sstream>
#include <tuple>
#include <string>
#include <vector>

using namespace valhalla;
using namespace valhalla::baldr;

namespace {

// Reads the raw complex restriction memory, which GraphTile only exposes through id lookups.
struct TilePeek : GraphTile {
  static std::pair<const char*, size_t> forward(const GraphTile* t) {
    const auto* p = static_cast<const TilePeek*>(t);
    return {p->complex_restriction_forward_, p->complex_restriction_forward_size_};
  }
  static std::pair<const char*, size_t> reverse(const GraphTile* t) {
    const auto* p = static_cast<const TilePeek*>(t);
    return {p->complex_restriction_reverse_, p->complex_restriction_reverse_size_};
  }
  static const Sign* signs(const GraphTile* t) {
    return static_cast<const TilePeek*>(t)->signs_;
  }
  static const TurnLanes* turnlanes(const GraphTile* t) {
    return static_cast<const TilePeek*>(t)->turnlanes_;
  }
  static const AccessRestriction* access_restrictions(const GraphTile* t) {
    return static_cast<const TilePeek*>(t)->access_restrictions_;
  }
  static std::pair<const LaneConnectivity*, size_t> lanes(const GraphTile* t) {
    const auto* p = static_cast<const TilePeek*>(t);
    return {p->lane_connectivity_, p->lane_connectivity_size_ / sizeof(LaneConnectivity)};
  }
};

// Printable ASCII stays as is; everything else (including UTF-8) is \xHH.
std::string esc(const std::string& s) {
  std::string out;
  char buf[8];
  for (unsigned char c : s) {
    if (c >= 0x20 && c < 0x7f && c != '\\') {
      out.push_back(static_cast<char>(c));
    } else {
      snprintf(buf, sizeof(buf), "\\x%02x", c);
      out += buf;
    }
  }
  return out;
}

std::string gid(const GraphId& id) {
  if (!id.is_valid()) {
    return "invalid";
  }
  return std::to_string(id.level()) + "/" + std::to_string(id.tileid()) + "/" +
         std::to_string(id.id());
}

void dump_restriction(const char* label, const ComplexRestriction& cr) {
  printf(" %s_restriction from=%s to=%s type=%u modes=%u vias=%u", label, gid(cr.from_graphid()).c_str(),
         gid(cr.to_graphid()).c_str(), static_cast<unsigned>(cr.type()), cr.modes(),
         cr.via_count());
  cr.WalkVias([](const GraphId* via) {
    printf(" %s", gid(*via).c_str());
    return WalkingVia::KeepWalking;
  });
  printf(" probability=%u has_dt=%u", cr.probability(), cr.has_dt());
  // Mainline leaves the date and time fields uninitialized when has_dt is 0,
  // so they differ between builds of the same tile; only print them when they mean something.
  if (cr.has_dt()) {
    printf(" dt_type=%u dow=%u begin=%u/%u/%u %u:%u end=%u/%u/%u %u:%u", cr.dt_type(), cr.dow(),
           cr.begin_month(), cr.begin_week(), cr.begin_day_dow(), cr.begin_hrs(), cr.begin_mins(),
           cr.end_month(), cr.end_week(), cr.end_day_dow(), cr.end_hrs(), cr.end_mins());
  }
  printf("\n");
}

void dump_restrictions(const char* label, std::pair<const char*, size_t> mem) {
  size_t offset = 0, count = 0;
  while (offset < mem.second) {
    const auto* cr = reinterpret_cast<const ComplexRestriction*>(mem.first + offset);
    dump_restriction(label, *cr);
    offset += cr->SizeOf();
    ++count;
  }
  printf(" %s_restrictions count=%zu\n", label, count);
}

void dump_edge_info(const EdgeInfo& ei, uint32_t edge, uint32_t offset) {
  printf(" edgeinfo edge=%u offset=%u way=%llu mean_elevation=%.9g speed_limit=%u bike=%u "
         "has_elevation=%d names=%u types=%u\n",
         edge, offset, (unsigned long long)ei.wayid(), ei.mean_elevation(),
         ei.speed_limit(), ei.bike_network(), ei.has_elevation(), ei.name_count(), ei.GetTypes());
  for (const auto& [name, rn, tag] : ei.GetNamesAndTypes(false)) {
    printf("  name rn=%d %s\n", rn, esc(name).c_str());
  }
  for (const auto& [tag, value] : ei.GetTags()) {
    printf("  tag %u %s\n", static_cast<unsigned>(tag), esc(value).c_str());
  }
  for (const auto& value : ei.GetLinguisticTaggedValues()) {
    printf("  linguistic %s\n", esc(value).c_str());
  }
  {
    auto map = ei.GetLinguisticMap();
    std::vector<uint8_t> keys;
    for (const auto& kv : map)
      keys.push_back(kv.first);
    std::sort(keys.begin(), keys.end());
    for (auto k : keys) {
      const auto& [lang, alphabet, pronunciation] = map.at(k);
      printf("  linguistic_map %u lang=%u alphabet=%u %s\n", k, lang, alphabet,
             esc(pronunciation).c_str());
    }
  }
  if (ei.layer() != 0) {
    printf("  layer %d\n", ei.layer());
  }
  auto [levels, precision] = ei.levels();
  if (!levels.empty()) {
    printf("  levels precision=%u:", precision);
    for (const auto& [lo, hi] : levels)
      printf(" %.9g-%.9g", lo, hi);
    printf("\n");
    printf("  includes_level:");
    for (float probe : {-2.0f, -1.0f, 0.0f, 0.5f, 1.0f, 2.5f, 3.0f, 13.0f, 14.0f, 17.5f, 25.0f})
      printf(" %.9g=%d", probe, ei.includes_level(probe));
    printf("\n");
  }
  for (const auto& r : ei.level_ref()) {
    printf("  level_ref %s\n", esc(r).c_str());
  }
  auto node_ids = ei.osm_node_ids();
  if (!node_ids.empty()) {
    printf("  osm_node_ids:");
    for (auto id : node_ids)
      printf(" %llu", (unsigned long long)id);
    printf("\n");
  }
  for (const auto& l : ei.conditional_speed_limits()) {
    printf("  conditional_speed td=%llu speed=%u \"%s\"\n", (unsigned long long)l.td_.td_value(),
           static_cast<unsigned>(l.speed_), l.td_.to_string().c_str());
  }
  for (const auto& [tag, value] : ei.GetTags()) {
    if (tag == TaggedValue::kLandmark) {
      Landmark l(value);
      printf("  landmark type=%u lng=%.7f lat=%.7f %s\n", static_cast<unsigned>(l.type), l.lng,
             l.lat, esc(l.name).c_str());
    }
  }
}

void dump_tile(GraphReader& reader, const GraphId& tile_id) {
  auto tile = reader.GetGraphTile(tile_id);
  const auto* header = tile->header();
  printf("tile %s\n", gid(tile_id).c_str());

  // Edge info, once per distinct offset, in edge order.
  std::set<uint32_t> seen;
  for (uint32_t i = 0; i < header->directededgecount(); ++i) {
    const auto* de = tile->directededge(i);
    auto ei = tile->edgeinfo(de);

    // Elevation depends on the edge length, so it is per directed edge.
    double interval = 0;
    auto elevation = ei.encoded_elevation(de->length(), interval);
    if (ei.has_elevation()) {
      printf(" elevation edge=%u length=%u interval=%.17g:", i, de->length(), interval);
      for (auto e : elevation)
        printf(" %d", e);
      printf("\n");
    }

    if (!seen.insert(de->edgeinfo_offset()).second) {
      continue;
    }
    dump_edge_info(ei, i, de->edgeinfo_offset());
  }

  // Signs: the raw list, then what GetSigns returns per edge and per node.
  for (uint32_t i = 0; i < header->signcount(); ++i) {
    const auto& s = TilePeek::signs(tile.get())[i];
    printf(" sign %u index=%u type=%u rn=%d tagged=%d offset=%u\n", i, s.index(),
           static_cast<unsigned>(s.type()), s.is_route_num_type(), s.tagged(), s.text_offset());
  }
  std::set<uint32_t> sign_indexes;
  for (uint32_t i = 0; i < header->signcount(); ++i) {
    sign_indexes.insert(TilePeek::signs(tile.get())[i].index());
  }
  for (auto i : sign_indexes) {
    for (bool on_node : {false, true}) {
      for (const auto& s : tile->GetSigns(i, on_node)) {
        printf(" get_signs %s=%u type=%u rn=%d tagged=%d %s\n", on_node ? "node" : "edge", i,
               static_cast<unsigned>(s.type()), s.is_route_num(), s.is_tagged(),
               esc(s.text()).c_str());
      }
      std::unordered_map<uint8_t, std::tuple<uint8_t, uint8_t, std::string>> map;
      for (const auto& s : tile->GetSigns(i, map, on_node)) {
        printf(" get_signs_linguistic %s=%u type=%u rn=%d tagged=%d %s\n",
               on_node ? "node" : "edge", i, static_cast<unsigned>(s.type()), s.is_route_num(),
               s.is_tagged(), esc(s.text()).c_str());
      }
      std::vector<uint8_t> keys;
      for (const auto& kv : map)
        keys.push_back(kv.first);
      std::sort(keys.begin(), keys.end());
      for (auto k : keys) {
        const auto& [lang, alphabet, pronunciation] = map.at(k);
        printf(" sign_linguistic_map %s=%u %u lang=%u alphabet=%u %s\n", on_node ? "node" : "edge",
               i, k, lang, alphabet, esc(pronunciation).c_str());
      }
    }
  }

  // Turn lanes.
  for (uint32_t i = 0; i < header->turnlane_count(); ++i) {
    const auto& tl = TilePeek::turnlanes(tile.get())[i];
    auto masks = tile->turnlanes(tl.edgeindex());
    printf(" turn_lanes edge=%u offset=%u text=%s masks=%s\n", tl.edgeindex(), tl.text_offset(),
           esc(tile->GetName(tl.text_offset())).c_str(),
           esc(TurnLanes::turnlane_string(masks)).c_str());
  }

  // Access restrictions.
  for (uint32_t i = 0; i < header->access_restriction_count(); ++i) {
    const auto& r = TilePeek::access_restrictions(tile.get())[i];
    printf(" access_restriction edge=%u type=%u modes=%u except_destination=%d value=%llu\n",
           r.edgeindex(), static_cast<unsigned>(r.type()), r.modes(), r.except_destination(),
           (unsigned long long)r.value());
  }

  // Complex restrictions, raw, then the per-edge lookups.
  dump_restrictions("forward", TilePeek::forward(tile.get()));
  dump_restrictions("reverse", TilePeek::reverse(tile.get()));
  for (uint32_t i = 0; i < header->directededgecount(); ++i) {
    GraphId id(tile_id.tileid(), tile_id.level(), i);
    for (bool forward : {true, false}) {
      for (const auto& cr : tile->GetComplexRestrictions(forward, id, kAllAccess)) {
        printf(" get_restrictions edge=%u %s from=%s to=%s\n", i, forward ? "forward" : "reverse",
               gid(cr.from_graphid()).c_str(), gid(cr.to_graphid()).c_str());
      }
    }
  }

  // Lane connectivity.
  auto [lanes, lane_count] = TilePeek::lanes(tile.get());
  for (size_t i = 0; i < lane_count; ++i) {
    const auto& lc = lanes[i];
    printf(" lane_connectivity to=%u from=%llu to_lanes=%s from_lanes=%s\n", lc.to(),
           (unsigned long long)lc.from(), lc.to_lanes().c_str(), lc.from_lanes().c_str());
  }
  for (uint32_t i = 0; i < header->directededgecount(); ++i) {
    auto span = tile->GetLaneConnectivity(i);
    if (!span.empty()) {
      printf(" get_lane_connectivity edge=%u count=%zu\n", i, span.size());
    }
  }

  // Admins.
  for (uint32_t i = 0; i < header->admincount(); ++i) {
    auto a = tile->admininfo(i);
    printf(" admin %u country=%s state=%s country_iso=%s state_iso=%s\n", i,
           esc(a.country_text()).c_str(), esc(a.state_text()).c_str(), a.country_iso().c_str(),
           a.state_iso().c_str());
  }
}


std::string hex(const std::string& s) {
  std::string out;
  char buf[4];
  for (unsigned char c : s) {
    snprintf(buf, sizeof(buf), "%02x", c);
    out += buf;
  }
  return out;
}

template <typename T> std::string bytes_of(const T& value) {
  return std::string(reinterpret_cast<const char*>(&value), sizeof(T));
}

// A verbatim copy of encode_level from mjolnir/osmway.cc (it is in an anonymous namespace there).
std::string encode_level(const float lvl, const int precision = 0) {
  std::string encoded;
  encoded.reserve(2);
  int val = static_cast<int>(lvl * pow(10, precision));
  val = val < 0 ? ~(*reinterpret_cast<unsigned int*>(&val) << 1) : val << 1;
  while (val > 0x7f) {
    int next_value = (0x80 | (val & 0x7f));
    encoded.push_back(static_cast<char>(next_value));
    val >>= 7;
  }
  encoded.push_back(static_cast<char>(val & 0x7f));
  return encoded;
}

// The levels encoding from OSMWay::GetTaggedValues, for already sorted and parsed ranges.
std::string encode_levels(const std::vector<std::pair<float, float>>& values, int precision) {
  std::string levels_encoded;
  std::string sep = encode_level(kLevelRangeSeparator);
  auto prec_power = pow(10, precision);
  for (size_t i = 0; i < values.size(); ++i) {
    const auto& range = values[i];
    levels_encoded.append(encode_level(range.first * prec_power));
    if (range.first != range.second)
      levels_encoded.append(encode_level(range.second * prec_power));
    if (i + 1 != values.size())
      levels_encoded.append(sep);
  }
  std::string precision_enc = encode_level(static_cast<float>(precision));
  return std::string(1, static_cast<char>(TaggedValue::kLevels)) +
         encode_level(static_cast<float>(levels_encoded.size() + precision_enc.size())) +
         precision_enc + levels_encoded;
}

std::string linguistic(uint8_t language,
                       uint8_t alphabet,
                       uint8_t name_index,
                       const std::string& pronunciation) {
  linguistic_text_header_t header{language, static_cast<uint32_t>(pronunciation.size()), alphabet,
                                  name_index, 0, 0};
  return std::string(reinterpret_cast<const char*>(&header), kLinguisticHeaderSize) +
         pronunciation;
}

TimeDomain time_domain(bool nth_dow,
                       uint8_t dow,
                       uint8_t bm,
                       uint8_t bw,
                       uint8_t bd,
                       uint8_t bh,
                       uint8_t bmin,
                       uint8_t em,
                       uint8_t ew,
                       uint8_t ed,
                       uint8_t eh,
                       uint8_t emin) {
  TimeDomain td;
  td.set_type(nth_dow);
  td.set_dow(dow);
  td.set_begin_month(bm);
  td.set_begin_week(bw);
  td.set_begin_day_dow(bd);
  td.set_begin_hrs(bh);
  td.set_begin_mins(bmin);
  td.set_end_month(em);
  td.set_end_week(ew);
  td.set_end_day_dow(ed);
  td.set_end_hrs(eh);
  td.set_end_mins(emin);
  return td;
}

struct SyntheticEntry {
  std::string value;
  bool route_num;
  bool tagged;
};

struct SyntheticEdge {
  std::vector<SyntheticEntry> entries;
  uint64_t way_id;
  std::vector<int8_t> elevation;
  uint32_t length;
};

void synthetic() {
  // Time domains, from a fixed pseudo-random sequence of valid fields.
  uint64_t seed = 0x5eed;
  auto next = [&](uint32_t n) {
    seed = seed * 6364136223846793005ULL + 1442695040888963407ULL;
    return static_cast<uint8_t>((seed >> 33) % n);
  };
  // Leave fields unset a third of the time, to cover the "not set" branches.
  auto maybe = [&](uint32_t n) { return next(3) == 0 ? 0 : next(n); };
  for (int i = 0; i < 300; ++i) {
    bool nth = next(2);
    uint32_t days = nth ? 8 : 32;
    uint8_t dow = next(4) == 0 ? 127 : maybe(128);
    uint8_t bm = maybe(13), bw = maybe(6), bd = maybe(days), bh = maybe(24), bmin = maybe(60);
    uint8_t em = maybe(13), ew = maybe(6), ed = maybe(days), eh = maybe(24), emin = maybe(60);
    auto td = time_domain(nth, dow, bm, bw, bd, bh, bmin, em, ew, ed, eh, emin);
    printf("td %llu %s\n", (unsigned long long)td.td_value(), td.to_string().c_str());
  }

  // Edge infos with every kind of tagged value, sharing one text list.
  std::string text(1, '\0');
  auto add = [&](const std::string& value) {
    uint32_t offset = text.size();
    text += value;
    text.push_back('\0');
    return offset;
  };
  auto tag = [](TaggedValue t) { return std::string(1, static_cast<char>(t)); };

  ConditionalSpeedLimit limit{TimeDomain()};
  limit.td_ = time_domain(false, 0b0111110, 0, 0, 0, 7, 0, 0, 0, 0, 9, 30);
  limit.speed_ = 30;
  ConditionalSpeedLimit limit2{TimeDomain()};
  limit2.td_ = time_domain(true, 0, 12, 5, 1, 0, 0, 1, 1, 7, 0, 0);
  limit2.speed_ = 90;

  std::string node_ids = tag(TaggedValue::kOSMNodeIds);
  node_ids += midgard::encode7int(
      std::vector<uint64_t>{12345678901ULL, 12345678905ULL, 1ULL, (1ULL << 40) + 3});
  node_ids.insert(1, midgard::encode7int(std::vector{node_ids.size() - 1}));

  std::string ling = tag(TaggedValue::kLinguistic) + linguistic(9, 5, 0, "") +
                     linguistic(17, 1, 1, "abc") + linguistic(17, 4, 1, "xyz") +
                     linguistic(22, 4, 2, "fr");

  std::vector<SyntheticEdge> edges = {
      {{{"Carrer Major", false, false},
        {"CG-1", true, false},
        {tag(TaggedValue::kTunnel) + "T\xc3\xbanel d'Envalira", false, true},
        {tag(TaggedValue::kBridge) + "Pont", false, true},
        {tag(TaggedValue::kLayer) + std::string(1, static_cast<char>(-2)), false, true},
        {ling, false, true},
        {tag(TaggedValue::kBssInfo) + "bss", false, true},
        {tag(TaggedValue::kLevel) + "1;2", false, true},
        {tag(TaggedValue::kLevelRef) + "P1", false, true},
        {tag(TaggedValue::kLevelRef) + "P2", false, true},
        {encode_levels({{0, 12}, {14, 14}}, 0), false, true},
        {tag(TaggedValue::kConditionalSpeedLimits) + bytes_of(limit), false, true},
        {tag(TaggedValue::kConditionalSpeedLimits) + bytes_of(limit2), false, true},
        {tag(TaggedValue::kLandmark) +
             Landmark(0, "Repsol", LandmarkType::fuel, 1.5213456, 42.5063).to_str(),
         false, true},
        {tag(TaggedValue::kLandmark) +
             Landmark(0, "Caf\xc3\xa8", LandmarkType::cafe, -180.0, -90.0).to_str(),
         false, true}},
       0x0102030405060708ULL,
       {4, -4, 1},
       100},
      {{{node_ids, false, true}, {encode_levels({{-1, -1}, {0.5, 0.5}}, 1), false, true}},
       1ULL << 40,
       {},
       20},
      {{{encode_levels({{0.25, 0.25}, {1.75, 2.5}}, 2), false, true}}, 7, {}, 20},
      {{{encode_levels({{-2, -1}, {3, 3}}, 0), false, true}}, 8, {1, 2, 3, 4, 5, 6}, 200},
  };

  std::vector<std::string> records;
  for (const auto& e : edges) {
    EdgeInfo::EdgeInfoInner inner{};
    inner.wayid_ = static_cast<uint32_t>(e.way_id);
    inner.mean_elevation_ = 760;
    inner.bike_network_ = kRcn;
    inner.speed_limit_ = 50;
    inner.extended_wayid0_ = (e.way_id >> 32) & 0xff;
    inner.extended_wayid1_ = (e.way_id >> 40) & 0xff;
    inner.extended_wayid_size_ = (e.way_id >> 56) ? 2 : ((e.way_id >> 48) ? 1 : 0);
    inner.name_count_ = e.entries.size();
    std::string shape =
        midgard::encode7(std::vector<midgard::PointLL>{{1.5, 42.5}, {1.501, 42.502}});
    inner.encoded_shape_size_ = shape.size();
    inner.has_elevation_ = !e.elevation.empty();
    std::string record = bytes_of(inner);
    for (const auto& entry : e.entries) {
      NameInfo ni{add(entry.value), 0, entry.route_num, entry.tagged, 0};
      record += bytes_of(ni);
    }
    record += shape;
    if (inner.extended_wayid_size_ > 0)
      record.push_back(static_cast<char>((e.way_id >> 48) & 0xff));
    if (inner.extended_wayid_size_ > 1)
      record.push_back(static_cast<char>((e.way_id >> 56) & 0xff));
    record.append(reinterpret_cast<const char*>(e.elevation.data()), e.elevation.size());
    records.push_back(record);
  }
  printf("text %s\n", hex(text).c_str());
  for (size_t i = 0; i < edges.size(); ++i) {
    // Copy into a buffer with some slack, since mainline reads the elevation without bounds.
    std::string buffer = records[i] + std::string(64, '\0');
    printf("edgeinfo_bytes %u %s\n", edges[i].length, hex(records[i]).c_str());
    EdgeInfo ei(buffer.data(), text.data(), text.size());
    double interval = 0;
    auto elevation = ei.encoded_elevation(edges[i].length, interval);
    printf(" elevation edge=%zu length=%u interval=%.17g:", i, edges[i].length, interval);
    for (auto v : elevation)
      printf(" %d", v);
    printf("\n");
    dump_edge_info(ei, i, 0);
  }

  // Complex restrictions, as ComplexRestrictionBuilder serializes them.
  {
    mjolnir::ComplexRestrictionBuilder crb;
    crb.set_from_id(GraphId(3015, 0, 12));
    crb.set_to_id(GraphId(47701, 1, 7));
    crb.set_via_list({GraphId(762485, 2, 1), GraphId(3015, 0, 13)});

    crb.set_type(RestrictionType::kNoProbable);
    crb.set_modes(kAutoAccess | kBicycleAccess | kMotorcycleAccess);
    crb.set_dt(true);
    crb.set_dt_type(true);
    crb.set_dow(0b0111110);
    crb.set_begin_month(3);
    crb.set_begin_week(5);
    crb.set_begin_day_dow(1);
    crb.set_begin_hrs(7);
    crb.set_begin_mins(30);
    crb.set_end_month(10);
    crb.set_end_week(1);
    crb.set_end_day_dow(7);
    crb.set_end_hrs(19);
    crb.set_end_mins(59);
    crb.set_probability(42);
    std::ostringstream os;
    os << crb;
    std::string bytes = os.str();
    printf("restriction_bytes %s\n", hex(bytes).c_str());
    dump_restriction("synthetic", *reinterpret_cast<const ComplexRestriction*>(bytes.data()));
  }

  // Lane connectivity, as the constructor packs it.
  std::vector<std::tuple<uint32_t, uint64_t, std::string, std::string>> lanes = {
      {4194303, (1ULL << 42) - 1, "1|2|15", "3|4"},
      {7, 12345, "1", "2|3|4|5|6|7|8"},
  };
  for (const auto& [to, from, to_lanes, from_lanes] : lanes) {
    LaneConnectivity lc(to, from, to_lanes, from_lanes);
    printf("lane_connectivity_bytes %s to=%u from=%llu to_lanes=%s from_lanes=%s\n",
           hex(bytes_of(lc)).c_str(), lc.to(), (unsigned long long)lc.from(),
           lc.to_lanes().c_str(), lc.from_lanes().c_str());
  }

  // Access restrictions, as the constructor packs them.
  uint64_t rush_hour = time_domain(false, 0b0111110, 0, 0, 0, 7, 0, 0, 0, 0, 9, 0).td_value();
  std::vector<AccessRestriction> restrictions = {
      AccessRestriction(1234, AccessType::kMaxWeight, kAllAccess, 350, true),
      AccessRestriction(4194303, AccessType::kTimedDenied, kTruckAccess, rush_hour, false),
      AccessRestriction(5, AccessType::kMaxAxles, kBusAccess, 3, true),
  };
  for (const auto& r : restrictions) {
    printf("access_restriction_bytes %s edge=%u type=%u modes=%u except_destination=%d "
           "value=%llu\n",
           hex(bytes_of(r)).c_str(), r.edgeindex(), static_cast<unsigned>(r.type()), r.modes(),
           r.except_destination(), (unsigned long long)r.value());
  }
}

} // namespace

int main(int argc, char** argv) {
  if (argc > 1 && std::string(argv[1]) == "--synthetic") {
    synthetic();
    return 0;
  }
  boost::property_tree::ptree pt;
  pt.put("tile_dir", argv[1]);
  GraphReader reader(pt);
  auto ids = reader.GetTileSet();
  std::vector<GraphId> sorted(ids.begin(), ids.end());
  std::sort(sorted.begin(), sorted.end(), [](const GraphId& a, const GraphId& b) {
    return a.level() == b.level() ? a.tileid() < b.tileid() : a.level() < b.level();
  });
  for (const auto& id : sorted) {
    dump_tile(reader, id);
  }
  return 0;
}
