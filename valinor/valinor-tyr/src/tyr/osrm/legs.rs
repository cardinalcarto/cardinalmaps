//! A route's `legs` and their `steps` (`serialize_legs` in mainline's `route_serializer_osrm.cc`).

use super::annotations::{add_closures, add_incidents, annotations, via_waypoints};
use super::banner::{BannerInput, banner_instructions};
use super::countries::{iso_3166_1_alpha3, speed_limit_info};
use super::geo::units_to_meters;
use super::geometry::{decode_shape, maneuver_geometry};
use super::intersections::{Attributes, drive_on_right, intersections};
use super::json::{Json, Map};
use super::maneuvers::{
    ManeuverContext, maneuver_type, mode, names_and_refs, osrm_maneuver, pronunciations,
    turn_modifier,
};
use super::signs::{destinations, exits, sign_elements};
use super::voice::{VoiceContext, voice_instructions};
use crate::tyr::polyline::LngLat;
use crate::tyr::trip::{curr_edge, prev_edge};
use crate::tyr::unknown;
use valhalla_proto::directions_leg::Maneuver;
use valhalla_proto::directions_leg::guidance_view::Type as GuidanceViewType;
use valhalla_proto::directions_leg::maneuver::Type as ManeuverType;
use valhalla_proto::trip_leg::{Cost, Node};
use valhalla_proto::{DirectionsLeg, DirectionsType, Options, ShapeFormat, TripLeg, TripSign};
use valhalla_response::{ErrorCode, ValhallaError};

/// What every leg of a route is serialized with.
#[derive(Debug, Clone, Copy)]
pub(super) struct LegOptions<'a> {
    pub(super) options: &'a Options,
    pub(super) imperial: bool,
    pub(super) shape_format: ShapeFormat,
    pub(super) attributes: Attributes,
    /// `None` without voice instructions.
    pub(super) voice: Option<VoiceContext<'a>>,
}

fn elapsed(node: &Node) -> Cost {
    node.cost
        .unwrap_or_default()
        .elapsed_cost
        .unwrap_or_default()
}

/// `GuidanceViewTypeToString`.
fn guidance_view_type(kind: i32) -> &'static str {
    match GuidanceViewType::try_from(kind) {
        Ok(GuidanceViewType::KJunction) => "jct",
        Ok(GuidanceViewType::KSapa) => "sapa",
        Ok(GuidanceViewType::KTollbranch) => "tollbranch",
        Ok(GuidanceViewType::KAftertoll) => "aftertoll",
        Ok(GuidanceViewType::KEnt) => "ent",
        Ok(GuidanceViewType::KExit) => "exit",
        Ok(GuidanceViewType::KCityreal) => "cityreal",
        Ok(GuidanceViewType::KDirectionboard) => "directionboard",
        Ok(GuidanceViewType::KSignboard) => "signboard",
        Err(_) => "",
    }
}

/// Adds `duration_<name>` and `weight_<name>` for each recosting
/// (with `begin` subtracted, for steps), or nulls where a recosting failed.
fn add_recosts(out: &mut Map, options: &Options, end: &Node, begin: Option<&Node>) {
    for (i, end_recost) in end.recosts.iter().enumerate() {
        let name = options
            .recostings
            .get(i)
            .map(costing_name)
            .unwrap_or_default();
        if let Some(end_cost) = end_recost.elapsed_cost {
            let begin_cost = begin
                .and_then(|node| node.recosts.get(i))
                .and_then(|recost| recost.elapsed_cost)
                .unwrap_or_default();
            out.emplace(
                format!("duration_{name}"),
                Json::Fixed(end_cost.seconds - begin_cost.seconds, 3),
            );
            out.emplace(
                format!("weight_{name}"),
                Json::Fixed(end_cost.cost - begin_cost.cost, 3),
            );
        } else {
            out.emplace(format!("duration_{name}"), Json::Null);
            out.emplace(format!("weight_{name}"), Json::Null);
        }
    }
}

/// A costing's `name`, or `""` if it has none.
pub(super) fn costing_name(costing: &valhalla_proto::Costing) -> &str {
    match &costing.has_name {
        Some(valhalla_proto::costing::HasName::Name(name)) => name,
        None => "",
    }
}

/// `serialize_legs`: the legs of one route.
pub(super) fn serialize_legs(
    legs: &[DirectionsLeg],
    leg_summaries: &[String],
    path_legs: &[TripLeg],
    leg_options: &LegOptions<'_>,
) -> Result<Vec<Json>, ValhallaError> {
    if legs.len() != path_legs.len() {
        return Err(ErrorCode::LegCountMismatch.into());
    }
    legs.iter()
        .zip(path_legs)
        .zip(
            leg_summaries
                .iter()
                .map(String::as_str)
                .chain(core::iter::repeat("")),
        )
        .map(|((leg, path_leg), summary)| serialize_leg(leg, summary, path_leg, leg_options))
        .collect()
}

