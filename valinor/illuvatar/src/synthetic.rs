//! Synthetic trips for maneuvers and phrases the captured routes do not reach.
//!
//! Real routes rarely pass building entrances, bike share stations, or exits with every kind of
//! sign, and the narrative's number formatting only shows thousands separators on very long
//! maneuvers. These trips are built by hand, written to `fixtures/synthetic`,
//! and run through mainline's own `DirectionsBuilder` by `tools/oracle/odin-synthetic.sh`
//! (with the `odin` helper in `tools/oracle/cpp`), whose output is the reference.
//! So the expectations still come from mainline, not from us.
//!
//! Regenerate the inputs with `cargo test -p illuvatar -- --ignored write_synthetic_inputs`.

use std::fs;
use std::path::Path;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::SideOfStreet;
use valhalla_proto::options::{HasLanguage, HasRoundaboutExits, Units};
use valhalla_proto::prost::Message;
use valhalla_proto::trip_leg::edge::Level;
use valhalla_proto::trip_leg::{
    Admin, Cost, Edge, IntersectingEdge, Node, PathCost, Traversability, Use, node,
};
use valhalla_proto::{
    Api, BikeShareStationInfo, LatLng, Location, Options, PedestrianType, Pronunciation, RoadClass,
    StreetName, TaggedValue, TravelMode, Trip, TripLeg, TripRoute, TripSign, TripSignElement,
    TurnLane, pronunciation, tagged_value,
};

/// Where the synthetic cases live.
pub(crate) const SYNTHETIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/synthetic");

fn names(values: &[&str]) -> Vec<StreetName> {
    values
        .iter()
        .map(|value| StreetName {
            value: (*value).to_owned(),
            // Mainline marks names that look like route numbers; tests use a digit as the cue.
            is_route_number: value.chars().any(|c| c.is_ascii_digit()),
            ..StreetName::default()
        })
        .collect()
}

fn sign_elements(values: &[&str]) -> Vec<TripSignElement> {
    values
        .iter()
        .map(|value| TripSignElement {
            text: (*value).to_owned(),
            is_route_number: value.chars().any(|c| c.is_ascii_digit()),
            ..TripSignElement::default()
        })
        .collect()
}

/// A drivable road edge with the given name, heading, and length.
fn road(name: &[&str], heading: u32, length_km: f32) -> Edge {
    Edge {
        name: names(name),
        length_km,
        speed: 40.0,
        default_speed: 40.0,
        road_class: RoadClass::KResidential as i32,
        begin_heading: heading,
        end_heading: heading,
        traversability: Traversability::KBoth as i32,
        r#use: Use::KRoadUse as i32,
        travel_mode: TravelMode::KDrive as i32,
        lane_count: 1,
        ..Edge::default()
    }
}

/// A footway edge for a pedestrian.
fn footway(name: &[&str], heading: u32, length_km: f32) -> Edge {
    Edge {
        r#use: Use::KFootwayUse as i32,
        road_class: RoadClass::KServiceOther as i32,
        travel_mode: TravelMode::KPedestrian as i32,
        speed: 5.1,
        default_speed: 5.1,
        ..road(name, heading, length_km)
    }
}

/// An intersecting edge that every mode can take in both directions.
fn xedge(heading: u32, name: &[&str]) -> IntersectingEdge {
    IntersectingEdge {
        begin_heading: heading,
        driveability: Traversability::KBoth as i32,
        cyclability: Traversability::KBoth as i32,
        walkability: Traversability::KBoth as i32,
        r#use: Use::KRoadUse as i32,
        road_class: RoadClass::KResidential as i32,
        name: names(name),
        ..IntersectingEdge::default()
    }
}

fn lat_lng(lat: f64, lng: f64) -> LatLng {
    LatLng {
        has_lat: Some(HasLat::Lat(lat)),
        has_lng: Some(HasLng::Lng(lng)),
    }
}

/// Builds a trip leg node by node.
struct LegBuilder {
    leg: TripLeg,
    elapsed: f64,
    shape_index: u32,
}

