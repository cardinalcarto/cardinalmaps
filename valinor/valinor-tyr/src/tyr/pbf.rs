//! The PBF response, as mainline's `tyr::serializePbf` (`tyr/serializers.cc`) writes it.
//!
//! The response is the request's `Api` itself,
//! with every top-level field the client did not select cleared.
//! Clients select fields with `options.pbf_field_selector`
//! (which only PBF requests can set; the JSON front never parses it).
//! Without a selector, the action picks the obvious field
//! (`directions` for route-like actions, `trip` for `trace_attributes`, and so on).
//! `info` is never cleared.

use super::{PBF_CONTENT_TYPE, Response, unknown};
use valhalla_proto::options::Action;
use valhalla_proto::prost::Message;
use valhalla_proto::{Api, PbfFieldSelector};
use valhalla_response::ValhallaError;

/// The fields mainline sends when the request has no `pbf_field_selector`.
///
/// # Errors
///
/// Fails, as mainline does (`logic_error`, which Odin reports as 299),
/// for actions with no PBF output.
fn default_selection(action: i32) -> Result<PbfFieldSelector, ValhallaError> {
    let mut selection = PbfFieldSelector::default();
    match Action::try_from(action) {
        Ok(Action::Route | Action::Centroid | Action::OptimizedRoute | Action::TraceRoute) => {
            selection.directions = true;
        }
        Ok(Action::TraceAttributes) => selection.trip = true,
        Ok(Action::Status) => selection.status = true,
        Ok(Action::SourcesToTargets) => selection.matrix = true,
        Ok(Action::Isochrone) => selection.isochrone = true,
        Ok(Action::Expansion) => selection.expansion = true,
        _ => {
            return Err(unknown("Requested action is not yet serializable as pbf"));
        }
    }
    Ok(selection)
}