/// The step before the current one, which gets the banners and announcements
/// leading up to the current maneuver.
#[derive(Debug)]
struct PrevStep<'a> {
    step: Map,
    maneuver: &'a Maneuver,
    distance: f64,
}

/// `guidance_views` of a maneuver.
fn guidance_views(maneuver: &Maneuver) -> Vec<Json> {
    maneuver
        .guidance_views
        .iter()
        .map(|view| {
            let mut out = Map::new();
            out.emplace("data_id", view.data_id.as_str());
            out.emplace("type", guidance_view_type(view.r#type));
            out.emplace("base_id", view.base_id.as_str());
            let overlays: Vec<Json> = view
                .overlay_ids
                .iter()
                .map(|id| Json::str(id.as_str()))
                .collect();
            out.emplace("overlay_ids", overlays);
            out.into()
        })
        .collect()
}

/// The leg's `admins`: country codes, with alpha-3 codes mainline knows.
fn admins(path_leg: &TripLeg) -> Vec<Json> {
    path_leg
        .admin
        .iter()
        .map(|admin| {
            let mut out = Map::new();
            if !admin.country_code.is_empty() {
                out.emplace("iso_3166_1", admin.country_code.as_str());
                if let Some(alpha3) = iso_3166_1_alpha3(&admin.country_code) {
                    out.emplace("iso_3166_1_alpha3", alpha3);
                }
            }
            out.into()
        })
        .collect()
}

/// `speedLimitSign` and `speedLimitUnit` for the step's country,
/// when the request asked for speed limits.
fn add_speed_limit_sign(
    step: &mut Map,
    path_leg: &TripLeg,
    begin_node: &Node,
) -> Result<(), ValhallaError> {
    if path_leg
        .shape_attributes
        .as_ref()
        .is_some_and(|attributes| !attributes.speed_limit.is_empty())
    {
        let country = path_leg
            .admin
            .get(begin_node.admin_index as usize)
            .ok_or_else(|| unknown("node admin index past the leg's admins"))?;
        let (sign, unit) = speed_limit_info(&country.country_code);
        step.emplace("speedLimitSign", sign);
        step.emplace("speedLimitUnit", unit);
    }
    Ok(())
}

/// What one maneuver's step says about it, which banners repeat on the previous step.
#[derive(Debug)]
struct StepFacts<'a> {
    maneuver_index: usize,
    maneuver: &'a Maneuver,
    arrive_maneuver: bool,
    osrm_type: &'static str,
    modifier: &'static str,
    destinations: String,
    exit: String,
}

/// The state `serialize_legs` carries from step to step.
#[derive(Debug)]
struct LegState<'a> {
    path_leg: &'a TripLeg,
    leg_options: &'a LegOptions<'a>,
    shape: Vec<LngLat>,
    prev_intersection_count: u32,
    drive_side: &'static str,
    name: String,
    refs: String,
    pronunciation: String,
    mode: &'static str,
    prev_mode: &'static str,
    prev_rotary: bool,
    steps: Vec<Json>,
    prev: Option<PrevStep<'a>>,
}