impl LegBuilder {
    fn new(country_code: &str, state_code: &str) -> Self {
        let leg = TripLeg {
            admin: vec![Admin {
                country_code: country_code.to_owned(),
                state_code: state_code.to_owned(),
                ..Admin::default()
            }],
            location: vec![
                Location {
                    ll: Some(lat_lng(1.0, 1.0)),
                    ..Location::default()
                },
                Location {
                    ll: Some(lat_lng(1.1, 1.1)),
                    ..Location::default()
                },
            ],
            ..TripLeg::default()
        };
        Self {
            leg,
            elapsed: 0.0,
            shape_index: 0,
        }
    }

    fn cost(&self) -> PathCost {
        PathCost {
            elapsed_cost: Some(Cost {
                seconds: self.elapsed,
                cost: self.elapsed,
            }),
            transition_cost: Some(Cost::default()),
        }
    }

    /// Adds a node and the edge leaving it.
    fn edge(mut self, mut edge: Edge) -> Self {
        edge.begin_shape_index = self.shape_index;
        self.shape_index += 1;
        edge.end_shape_index = self.shape_index;
        let node = Node {
            cost: Some(self.cost()),
            ..Node::default()
        };
        self.elapsed += f64::from(edge.length_km) / f64::from(edge.speed.max(1.0)) * 3600.0;
        self.leg.node.push(Node {
            edge: Some(edge),
            ..node
        });
        self
    }

    /// Changes the node that the last edge leaves from.
    fn at(mut self, f: impl FnOnce(&mut Node)) -> Self {
        if let Some(node) = self.leg.node.last_mut() {
            f(node);
        }
        self
    }

    fn origin(mut self, f: impl FnOnce(&mut Location)) -> Self {
        f(&mut self.leg.location[0]);
        self
    }

    fn destination(mut self, f: impl FnOnce(&mut Location)) -> Self {
        f(&mut self.leg.location[1]);
        self
    }

    fn finish(mut self) -> TripLeg {
        let node = Node {
            cost: Some(self.cost()),
            ..Node::default()
        };
        self.leg.node.push(node);
        self.leg
    }
}

fn api(language: &str, units: Units, legs: Vec<TripLeg>) -> Api {
    Api {
        options: Some(Options {
            units: units as i32,
            has_language: Some(HasLanguage::Language(language.to_owned())),
            has_roundabout_exits: Some(HasRoundaboutExits::RoundaboutExits(true)),
            ..Options::default()
        }),
        trip: Some(Trip {
            routes: vec![TripRoute {
                legs,
                ..TripRoute::default()
            }],
        }),
        ..Api::default()
    }
}

/// A short town route with a few turns: left, right, sharp, slight, and a U-turn.
fn turns() -> TripLeg {
    LegBuilder::new("AD", "")
        .edge(road(&["Carrer A"], 0, 0.3))
        .edge(road(&["Carrer B"], 270, 0.2))
        .at(|n| n.intersecting_edge = vec![xedge(0, &["Carrer A"]), xedge(90, &["Carrer B"])])
        .edge(road(&["Carrer C"], 0, 0.25))
        .at(|n| n.intersecting_edge = vec![xedge(270, &["Carrer B"]), xedge(180, &["Carrer D"])])
        .edge(road(&["Carrer D"], 150, 0.4))
        .at(|n| n.intersecting_edge = vec![xedge(0, &["Carrer C"])])
        .edge(road(&["Carrer E"], 170, 0.6))
        .at(|n| n.intersecting_edge = vec![xedge(250, &["Carrer F"])])
        .finish()
}