/// Serializes the request as a PBF response.
///
/// Like mainline, this clears the unselected fields from the request itself.
/// The one exception is `options`:
/// it is left out of the response unless selected, but stays in the request,
/// which is what mainline does for service requests
/// (so code that runs after serialization can still read the format and action).
///
/// # Errors
///
/// Fails with 299 for actions that have no default selection and no `pbf_field_selector`.
pub(crate) fn serialize(api: &mut Api) -> Result<Response, ValhallaError> {
    let options = api.options.as_ref();
    let selection = match options.and_then(|options| options.pbf_field_selector) {
        Some(selection) => selection,
        None => default_selection(options.map_or(0, |options| options.action))?,
    };

    if !selection.trip {
        api.trip = None;
    }
    if !selection.directions {
        api.directions = None;
    }
    if !selection.status {
        api.status = None;
    }
    if !selection.matrix {
        api.matrix = None;
    }
    if !selection.isochrone {
        api.isochrone = None;
    }
    if !selection.expansion {
        api.expansion = None;
    }
    let options = if selection.options {
        None
    } else {
        api.options.take()
    };
    let body = api.encode_to_vec();
    if options.is_some() {
        api.options = options;
    }
    Ok(Response {
        content_type: PBF_CONTENT_TYPE,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::captures;
    use valhalla_proto::{
        Directions, Expansion, Info, Isochrone, Matrix, Options, Status, Trip, options,
    };
    use valhalla_response::ErrorCode;

    /// An `Api` with every top-level field present.
    fn full(action: Action, selector: Option<PbfFieldSelector>) -> Api {
        Api {
            options: Some(Options {
                action: action.into(),
                format: options::Format::Pbf.into(),
                pbf_field_selector: selector,
                ..Options::default()
            }),
            trip: Some(Trip::default()),
            directions: Some(Directions::default()),
            status: Some(Status::default()),
            matrix: Some(Matrix::default()),
            isochrone: Some(Isochrone::default()),
            expansion: Some(Expansion::default()),
            info: Some(Info {
                is_service: true,
                ..Info::default()
            }),
        }
    }

    /// Which top-level fields a response holds, in `PbfFieldSelector` order.
    fn present(body: &[u8]) -> [bool; 7] {
        let api = Api::decode(body).expect("valid Api");
        assert!(api.info.is_some(), "info is always sent");
        [
            api.options.is_some(),
            api.trip.is_some(),
            api.directions.is_some(),
            api.status.is_some(),
            api.matrix.is_some(),
            api.isochrone.is_some(),
            api.expansion.is_some(),
        ]
    }

    #[test]
    fn action_defaults() {
        let cases = [
            (Action::Route, 2),
            (Action::Centroid, 2),
            (Action::OptimizedRoute, 2),
            (Action::TraceRoute, 2),
            (Action::TraceAttributes, 1),
            (Action::Status, 3),
            (Action::SourcesToTargets, 4),
            (Action::Isochrone, 5),
            (Action::Expansion, 6),
        ];
        for (action, field) in cases {
            let mut api = full(action, None);
            let response = serialize(&mut api).expect("serializable");
            assert_eq!(response.content_type, PBF_CONTENT_TYPE);
            let mut expected = [false; 7];
            expected[field] = true;
            assert_eq!(present(&response.body), expected, "{action:?}");
            // The options stay in the request.
            assert!(api.options.is_some(), "{action:?}");
        }
    }

    #[test]
    fn actions_without_pbf_output() {
        for action in [
            Action::NoAction,
            Action::Locate,
            Action::Height,
            Action::TransitAvailable,
            Action::Tile,
        ] {
            let error = serialize(&mut full(action, None)).expect_err("no pbf output");
            assert_eq!(error.code(), ErrorCode::OdinUnknown, "{action:?}");
            assert_eq!(
                error.message(),
                "Unknown: Requested action is not yet serializable as pbf"
            );
        }
        // No options at all reads as no action.
        let mut api = Api::default();
        assert!(serialize(&mut api).is_err());
    }

    #[test]
    fn each_selector_field() {
        for field in 0..7 {
            let mut selector = PbfFieldSelector::default();
            *[
                &mut selector.options,
                &mut selector.trip,
                &mut selector.directions,
                &mut selector.status,
                &mut selector.matrix,
                &mut selector.isochrone,
                &mut selector.expansion,
            ][field] = true;
            // The selector overrides the action's default, even for actions without one.
            for action in [Action::Route, Action::Locate] {
                let mut api = full(action, Some(selector));
                let response = serialize(&mut api).expect("selected");
                let mut expected = [false; 7];
                expected[field] = true;
                assert_eq!(present(&response.body), expected, "{field} {action:?}");
            }
        }
    }

    #[test]
    fn empty_selector_sends_only_info() {
        let mut api = full(Action::Route, Some(PbfFieldSelector::default()));
        let response = serialize(&mut api).expect("selected");
        assert_eq!(present(&response.body), [false; 7]);
        assert!(api.trip.is_none() && api.directions.is_none());
    }

    #[test]
    fn selected_options_are_sent() {
        let selector = PbfFieldSelector {
            options: true,
            directions: true,
            ..PbfFieldSelector::default()
        };
        let mut api = full(Action::Route, Some(selector));
        let expected_options = api.options.clone();
        let response = serialize(&mut api).expect("selected");
        let decoded = Api::decode(response.body.as_slice()).expect("valid Api");
        assert_eq!(decoded.options, expected_options);
        assert!(decoded.directions.is_some());
    }

    /// Every captured PBF route response, from what Odin handed Tyr.
    #[test]
    fn route_responses_match_mainline() {
        let mut checked = 0;
        for capture in captures("pbf") {
            let (status, expected) = capture.pbf_response();
            let Some(mut api) = capture.odin_output() else {
                assert_ne!(status, 200, "{}", capture.name());
                continue;
            };
            assert_eq!(status, 200, "{}", capture.name());
            let response = serialize(&mut api).expect("serializable");
            let actual = Api::decode(response.body.as_slice()).expect("valid Api");
            let decoded = Api::decode(expected.as_slice()).expect("valid Api");
            assert_eq!(actual, decoded, "{}", capture.name());
            // Mainline's bytes too: prost and protobuf write fields in the same order.
            assert!(
                response.body == expected,
                "{}: bytes differ",
                capture.name()
            );
            checked += 1;
        }
        assert!(checked > 20, "only {checked} PBF captures");
    }
}
