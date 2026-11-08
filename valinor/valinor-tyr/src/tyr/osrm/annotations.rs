//! The optional parts of an OSRM leg: `annotation` (`serialize_annotations`),
//! `incidents` (`serializeIncidents` and `osrm::serializeIncidentProperties`),
//! `closures` (`serializeClosures`), and `via_waypoints` (`osrm::intermediate_waypoints`).

use super::json::{Json, Map};
use crate::tyr::unknown;
use crate::tyr::writer::JsonWriter;
use valhalla_proto::incidents_tile::Metadata;
use valhalla_proto::incidents_tile::metadata::{Impact, Type as IncidentType, lat_lng};
use valhalla_proto::trip_leg::{Incident, ShapeAttributes, closure};
use valhalla_proto::{Location, TripLeg, location};
use valhalla_response::ValhallaError;

/// `midgard::kSecPerMillisecond`.
const SEC_PER_MILLISECOND: f64 = 0.001;
/// `midgard::kMeterPerDecimeter`.
const METER_PER_DECIMETER: f64 = 0.1;
/// `baldr::kUnlimitedSpeedLimit`.
const UNLIMITED_SPEED_LIMIT: u32 = 255;
/// The only unit speed limit annotations use (mainline has a TODO for mph).
const SPEED_LIMIT_UNITS_KPH: &str = "km/h";

/// `serialize_annotations`: per-shape-segment durations, distances, and so on.
pub(super) fn annotations(attributes: &ShapeAttributes) -> Map {
    let mut out = Map::new();
    let scaled = |values: &[u32], factor: f64, precision: usize| -> Vec<Json> {
        values
            .iter()
            .map(|&value| Json::Fixed(f64::from(value) * factor, precision))
            .collect()
    };
    if !attributes.time.is_empty() {
        out.emplace("duration", scaled(&attributes.time, SEC_PER_MILLISECOND, 3));
    }
    if !attributes.length.is_empty() {
        out.emplace(
            "distance",
            scaled(&attributes.length, METER_PER_DECIMETER, 1),
        );
    }
    if !attributes.congestion.is_empty() {
        let congestion: Vec<Json> = attributes
            .congestion
            .iter()
            .map(|&c| Json::from(c))
            .collect();
        out.emplace("congestion", congestion);
    }
    if !attributes.speed.is_empty() {
        out.emplace("speed", scaled(&attributes.speed, METER_PER_DECIMETER, 1));
    }
    if !attributes.speed_limit.is_empty() {
        let limits: Vec<Json> = attributes
            .speed_limit
            .iter()
            .map(|&limit| {
                let mut annotation = Map::new();
                if limit == UNLIMITED_SPEED_LIMIT {
                    annotation.emplace("none", true);
                } else if limit > 0 {
                    annotation.emplace("unit", SPEED_LIMIT_UNITS_KPH);
                    annotation.emplace("speed", limit);
                } else {
                    annotation.emplace("unknown", true);
                }
                annotation.into()
            })
            .collect();
        out.emplace("maxspeed", limits);
    }
    out
}

/// `incidentTypeToString`.
fn incident_type(kind: i32) -> Result<&'static str, ValhallaError> {
    Ok(match IncidentType::try_from(kind) {
        Ok(IncidentType::Accident) => "accident",
        Ok(IncidentType::Congestion) => "congestion",
        Ok(IncidentType::Construction) => "construction",
        Ok(IncidentType::DisabledVehicle) => "disabled_vehicle",
        Ok(IncidentType::LaneRestriction) => "lane_restriction",
        Ok(IncidentType::MassTransit) => "mass_transit",
        Ok(IncidentType::Miscellaneous) => "miscellaneous",
        Ok(IncidentType::OtherNews) => "other_news",
        Ok(IncidentType::PlannedEvent) => "planned_event",
        Ok(IncidentType::RoadClosure) => "road_closure",
        Ok(IncidentType::RoadHazard) => "road_hazard",
        Ok(IncidentType::Weather) => "weather",
        Err(_) => {
            return Err(unknown(format!(
                "Unhandled case in incidentTypeToString: {kind}"
            )));
        }
    })
}