/// The synthetic cases, by name.
///
/// Names ending in `_markup` are run with phoneme markup enabled.
#[expect(clippy::too_many_lines, reason = "one place to read all the cases")]
pub(crate) fn cases() -> Vec<(&'static str, Api)> {
    let mut cases = Vec::new();

    // Lengths with thousands separators and decimals, per locale number format.
    for (name, language, units) in [
        ("long_lengths_de_de", "de-DE", Units::Kilometers),
        ("long_lengths_fr_fr", "fr-FR", Units::Kilometers),
        ("long_lengths_pt_pt", "pt-PT", Units::Kilometers),
        ("long_lengths_en_us_miles", "en-US", Units::Miles),
        ("long_lengths_ca_es_miles", "ca-ES", Units::Miles),
        ("long_lengths_hi_in", "hi-IN", Units::Kilometers),
    ] {
        let leg = LegBuilder::new("AD", "")
            .edge(road(&["A1"], 0, 1234.4))
            .edge(road(&["A2"], 90, 2.4))
            .at(|n| n.intersecting_edge = vec![xedge(0, &["A1"])])
            .edge(road(&["A3"], 0, 0.305))
            .at(|n| n.intersecting_edge = vec![xedge(90, &["A2"])])
            .edge(road(&["A4"], 90, 1.3))
            .at(|n| n.intersecting_edge = vec![xedge(0, &["A3"])])
            .edge(road(&["A5"], 0, 0.004))
            .at(|n| n.intersecting_edge = vec![xedge(90, &["A4"])])
            .edge(road(&["A6"], 270, 0.06))
            .at(|n| n.intersecting_edge = vec![xedge(0, &["A5"])])
            .finish();
        cases.push((name, api(language, units, vec![leg])));
    }

    // Turns in a few languages, including the subclassed ones.
    for (name, language) in [
        ("turns_it_it", "it-IT"),
        ("turns_cs_cz", "cs-CZ"),
        ("turns_ru_ru", "ru-RU"),
        ("turns_hi_in", "hi-IN"),
        ("turns_ja_jp", "ja-JP"),
    ] {
        cases.push((name, api(language, Units::Kilometers, vec![turns()])));
    }

    // Driving on the left, with a U-turn at the end.
    let mut left = turns();
    for node in &mut left.node {
        if let Some(edge) = &mut node.edge {
            edge.drive_on_left = true;
        }
    }
    cases.push(("drive_on_left", api("en-GB", Units::Miles, vec![left])));

    // Destination and origin descriptions.
    let named = LegBuilder::new("AD", "")
        .edge(road(&["Carrer A"], 0, 0.5))
        .edge(road(&["Carrer B"], 90, 0.5))
        .at(|n| n.intersecting_edge = vec![xedge(0, &["Carrer A"])])
        .origin(|l| l.side_of_street = SideOfStreet::KRight as i32)
        .destination(|l| {
            l.name = "Casa de la Vall".to_owned();
            l.side_of_street = SideOfStreet::KLeft as i32;
        })
        .finish();
    cases.push((
        "destination_name_left",
        api("en-US", Units::Kilometers, vec![named]),
    ));
    let street = LegBuilder::new("US", "TX")
        .edge(road(&["FM 1960"], 0, 0.5))
        .edge(road(&["I-45"], 90, 0.5))
        .at(|n| n.intersecting_edge = vec![xedge(0, &["FM 1960"])])
        .destination(|l| {
            l.street = "FM 1960".to_owned();
            l.side_of_street = SideOfStreet::KRight as i32;
        })
        .finish();
    cases.push((
        "destination_street_texas",
        api("en-US", Units::Miles, vec![street]),
    ));

    // US verbal formatting in Colorado.
    let colorado = LegBuilder::new("US", "CO")
        .edge(road(&["CO 93", "Broadway"], 0, 1.5))
        .edge(road(&["US 36"], 90, 3.0))
        .at(|n| n.intersecting_edge = vec![xedge(0, &["Broadway"])])
        .edge(road(&["CR 1500"], 0, 0.5))
        .at(|n| n.intersecting_edge = vec![xedge(90, &["US 36"])])
        .finish();
    cases.push((
        "verbal_colorado",
        api("en-US", Units::Miles, vec![colorado]),
    ));

    // A motorway exit with every kind of exit sign, then guide and junction signs.
    let motorway = |name: &[&str], heading, km| Edge {
        road_class: RoadClass::KMotorway as i32,
        traversability: Traversability::KForward as i32,
        speed: 100.0,
        default_speed: 100.0,
        lane_count: 3,
        ..road(name, heading, km)
    };
    let ramp = |heading, km, sign: Option<TripSign>| Edge {
        r#use: Use::KRampUse as i32,
        road_class: RoadClass::KMotorway as i32,
        traversability: Traversability::KForward as i32,
        sign,
        ..road(&[], heading, km)
    };
    let exit_sign = TripSign {
        exit_numbers: sign_elements(&["12A"]),
        exit_onto_streets: sign_elements(&["US 6", "Main Street"]),
        exit_toward_locations: sign_elements(&["Downtown", "Airport"]),
        exit_names: sign_elements(&["Gateway"]),
        ..TripSign::default()
    };
    let guide_sign = TripSign {
        guide_onto_streets: sign_elements(&["SR 7"]),
        guide_toward_locations: sign_elements(&["Lakeside", "Harbor"]),
        junction_names: sign_elements(&["Five Points"]),
        ..TripSign::default()
    };
    let signs = LegBuilder::new("US", "WA")
        .edge(motorway(&["I-5"], 0, 3.0))
        .edge(motorway(&["I-5"], 0, 2.0))
        .edge(ramp(20, 0.4, Some(exit_sign)))
        .at(|n| {
            n.intersecting_edge = vec![IntersectingEdge {
                r#use: Use::KRoadUse as i32,
                road_class: RoadClass::KMotorway as i32,
                driveability: Traversability::KForward as i32,
                ..xedge(0, &["I-5"])
            }];
        })
        .edge(ramp(20, 0.3, None))
        .edge(road(&["Main Street"], 90, 0.8))
        .at(|n| n.intersecting_edge = vec![xedge(270, &["Main Street"])])
        .edge(Edge {
            sign: Some(guide_sign.clone()),
            ..road(&["Elm Street"], 0, 0.5)
        })
        .at(|n| {
            n.intersecting_edge = vec![xedge(180, &["Elm Street"]), xedge(90, &["Main Street"])];
        })
        .edge(Edge {
            sign: Some(TripSign {
                junction_names: sign_elements(&["Hub Corner"]),
                ..TripSign::default()
            }),
            ..road(&["Oak Street"], 270, 0.5)
        })
        .at(|n| n.intersecting_edge = vec![xedge(0, &["Elm Street"])])
        .finish();
    cases.push((
        "exit_and_guide_signs",
        api("en-US", Units::Miles, vec![signs.clone()]),
    ));
    cases.push((
        "exit_and_guide_signs_de_de",
        api("de-DE", Units::Kilometers, vec![signs]),
    ));

    // Names and signs with pronunciations, run with phoneme markup.
    let ipa = |value: &str| {
        Some(Pronunciation {
            alphabet: pronunciation::Alphabet::KIpa as i32,
            value: value.to_owned(),
        })
    };
    let mut pronounced = turns();
    for node in &mut pronounced.node {
        if let Some(edge) = &mut node.edge {
            for name in &mut edge.name {
                name.pronunciation = ipa("kəˈrer");
            }
        }
    }
    if let Some(edge) = &mut pronounced.node[2].edge {
        edge.sign = Some(TripSign {
            guide_toward_locations: vec![TripSignElement {
                pronunciation: ipa("ˈsentre"),
                ..sign_elements(&["Centre"])[0].clone()
            }],
            ..TripSign::default()
        });
    }
    cases.push((
        "pronunciations_markup",
        api("en-US", Units::Kilometers, vec![pronounced.clone()]),
    ));
    cases.push((
        "pronunciations_plain",
        api("en-US", Units::Kilometers, vec![pronounced]),
    ));

    // A blind pedestrian passes an intersection with signals, a gate, a bollard, steps, and a bridge.
    let blind = |edge: Edge| Edge {
        pedestrian_type: PedestrianType::KBlind as i32,
        ..edge
    };
    let blind_walk = LegBuilder::new("AD", "")
        .edge(blind(footway(&["Carrer A"], 0, 0.1)))
        .edge(blind(footway(&["Carrer A"], 0, 0.1)))
        .at(|n| {
            n.traffic_signal = true;
            n.intersecting_edge = vec![xedge(90, &["Carrer X"]), xedge(270, &["Carrer Y"])];
        })
        .edge(blind(footway(&["Carrer A"], 0, 0.1)))
        .at(|n| n.r#type = node::Type::KGate as i32)
        .edge(blind(footway(&["Carrer A"], 0, 0.1)))
        .at(|n| n.r#type = node::Type::KBollard as i32)
        .edge(blind(Edge {
            r#use: Use::KStepsUse as i32,
            ..footway(&[], 0, 0.02)
        }))
        .edge(blind(Edge {
            bridge: true,
            ..footway(&[], 0, 0.1)
        }))
        .edge(blind(Edge {
            tunnel: true,
            ..footway(&[], 0, 0.1)
        }))
        .finish();
    cases.push((
        "blind_pedestrian",
        api("en-US", Units::Kilometers, vec![blind_walk]),
    ));

    // Entering and leaving a building, with an elevator, an escalator, and indoor steps between levels.
    let level = |start: f32, end: f32| vec![Level { start, end }];
    let level_ref = |value: &str| {
        vec![TaggedValue {
            value: value.as_bytes().to_vec(),
            r#type: tagged_value::Type::KLevelRef as i32,
        }]
    };
    let indoor = |edge: Edge| Edge {
        indoor: true,
        ..edge
    };
    let building = LegBuilder::new("AD", "")
        .edge(footway(&["Plaça"], 0, 0.1))
        .edge(indoor(Edge {
            tagged_value: level_ref("Ground"),
            levels: level(0.0, 0.0),
            ..footway(&[], 0, 0.05)
        }))
        .at(|n| n.r#type = node::Type::KBuildingEntrance as i32)
        .edge(indoor(Edge {
            r#use: Use::KElevatorUse as i32,
            levels: vec![Level {
                start: 0.0,
                end: 2.0,
            }],
            ..footway(&[], 0, 0.005)
        }))
        .edge(indoor(Edge {
            tagged_value: level_ref("Level 2"),
            levels: level(2.0, 2.0),
            ..footway(&[], 90, 0.05)
        }))
        .at(|n| n.r#type = node::Type::KElevator as i32)
        .edge(indoor(Edge {
            r#use: Use::KEscalatorUse as i32,
            levels: vec![Level {
                start: 2.0,
                end: 3.0,
            }],
            tagged_value: level_ref("Level 3"),
            ..footway(&[], 90, 0.02)
        }))
        .edge(indoor(Edge {
            tagged_value: level_ref("Level 3"),
            levels: level(3.0, 3.0),
            ..footway(&[], 180, 0.05)
        }))
        .edge(indoor(Edge {
            r#use: Use::KStepsUse as i32,
            levels: vec![Level {
                start: 3.0,
                end: 1.0,
            }],
            tagged_value: level_ref("Level 1"),
            ..footway(&[], 180, 0.02)
        }))
        .edge(indoor(Edge {
            tagged_value: level_ref("Level 1"),
            levels: vec![Level {
                start: 1.0,
                end: 0.0,
            }],
            ..footway(&[], 180, 0.05)
        }))
        .edge(footway(&["Carrer Major"], 180, 0.1))
        .at(|n| n.r#type = node::Type::KBuildingEntrance as i32)
        .finish();
    cases.push((
        "building_levels",
        api("en-US", Units::Kilometers, vec![building.clone()]),
    ));
    cases.push((
        "building_levels_fr_fr",
        api("fr-FR", Units::Kilometers, vec![building]),
    ));

    // Driving to a car park, then walking.
    let park = LegBuilder::new("AD", "")
        .edge(road(&["Carrer A"], 0, 0.5))
        .edge(Edge {
            r#use: Use::KParkingAisleUse as i32,
            ..road(&[], 90, 0.05)
        })
        .at(|n| n.intersecting_edge = vec![xedge(0, &["Carrer A"])])
        .edge(footway(&["Carrer B"], 90, 0.2))
        .at(|n| n.r#type = node::Type::KParking as i32)
        .finish();
    cases.push(("park_vehicle", api("en-US", Units::Kilometers, vec![park])));

    // Walking to a bike share station, cycling, and walking again.
    let bss = Some(BikeShareStationInfo {
        name: "Station".to_owned(),
        capacity: 10,
        ..BikeShareStationInfo::default()
    });
    let cycleway = |name: &[&str], heading, km| Edge {
        r#use: Use::KCyclewayUse as i32,
        travel_mode: TravelMode::KBicycle as i32,
        speed: 18.0,
        default_speed: 18.0,
        ..road(name, heading, km)
    };
    let share = LegBuilder::new("AD", "")
        .edge(footway(&["Carrer A"], 0, 0.2))
        .edge(cycleway(&["Carril Bici"], 0, 1.2))
        .at(|n| {
            n.r#type = node::Type::KBikeShare as i32;
            n.bss_info.clone_from(&bss);
        })
        .edge(cycleway(&[], 90, 0.6))
        .at(|n| n.intersecting_edge = vec![xedge(0, &["Carril Bici"])])
        .edge(footway(&["Carrer B"], 90, 0.2))
        .at(|n| {
            n.r#type = node::Type::KBikeShare as i32;
            n.bss_info.clone_from(&bss);
        })
        .finish();
    cases.push(("bike_share", api("en-US", Units::Kilometers, vec![share])));

    // Roundabouts: more exits than there are ordinals, and a named roundabout, with combined exits.
    let roundabout = |km| Edge {
        roundabout: true,
        traversability: Traversability::KForward as i32,
        ..road(&["Rotonda"], 90, km)
    };
    let mut many_exits = LegBuilder::new("AD", "")
        .edge(road(&["Carrer A"], 0, 0.5))
        .edge(roundabout(0.02));
    for _ in 0..11 {
        many_exits = many_exits
            .edge(roundabout(0.02))
            .at(|n| n.intersecting_edge = vec![xedge(180, &["Sortida"])]);
    }
    let many_exits = many_exits
        .edge(road(&["Carrer B"], 180, 0.4))
        .at(|n| n.intersecting_edge = vec![xedge(90, &["Rotonda"])])
        .finish();
    cases.push((
        "roundabout_many_exits",
        api("en-US", Units::Kilometers, vec![many_exits.clone()]),
    ));
    let mut combined = api("es-ES", Units::Kilometers, vec![many_exits]);
    if let Some(options) = &mut combined.options {
        options.has_roundabout_exits = Some(HasRoundaboutExits::RoundaboutExits(false));
    }
    cases.push(("roundabout_combined_exits_es_es", combined));

    // Turn lanes on the way to a left turn, and a zero-length edge without headings.
    let lanes = |masks: &[u32]| {
        masks
            .iter()
            .map(|m| TurnLane {
                directions_mask: *m,
                ..TurnLane::default()
            })
            .collect()
    };
    let lane_leg = LegBuilder::new("AD", "")
        .edge(Edge {
            turn_lanes: lanes(&[8, 2, 2, 64]),
            lane_count: 4,
            ..road(&["Avinguda"], 0, 0.3)
        })
        .edge(Edge {
            turn_lanes: lanes(&[8 | 2, 2, 64]),
            lane_count: 3,
            ..road(&["Avinguda"], 0, 0.2)
        })
        .at(|n| n.intersecting_edge = vec![xedge(90, &["Carrer X"])])
        .edge(Edge {
            begin_heading: 0,
            end_heading: 0,
            ..road(&["Carrer Y"], 0, 0.0005)
        })
        .edge(road(&["Carrer Y"], 270, 0.3))
        .at(|n| n.intersecting_edge = vec![xedge(0, &["Avinguda"]), xedge(90, &["Carrer Z"])])
        .finish();
    cases.push((
        "turn_lanes",
        api("en-US", Units::Kilometers, vec![lane_leg]),
    ));

    // Errors: a leg with one node, one location, no nodes, and an unknown language.
    let one_node = LegBuilder::new("AD", "").finish();
    cases.push((
        "error_one_node",
        api("en-US", Units::Kilometers, vec![one_node]),
    ));
    let mut one_location = turns();
    one_location.location.truncate(1);
    cases.push((
        "error_one_location",
        api("en-US", Units::Kilometers, vec![one_location]),
    ));
    cases.push((
        "error_no_nodes",
        api("en-US", Units::Kilometers, vec![TripLeg::default()]),
    ));
    cases.push((
        "error_unknown_language",
        api("xx-XX", Units::Kilometers, vec![turns()]),
    ));

    cases
}

/// Writes the synthetic inputs (run on demand; see the module docs).
#[test]
#[ignore = "writes fixtures; run by tools/oracle/odin-synthetic.sh"]
fn write_synthetic_inputs() {
    let dir = Path::new(SYNTHETIC_DIR);
    if dir.exists() {
        fs::remove_dir_all(dir).expect("remove old synthetic cases");
    }
    for (name, api) in cases() {
        let case = dir.join(name);
        fs::create_dir_all(&case).expect("create case directory");
        fs::write(case.join("thor-odin.bin"), api.encode_to_vec()).expect("write input");
    }
}

/// The written inputs are the current cases (so the references are for these trips).
#[test]
fn synthetic_inputs_are_current() {
    for (name, api) in cases() {
        let path = Path::new(SYNTHETIC_DIR).join(name).join("thor-odin.bin");
        let written = fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            written,
            api.encode_to_vec(),
            "{name} is stale; rerun tools/oracle/odin-synthetic.sh"
        );
    }
}
