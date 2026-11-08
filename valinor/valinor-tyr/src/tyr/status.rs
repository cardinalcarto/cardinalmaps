//! The `/status` response, as mainline's `tyr::serializeStatus` writes it.
//!
//! Loki (and, for verbose requests, Thor) fill in the request's `status`;
//! Odin's worker then serializes it here.

use super::{JSON_CONTENT_TYPE, PBF_CONTENT_TYPE, Response, unknown};
use valhalla_proto::options::Format;
use valhalla_proto::prost::Message;
use valhalla_proto::status::{
    HasBbox, HasHasAdmins, HasHasLiveTraffic, HasHasTiles, HasHasTimezones, HasHasTransitTiles,
    HasOsmChangeset,
};
use valhalla_proto::{Api, PbfFieldSelector};
use valhalla_response::{StatusResponse, ValhallaError};

/// Serializes the status that Loki and Thor filled in, in the requested format.
///
/// A missing `status` message is read as the default (proto3 semantics), as mainline does.
/// The PBF response takes the request's fields, so `api` is left empty for PBF.
///
/// # Errors
///
/// 299 if the JSON can't be written, which can't happen for the values a status holds.
pub fn serialize_status(api: &mut Api) -> Result<Response, ValhallaError> {
    let format = api.options.as_ref().map_or(0, |options| options.format);
    if Format::try_from(format) == Ok(Format::Pbf) {
        Ok(pbf_status(api))
    } else {
        json_status(api)
    }
}

/// The PBF response, as mainline's `tyr::serializePbf` writes it for the status action.
///
/// The response is the request's `Api` with every top-level field cleared
/// except those the client selected with `pbf_field_selector`
/// (only `status` by default) and `info`, which mainline never clears.
///
/// Mainline sends it with `Content-type: application/json;charset=utf-8`,
/// because `serializePbf` clears `options` before `to_response` reads the format from it.
/// No client can rely on that (it would misread the body),
/// so we send `application/x-protobuf` (see "Known divergences" in `docs/routing.md`).
fn pbf_status(request: &mut Api) -> Response {
    let mut response = core::mem::take(request);
    let selection = response
        .options
        .as_ref()
        .and_then(|options| options.pbf_field_selector)
        .unwrap_or(PbfFieldSelector {
            status: true,
            ..PbfFieldSelector::default()
        });

    if !selection.trip {
        response.trip = None;
    }
    if !selection.directions {
        response.directions = None;
    }
    if !selection.status {
        response.status = None;
    }
    if !selection.options {
        response.options = None;
    }
    if !selection.matrix {
        response.matrix = None;
    }
    if !selection.isochrone {
        response.isochrone = None;
    }
    if !selection.expansion {
        response.expansion = None;
    }

    Response {
        content_type: PBF_CONTENT_TYPE,
        body: response.encode_to_vec(),
    }
}

fn json_status(request: &Api) -> Result<Response, ValhallaError> {
    let status = request.status.clone().unwrap_or_default();

    let response = StatusResponse {
        version: status.version,
        tileset_last_modified: status.tileset_last_modified,
        available_actions: status.available_actions,
        has_tiles: status.has_has_tiles.map(|HasHasTiles::HasTiles(v)| v),
        has_admins: status.has_has_admins.map(|HasHasAdmins::HasAdmins(v)| v),
        has_timezones: status
            .has_has_timezones
            .map(|HasHasTimezones::HasTimezones(v)| v),
        has_live_traffic: status
            .has_has_live_traffic
            .map(|HasHasLiveTraffic::HasLiveTraffic(v)| v),
        has_transit_tiles: status
            .has_has_transit_tiles
            .map(|HasHasTransitTiles::HasTransitTiles(v)| v),
        // Mainline reads a zero changeset as "none" and leaves it out.
        osm_changeset: status
            .has_osm_changeset
            .map(|HasOsmChangeset::OsmChangeset(v)| v)
            .filter(|&changeset| changeset != 0),
        // Loki stores the bounding box as a `GeoJSON` string, which mainline embeds as JSON.
        // If it somehow does not parse, mainline's RapidJSON document is null, so we write null.
        bbox: status.has_bbox.map(|HasBbox::Bbox(bbox)| {
            serde_json::from_str(&bbox).unwrap_or(serde_json::Value::Null)
        }),
    };

    let body = serde_json::to_vec(&response).map_err(|e| unknown(e.to_string()))?;
    Ok(Response {
        content_type: JSON_CONTENT_TYPE,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use valhalla_proto::{Info, Options, Status, Trip, options};

    fn request(format: options::Format, status: Option<Status>) -> Api {
        Api {
            options: Some(Options {
                action: options::Action::Status.into(),
                format: format.into(),
                ..Default::default()
            }),
            status,
            ..Default::default()
        }
    }

    #[test]
    fn json_status_leaves_out_what_is_unset() {
        let mut api = request(
            options::Format::Json,
            Some(Status {
                version: "3.9.1".to_string(),
                tileset_last_modified: 7,
                available_actions: vec!["route".to_string()],
                has_has_admins: Some(HasHasAdmins::HasAdmins(true)),
                has_osm_changeset: Some(HasOsmChangeset::OsmChangeset(0)),
                has_bbox: Some(HasBbox::Bbox("not json".to_string())),
                ..Default::default()
            }),
        );
        let response = serialize_status(&mut api).unwrap();
        assert_eq!(response.content_type, JSON_CONTENT_TYPE);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(
            body,
            json!({
                "version": "3.9.1",
                "tileset_last_modified": 7,
                "available_actions": ["route"],
                "has_admins": true,
                "bbox": null,
            })
        );

        let mut api = request(options::Format::Json, None);
        let body: Value =
            serde_json::from_slice(&serialize_status(&mut api).unwrap().body).unwrap();
        assert_eq!(
            body,
            json!({"version": "", "tileset_last_modified": 0, "available_actions": []})
        );
    }

    #[test]
    fn pbf_status_keeps_the_selected_fields_and_info() {
        let status = Status {
            version: "3.9.1".to_string(),
            ..Default::default()
        };
        let info = Info {
            is_service: true,
            ..Default::default()
        };
        let mut api = request(options::Format::Pbf, Some(status.clone()));
        api.info = Some(info.clone());
        api.trip = Some(Trip::default());
        let response = serialize_status(&mut api).unwrap();
        assert_eq!(response.content_type, PBF_CONTENT_TYPE);
        assert_eq!(
            Api::decode(response.body.as_slice()).unwrap(),
            Api {
                status: Some(status),
                info: Some(info),
                ..Default::default()
            }
        );
    }
}