/// `incidentImpactToString`.
fn incident_impact(impact: i32) -> &'static str {
    match Impact::try_from(impact) {
        Ok(Impact::Unknown) => "unknown",
        Ok(Impact::Critical) => "critical",
        Ok(Impact::Major) => "major",
        Ok(Impact::Minor) => "minor",
        Ok(Impact::Low) => "low",
        Err(_) => "UNHANDLED_CASE",
    }
}

/// `DateTime::seconds_to_date_utc`: `YYYY-MM-DDTHH:MM:SSZ`.
fn seconds_to_date_utc(seconds: u64) -> String {
    let days = seconds / 86_400;
    let rem = seconds % 86_400;
    let (hour, minute, second) = (rem / 3600, rem / 60 % 60, rem % 60);
    // Howard Hinnant's civil_from_days, for days since 1970-01-01.
    let z = days.cast_signed() + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `osrm::serializeIncidentProperties` (with rapidjson's default number formatting).
fn incident_properties(
    writer: &mut JsonWriter,
    metadata: &Metadata,
    begin_shape_index: i64,
    end_shape_index: i64,
) -> Result<(), ValhallaError> {
    writer.key("id").string(&metadata.id.to_string());
    writer.key("type").string(incident_type(metadata.r#type)?);
    if !metadata.iso_3166_1_alpha2.is_empty() {
        writer
            .key("iso_3166_1_alpha2")
            .string(&metadata.iso_3166_1_alpha2);
    }
    if !metadata.iso_3166_1_alpha3.is_empty() {
        writer
            .key("iso_3166_1_alpha3")
            .string(&metadata.iso_3166_1_alpha3);
    }
    if !metadata.description.is_empty() {
        writer.key("description").string(&metadata.description);
    }
    if !metadata.long_description.is_empty() {
        writer
            .key("long_description")
            .string(&metadata.long_description);
    }
    if metadata.creation_time != 0 {
        writer
            .key("creation_time")
            .string(&seconds_to_date_utc(metadata.creation_time));
    }
    if metadata.start_time > 0 {
        writer
            .key("start_time")
            .string(&seconds_to_date_utc(metadata.start_time));
    }
    if metadata.end_time != 0 {
        writer
            .key("end_time")
            .string(&seconds_to_date_utc(metadata.end_time));
    }
    if metadata.impact != 0 {
        writer
            .key("impact")
            .string(incident_impact(metadata.impact));
    }
    if !metadata.sub_type.is_empty() {
        writer.key("sub_type").string(&metadata.sub_type);
    }
    if !metadata.sub_type_description.is_empty() {
        writer
            .key("sub_type_description")
            .string(&metadata.sub_type_description);
    }
    if !metadata.alertc_codes.is_empty() {
        writer.key("alertc_codes").start_array();
        for &code in &metadata.alertc_codes {
            writer.u64(u64::from(code));
        }
        writer.end_array();
    }
    writer.key("lanes_blocked").start_array();
    for lane in &metadata.lanes_blocked {
        writer.string(lane);
    }
    writer.end_array();
    if metadata.num_lanes_blocked != 0 {
        writer
            .key("num_lanes_blocked")
            .u64(metadata.num_lanes_blocked);
    }
    if !metadata.clear_lanes.is_empty() {
        writer.key("clear_lanes").string(&metadata.clear_lanes);
    }
    if metadata.length > 0 {
        writer.key("length").u64(u64::from(metadata.length));
    }
    if metadata.road_closed {
        writer.key("closed").bool(true);
    }
    if let Some(congestion) = &metadata.congestion {
        writer.key("congestion").start_object();
        writer.key("value").u64(u64::from(congestion.value));
        writer.end_object();
    }
    if begin_shape_index >= 0 {
        writer.key("geometry_index_start").i64(begin_shape_index);
    }
    if end_shape_index >= 0 {
        writer.key("geometry_index_end").i64(end_shape_index);
    }
    if let Some(display) = &metadata.display_ll
        && let (Some(lat_lng::HasLat::Lat(lat)), Some(lat_lng::HasLng::Lng(lng))) =
            (&display.has_lat, &display.has_lng)
    {
        writer.key("display_ll").start_object();
        writer.key("lat").double(*lat);
        writer.key("lon").double(*lng);
        writer.end_object();
    }
    Ok(())
}

/// `serializeIncident`: one incident, as raw JSON written by rapidjson.
fn incident(incident: &Incident) -> Result<Json, ValhallaError> {
    let mut writer = JsonWriter::new();
    writer.start_object();
    incident_properties(
        &mut writer,
        &incident.metadata.clone().unwrap_or_default(),
        // Mainline passes the indexes as `int`.
        i64::from(incident.begin_shape_index.cast_signed()),
        i64::from(incident.end_shape_index.cast_signed()),
    )?;
    writer.end_object();
    Ok(Json::Raw(writer.finish()))
}

/// `serializeIncidents`: adds the leg's incidents, if it has any.
pub(super) fn add_incidents(leg: &TripLeg, output_leg: &mut Map) -> Result<(), ValhallaError> {
    if leg.incidents.is_empty() {
        return Ok(());
    }
    let incidents = leg
        .incidents
        .iter()
        .map(incident)
        .collect::<Result<Vec<_>, _>>()?;
    output_leg.emplace("incidents", incidents);
    Ok(())
}

/// `serializeClosures`: adds the leg's closures, if it has any.
pub(super) fn add_closures(leg: &TripLeg, output_leg: &mut Map) {
    if leg.closures.is_empty() {
        return;
    }
    let closures: Vec<Json> = leg
        .closures
        .iter()
        .map(|closure| {
            let begin = match closure.has_begin_shape_index {
                Some(closure::HasBeginShapeIndex::BeginShapeIndex(index)) => index,
                None => 0,
            };
            let end = match closure.has_end_shape_index {
                Some(closure::HasEndShapeIndex::EndShapeIndex(index)) => index,
                None => 0,
            };
            let mut out = Map::new();
            out.emplace("geometry_index_start", begin);
            out.emplace("geometry_index_end", end);
            out.into()
        })
        .collect();
    output_leg.emplace("closures", closures);
}

/// `osrm::intermediate_waypoints`: the leg's via and through locations.
pub(super) fn via_waypoints(leg: &TripLeg) -> Vec<Json> {
    leg.location
        .iter()
        .filter(|loc| {
            loc.r#type == location::Type::KVia as i32
                || loc.r#type == location::Type::KThrough as i32
        })
        .map(|loc: &Location| {
            let correlation = loc.correlation.clone().unwrap_or_default();
            let mut out = Map::new();
            out.emplace("geometry_index", u64::from(correlation.leg_shape_index));
            out.emplace(
                "distance_from_start",
                Json::Fixed(correlation.distance_from_leg_origin, 3),
            );
            out.emplace("waypoint_index", u64::from(correlation.original_index));
            out.into()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use valhalla_proto::incidents_tile::metadata::{Congestion, LatLng};
    use valhalla_proto::trip_leg::Closure;

    /// mainline's `testserializeAnnotations` and `testserializeAnnotationsSpeedLimits`.
    #[test]
    fn annotation_values() {
        let attributes = ShapeAttributes {
            time: vec![1000, 1500],
            length: vec![25],
            speed: vec![139],
            congestion: vec![3],
            speed_limit: vec![255, 50, 0],
        };
        assert_eq!(
            annotations(&attributes).to_json(),
            concat!(
                r#"{"duration":[1.000,1.500],"distance":[2.5],"congestion":[3],"speed":[13.9],"#,
                r#""maxspeed":[{"none":true},{"unit":"km\/h","speed":50},{"unknown":true}]}"#
            )
        );
        assert_eq!(annotations(&ShapeAttributes::default()).to_json(), "{}");
    }

    #[test]
    fn dates() {
        assert_eq!(seconds_to_date_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(seconds_to_date_utc(1_597_241_829), "2020-08-12T14:17:09Z");
        assert_eq!(seconds_to_date_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    /// Modeled on mainline's `testserializeIncidents`.
    #[test]
    fn incident_json() {
        let leg = TripLeg {
            incidents: vec![Incident {
                metadata: Some(Metadata {
                    id: 1337,
                    r#type: IncidentType::Weather as i32,
                    iso_3166_1_alpha2: "SE".into(),
                    iso_3166_1_alpha3: "SWE".into(),
                    description: "Fooo".into(),
                    long_description: "Long Fooo".into(),
                    creation_time: 1_597_241_829,
                    start_time: 1_597_241_819,
                    end_time: 1_597_241_839,
                    impact: Impact::Critical as i32,
                    sub_type: "foo".into(),
                    sub_type_description: "foobar".into(),
                    alertc_codes: vec![11],
                    lanes_blocked: vec!["right".into()],
                    num_lanes_blocked: 2,
                    clear_lanes: "many lanes clear".into(),
                    length: 2,
                    road_closed: true,
                    congestion: Some(Congestion { value: 33 }),
                    display_ll: Some(LatLng {
                        has_lat: Some(lat_lng::HasLat::Lat(1.5)),
                        has_lng: Some(lat_lng::HasLng::Lng(2.25)),
                    }),
                }),
                begin_shape_index: 42,
                end_shape_index: 42,
            }],
            closures: vec![Closure {
                has_begin_shape_index: Some(closure::HasBeginShapeIndex::BeginShapeIndex(3)),
                has_end_shape_index: None,
            }],
            ..TripLeg::default()
        };
        let mut out = Map::new();
        add_incidents(&leg, &mut out).expect("serializes");
        add_closures(&leg, &mut out);
        assert_eq!(
            out.to_json(),
            concat!(
                r#"{"incidents":[{"id":"1337","type":"weather","iso_3166_1_alpha2":"SE","#,
                r#""iso_3166_1_alpha3":"SWE","description":"Fooo","long_description":"Long Fooo","#,
                r#""creation_time":"2020-08-12T14:17:09Z","start_time":"2020-08-12T14:16:59Z","#,
                r#""end_time":"2020-08-12T14:17:19Z","impact":"critical","sub_type":"foo","#,
                r#""sub_type_description":"foobar","alertc_codes":[11],"lanes_blocked":["right"],"#,
                r#""num_lanes_blocked":2,"clear_lanes":"many lanes clear","length":2,"closed":true,"#,
                r#""congestion":{"value":33},"geometry_index_start":42,"geometry_index_end":42,"#,
                r#""display_ll":{"lat":1.5,"lon":2.25}}],"#,
                r#""closures":[{"geometry_index_start":3,"geometry_index_end":0}]}"#
            )
        );

        let mut none = Map::new();
        add_incidents(&TripLeg::default(), &mut none).expect("serializes");
        add_closures(&TripLeg::default(), &mut none);
        assert!(none.is_empty());

        let bad = TripLeg {
            incidents: vec![Incident {
                metadata: Some(Metadata {
                    r#type: 99,
                    ..Metadata::default()
                }),
                ..Incident::default()
            }],
            ..TripLeg::default()
        };
        assert!(add_incidents(&bad, &mut Map::new()).is_err());
    }

    #[test]
    fn via_and_through_waypoints() {
        let location = |kind: location::Type, index: u32| Location {
            r#type: kind as i32,
            correlation: Some(valhalla_proto::Correlation {
                leg_shape_index: index * 10,
                distance_from_leg_origin: 123.456_7,
                original_index: index,
                ..Default::default()
            }),
            ..Location::default()
        };
        let leg = TripLeg {
            location: vec![
                location(location::Type::KBreak, 0),
                location(location::Type::KThrough, 1),
                location(location::Type::KVia, 2),
                location(location::Type::KBreakThrough, 3),
            ],
            ..TripLeg::default()
        };
        let waypoints = Json::Array(via_waypoints(&leg));
        let mut writer = JsonWriter::new();
        waypoints.write(&mut writer);
        assert_eq!(
            writer.finish(),
            concat!(
                r#"[{"geometry_index":10,"distance_from_start":123.457,"waypoint_index":1},"#,
                r#"{"geometry_index":20,"distance_from_start":123.457,"waypoint_index":2}]"#
            )
        );
    }
}