impl<'a> LegState<'a> {
    /// Adds the step of the `maneuver_index`th maneuver (of `maneuver_count`).
    fn step(
        &mut self,
        maneuver_index: usize,
        maneuver: &'a Maneuver,
        maneuver_count: usize,
    ) -> Result<(), ValhallaError> {
        let path_leg = self.path_leg;
        let options = self.leg_options.options;
        let mut step = Map::new();
        let depart_maneuver = maneuver_index == 0;
        let arrive_maneuver = maneuver_index + 1 == maneuver_count;

        step.emplace(
            "geometry",
            maneuver_geometry(
                maneuver.begin_shape_index,
                maneuver.end_shape_index,
                &self.shape,
                arrive_maneuver,
                self.leg_options.shape_format,
            )?,
        );
        let distance = f64::from(units_to_meters(maneuver.length, !self.leg_options.imperial));

        // The arrive step keeps the previous step's side, names, and mode.
        if !arrive_maneuver {
            let edge = curr_edge(path_leg, maneuver.begin_path_index as usize)
                .ok_or_else(|| unknown("maneuver without an outgoing edge"))?;
            self.drive_side = if drive_on_right(edge) {
                "right"
            } else {
                "left"
            };
            (self.name, self.refs) = names_and_refs(maneuver);
            self.pronunciation = pronunciations(maneuver);
            self.mode = mode(maneuver, arrive_maneuver, path_leg)?;
            if self.prev_mode.is_empty() {
                self.prev_mode = self.mode;
            }
        }

        step.emplace("mode", self.mode);
        step.emplace("driving_side", self.drive_side);
        step.emplace("distance", Json::Fixed(distance, 3));
        step.emplace("duration", Json::Fixed(maneuver.time, 3));
        let node = |index: u32| {
            path_leg
                .node
                .get(index as usize)
                .ok_or_else(|| unknown("maneuver path index past the leg"))
        };
        let end_node = node(maneuver.end_path_index)?;
        let begin_node = node(maneuver.begin_path_index)?;
        step.emplace(
            "weight",
            Json::Fixed(elapsed(end_node).cost - elapsed(begin_node).cost, 3),
        );
        add_recosts(&mut step, options, end_node, Some(begin_node));

        step.emplace("name", self.name.as_str());
        if !self.refs.is_empty() {
            step.emplace("ref", self.refs.as_str());
        }
        if !self.pronunciation.is_empty() {
            step.emplace("pronunciation", self.pronunciation.as_str());
        }
        add_speed_limit_sign(&mut step, path_leg, begin_node)?;

        let rotary = maneuver.r#type == ManeuverType::KRoundaboutEnter as i32
            && !maneuver.street_name.is_empty();
        if rotary && let Some(street) = maneuver.street_name.first() {
            step.emplace("rotary_name", street.value.as_str());
        }

        let facts =
            self.add_maneuver(&mut step, maneuver_index, maneuver, arrive_maneuver, rotary)?;
        self.add_guidance(&mut step, &facts)?;

        let sign = maneuver.sign.as_ref().unwrap_or(&EMPTY_SIGN);
        let junction_name = sign_elements(&sign.junction_names);
        if !depart_maneuver && !junction_name.is_empty() {
            step.emplace("junction_name", junction_name);
        }
        if options.guidance_views && !depart_maneuver && !maneuver.guidance_views.is_empty() {
            step.emplace("guidance_views", guidance_views(maneuver));
        }

        let (step_intersections, count) = intersections(
            maneuver,
            path_leg,
            &self.shape,
            arrive_maneuver,
            self.leg_options.attributes,
        )?;
        self.prev_intersection_count = count;
        step.emplace("intersections", step_intersections);

        self.prev_rotary = rotary;
        self.prev_mode = self.mode;
        if let Some(done) = self.prev.replace(PrevStep {
            step,
            maneuver,
            distance,
        }) {
            self.steps.push(done.step.into());
        }
        Ok(())
    }

    /// The step's `maneuver`, `destinations`, and `exits`.
    fn add_maneuver(
        &mut self,
        step: &mut Map,
        maneuver_index: usize,
        maneuver: &'a Maneuver,
        arrive_maneuver: bool,
        rotary: bool,
    ) -> Result<StepFacts<'a>, ValhallaError> {
        let depart_maneuver = maneuver_index == 0;
        let index = maneuver.begin_path_index as usize;
        let in_bearing = if index > 0 {
            prev_edge(self.path_leg, index)
                .ok_or_else(|| unknown("maneuver without an incoming edge"))?
                .end_heading
        } else {
            0
        };
        let out_bearing = maneuver.begin_heading;
        let modifier = if depart_maneuver {
            ""
        } else {
            turn_modifier(maneuver, in_bearing, out_bearing, arrive_maneuver)
        };
        let osrm_type = maneuver_type(
            maneuver,
            self.path_leg,
            &ManeuverContext {
                depart_maneuver,
                arrive_maneuver,
                modifier,
                prev_intersection_count: self.prev_intersection_count,
                mode: self.mode,
                prev_mode: self.prev_mode,
                rotary,
                prev_rotary: self.prev_rotary,
            },
        )?;
        let location = *self
            .shape
            .get(maneuver.begin_shape_index as usize)
            .ok_or_else(|| unknown("maneuver shape index past the leg's shape"))?;
        step.emplace(
            "maneuver",
            osrm_maneuver(
                maneuver,
                osrm_type,
                modifier,
                in_bearing,
                out_bearing,
                location,
                self.leg_options.options.directions_type == DirectionsType::Instructions as i32,
            ),
        );

