//! The `/status` response, as mainline's `tyr::serializeStatus` writes it
//! (see `valinor_tyr::tyr::serialize_status`).

use super::directions::http_response;
use valhalla_microservice::WorkerResult;
use valhalla_proto::Api;
use valinor_tyr::tyr::serialize_status;

/// Serializes the status that Loki and Thor filled in, in the requested format.
///
/// A missing `status` message is read as the default (proto3 semantics), as mainline does.
pub fn status(request: &mut Api) -> WorkerResult {
    match serialize_status(request) {
        Ok(response) => http_response(response),
        Err(error) => WorkerResult::error(&error, request),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;
    use serde_json::json;
    use valhalla_proto::prost::Message;
    use valhalla_proto::status::{
        HasBbox, HasHasAdmins, HasHasLiveTraffic, HasHasTiles, HasHasTimezones, HasHasTransitTiles,
        HasOsmChangeset,
    };
    use valhalla_proto::{Info, Options, PbfFieldSelector, Status, Trip, options};
    use valhalla_response::StatusResponse;

    // A status request from a 3.5.1 server, as Thor passed it to Odin.
    const STATUS_MESSAGE: &[u8] = &[
        0xa, 0x43, 0x12, 0x5, 0x65, 0x6e, 0x2d, 0x55, 0x53, 0x40, 0xc, 0x50, 0x0, 0x58, 0x0, 0x6a,
        0xf, 0x8, 0x0, 0x12, 0xb, 0xa, 0x3, 0xb8, 0x3, 0x0, 0x1a, 0x4, 0x6e, 0x6f, 0x6e, 0x65,
        0xb8, 0x1, 0x0, 0xc5, 0x1, 0x0, 0x0, 0x80, 0x3f, 0xd0, 0x1, 0x0, 0xa8, 0x2, 0x0, 0xb8, 0x2,
        0x0, 0xc8, 0x2, 0x0, 0xe0, 0x2, 0x1, 0x80, 0x3, 0x0, 0x90, 0x3, 0x0, 0xb0, 0x3, 0xff, 0xff,
        0xff, 0xff, 0xf, 0x22, 0xa3, 0x1, 0x32, 0x5, 0x33, 0x2e, 0x35, 0x2e, 0x31, 0x38, 0xee,
        0xc5, 0xc4, 0xc5, 0x6, 0x42, 0x6, 0x73, 0x74, 0x61, 0x74, 0x75, 0x73, 0x42, 0x8, 0x63,
        0x65, 0x6e, 0x74, 0x72, 0x6f, 0x69, 0x64, 0x42, 0x9, 0x65, 0x78, 0x70, 0x61, 0x6e, 0x73,
        0x69, 0x6f, 0x6e, 0x42, 0x11, 0x74, 0x72, 0x61, 0x6e, 0x73, 0x69, 0x74, 0x5f, 0x61, 0x76,
        0x61, 0x69, 0x6c, 0x61, 0x62, 0x6c, 0x65, 0x42, 0x10, 0x74, 0x72, 0x61, 0x63, 0x65, 0x5f,
        0x61, 0x74, 0x74, 0x72, 0x69, 0x62, 0x75, 0x74, 0x65, 0x73, 0x42, 0xb, 0x74, 0x72, 0x61,
        0x63, 0x65, 0x5f, 0x72, 0x6f, 0x75, 0x74, 0x65, 0x42, 0x9, 0x69, 0x73, 0x6f, 0x63, 0x68,
        0x72, 0x6f, 0x6e, 0x65, 0x42, 0xf, 0x6f, 0x70, 0x74, 0x69, 0x6d, 0x69, 0x7a, 0x65, 0x64,
        0x5f, 0x72, 0x6f, 0x75, 0x74, 0x65, 0x42, 0x12, 0x73, 0x6f, 0x75, 0x72, 0x63, 0x65, 0x73,
        0x5f, 0x74, 0x6f, 0x5f, 0x74, 0x61, 0x72, 0x67, 0x65, 0x74, 0x73, 0x42, 0x6, 0x68, 0x65,
        0x69, 0x67, 0x68, 0x74, 0x42, 0x5, 0x72, 0x6f, 0x75, 0x74, 0x65, 0x42, 0x6, 0x6c, 0x6f,
        0x63, 0x61, 0x74, 0x65,
    ];

    fn into_parts(result: WorkerResult) -> (StatusCode, http::HeaderMap, Vec<u8>) {
        let WorkerResult::HttpResponse {
            status_code,
            headers,
            body,
        } = result
        else {
            panic!("Expected an HTTP response.");
        };
        (status_code, headers, body)
    }

    fn status_request(format: options::Format, status: Status) -> Api {
        Api {
            options: Some(Options {
                action: options::Action::Status.into(),
                format: format.into(),
                ..Default::default()
            }),
            status: Some(status),
            ..Default::default()
        }
    }

    #[test]
    fn status_happy_path() {
        // Happy path test for generating a status response
        // using a protobuf capture from a working server.
        let mut request = Api::decode(STATUS_MESSAGE).expect("valid Api");
        let (status_code, headers, body) = into_parts(status(&mut request));

        assert_eq!(status_code, StatusCode::OK);
        assert_eq!(
            headers.get("Content-Type").expect("content type"),
            "application/json;charset=utf-8"
        );

        let status_res: StatusResponse = serde_json::from_slice(&body).expect("JSON status");

        if !cfg!(miri) {
            insta::assert_yaml_snapshot!(status_res);
        }
    }

    #[test]
    fn verbose_json_status() {
        let mut request = status_request(
            options::Format::Json,
            Status {
                version: "3.9.1".to_string(),
                tileset_last_modified: 1_762_000_000,
                available_actions: vec!["status".to_string(), "route".to_string()],
                has_has_tiles: Some(HasHasTiles::HasTiles(true)),
                has_has_admins: Some(HasHasAdmins::HasAdmins(true)),
                has_has_timezones: Some(HasHasTimezones::HasTimezones(false)),
                has_has_live_traffic: Some(HasHasLiveTraffic::HasLiveTraffic(false)),
                has_has_transit_tiles: Some(HasHasTransitTiles::HasTransitTiles(false)),
                has_osm_changeset: Some(HasOsmChangeset::OsmChangeset(0)),
                has_bbox: Some(HasBbox::Bbox(
                    r#"{"type":"FeatureCollection","features":[]}"#.to_string(),
                )),
            },
        );
        let (_, _, body) = into_parts(status(&mut request));
        let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(
            body,
            json!({
                "version": "3.9.1",
                "tileset_last_modified": 1_762_000_000,
                "available_actions": ["status", "route"],
                "has_tiles": true,
                "has_admins": true,
                "has_timezones": false,
                "has_live_traffic": false,
                "has_transit_tiles": false,
                "bbox": {"type": "FeatureCollection", "features": []},
            })
        );
    }

    #[test]
    fn nonzero_changeset_is_reported() {
        let mut request = status_request(
            options::Format::Json,
            Status {
                has_osm_changeset: Some(HasOsmChangeset::OsmChangeset(42)),
                ..Default::default()
            },
        );
        let (_, _, body) = into_parts(status(&mut request));
        let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(body["osm_changeset"], 42);
    }

    #[test]
    fn missing_status_is_the_default() {
        let mut request = status_request(options::Format::Json, Status::default());
        request.status = None;
        let (status_code, _, body) = into_parts(status(&mut request));
        assert_eq!(status_code, StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(
            body,
            json!({"version": "", "tileset_last_modified": 0, "available_actions": []})
        );
    }

    /// Checked against mainline 3.9.1's Odin in a staged chain (`ODIN=illuvatar` in
    /// `tools/oracle/serve-staged.sh`): the JSON and PBF bodies are byte-identical.
    ///
    /// Mainline sends PBF status responses with `Content-type: application/json;charset=utf-8`,
    /// because `serializePbf` clears `options` before `to_response` reads the format from it.
    /// No client can rely on that (it would misread the body),
    /// so we send `application/x-protobuf`.
    #[test]
    fn pbf_status_keeps_only_status_and_info() {
        let status_message = Status {
            version: "3.9.1".to_string(),
            available_actions: vec!["status".to_string()],
            ..Default::default()
        };
        let info = Info {
            is_service: true,
            ..Default::default()
        };
        let mut request = status_request(options::Format::Pbf, status_message.clone());
        request.info = Some(info.clone());
        request.trip = Some(Trip::default());

        let (status_code, headers, body) = into_parts(status(&mut request));
        assert_eq!(status_code, StatusCode::OK);
        assert_eq!(
            headers.get("Content-Type").expect("content type"),
            "application/x-protobuf"
        );
        assert_eq!(
            Api::decode(body.as_slice()).expect("valid Api"),
            Api {
                status: Some(status_message),
                info: Some(info),
                ..Default::default()
            }
        );
    }

    #[test]
    fn pbf_status_honors_the_field_selector() {
        let mut request = status_request(options::Format::Pbf, Status::default());
        let selector = PbfFieldSelector {
            options: true,
            ..Default::default()
        };
        if let Some(options) = request.options.as_mut() {
            options.pbf_field_selector = Some(selector);
        }
        let expected_options = request.options.clone();

        let (_, _, body) = into_parts(status(&mut request));
        assert_eq!(
            Api::decode(body.as_slice()).expect("valid Api"),
            Api {
                options: expected_options,
                ..Default::default()
            }
        );
    }
}