        let sign = maneuver.sign.as_ref().unwrap_or(&EMPTY_SIGN);
        let dest = destinations(sign);
        if !dest.is_empty() {
            step.emplace("destinations", dest.as_str());
            // A roundabout exit's destinations also go on the roundabout's entry step.
            if maneuver.r#type == ManeuverType::KRoundaboutExit as i32
                && let Some(prev) = self.prev.as_mut()
                && prev.maneuver.r#type == ManeuverType::KRoundaboutEnter as i32
            {
                prev.step.emplace("destinations", dest.as_str());
            }
        }
        let exit = exits(sign);
        if !exit.is_empty() {
            step.emplace("exits", exit.as_str());
        }
        Ok(StepFacts {
            maneuver_index,
            maneuver,
            arrive_maneuver,
            osrm_type,
            modifier,
            destinations: dest,
            exit,
        })
    }

    /// Banners and announcements for the maneuver go on the previous step;
    /// the arrive step gets empty ones.
    fn add_guidance(&mut self, step: &mut Map, facts: &StepFacts<'a>) -> Result<(), ValhallaError> {
        if self.leg_options.options.banner_instructions {
            if let Some(prev) = self.prev.as_mut() {
                let banners = banner_instructions(
                    &BannerInput {
                        name: &self.name,
                        destinations: &facts.destinations,
                        refs: &self.refs,
                        prev_maneuver: prev.maneuver,
                        maneuver: facts.maneuver,
                        arrive_maneuver: facts.arrive_maneuver,
                        maneuver_type: facts.osrm_type,
                        modifier: facts.modifier,
                        exit: &facts.exit,
                        distance: prev.distance,
                        drive_side: self.drive_side,
                    },
                    self.path_leg,
                )?;
                prev.step.emplace("bannerInstructions", banners);
            }
            if facts.arrive_maneuver {
                step.emplace("bannerInstructions", Vec::<Json>::new());
            }
        }
        if let Some(voice) = self.leg_options.voice {
            if let Some(prev) = self.prev.as_mut() {
                let announcements = voice_instructions(
                    prev.maneuver,
                    facts.maneuver,
                    prev.distance,
                    facts.maneuver_index,
                    self.path_leg,
                    voice,
                )?;
                prev.step.emplace("voiceInstructions", announcements);
            }
            if facts.arrive_maneuver {
                step.emplace("voiceInstructions", Vec::<Json>::new());
            }
        }
        Ok(())
    }
}

/// A missing sign reads as an empty one.
static EMPTY_SIGN: TripSign = TripSign {
    exit_numbers: Vec::new(),
    exit_onto_streets: Vec::new(),
    exit_toward_locations: Vec::new(),
    exit_names: Vec::new(),
    guide_onto_streets: Vec::new(),
    guide_toward_locations: Vec::new(),
    junction_names: Vec::new(),
    guidance_view_junctions: Vec::new(),
    guidance_view_signboards: Vec::new(),
};

fn serialize_leg(
    leg: &DirectionsLeg,
    summary: &str,
    path_leg: &TripLeg,
    leg_options: &LegOptions<'_>,
) -> Result<Json, ValhallaError> {
    let mut state = LegState {
        path_leg,
        leg_options,
        shape: decode_shape(&leg.shape)?,
        prev_intersection_count: 0,
        drive_side: "right",
        name: String::new(),
        refs: String::new(),
        pronunciation: String::new(),
        mode: "",
        prev_mode: "",
        prev_rotary: false,
        steps: Vec::new(),
        prev: None,
    };
    for (maneuver_index, maneuver) in leg.maneuver.iter().enumerate() {
        state.step(maneuver_index, maneuver, leg.maneuver.len())?;
    }
    let mut steps = state.steps;
    if let Some(done) = state.prev {
        steps.push(done.step.into());
    }

    let mut output_leg = Map::new();
    let leg_summary = leg.summary.unwrap_or_default();
    output_leg.emplace("summary", summary);
    output_leg.emplace(
        "distance",
        Json::Fixed(
            f64::from(units_to_meters(leg_summary.length, !leg_options.imperial)),
            3,
        ),
    );
    output_leg.emplace("duration", Json::Fixed(leg_summary.time, 3));
    let last = path_leg
        .node
        .last()
        .ok_or_else(|| unknown("trip leg without nodes"))?;
    output_leg.emplace("weight", Json::Fixed(elapsed(last).cost, 3));
    add_recosts(&mut output_leg, leg_options.options, last, None);
    output_leg.emplace("admins", admins(path_leg));
    output_leg.emplace("steps", steps);
    if let Some(attributes) = &path_leg.shape_attributes {
        output_leg.emplace("annotation", annotations(attributes));
    }
    output_leg.emplace("via_waypoints", via_waypoints(path_leg));
    add_incidents(path_leg, &mut output_leg)?;
    add_closures(path_leg, &mut output_leg);
    Ok(output_leg.into())
}
