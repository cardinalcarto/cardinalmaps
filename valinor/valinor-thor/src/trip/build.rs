//! The request's `trip`: one route per path found (the best, then each alternate),
//! with a leg per pair of break locations (the assembly in mainline's `path_depart_at`
//! and `path_arrive_by`).

use jiff::Timestamp;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::{Api, Trip, TripRoute};
use valhalla_response::ValhallaError;
use valinor_sif::AnyCosting;

use super::attributes::AttributesController;
use super::leg::{LegContext, build_leg};
use crate::error::ThorError;
use crate::path::Paths;
use crate::route::Thor;

impl<P: GraphTileProvider> Thor<P> {
    /// Builds the request's `trip` from the paths [`Thor::route`] found
    /// (mainline's `TripLegBuilder::Build` for each leg, and the trip's routes and legs).
    ///
    /// `api` must be the request as [`Thor::route`] left it
    /// (its options carry the locations path search updated).
    ///
    /// # Errors
    ///
    /// Error 499 for failures mainline reports as exceptions: tiles that are gone,
    /// corrupt tile data, or locations whose candidate edges don't match the path
    /// (mainline's "`leg_shape_index` not set for intermediate location" among them).
    pub fn build_trip(&self, api: &mut Api, paths: &Paths) -> Result<(), ValhallaError> {
        self.build_trip_interruptible(api, paths, Timestamp::now(), &|| false)
    }

    /// [`Thor::build_trip`], with `now` as the current time,
    /// giving up when `interrupt` returns true (it is checked before each leg, as in mainline).
    ///
    /// # Errors
    ///
    /// As for [`Thor::build_trip`]; an interrupt is error 499
    /// (see [`Thor::route_interruptible`]).
    pub fn build_trip_interruptible(
        &self,
        api: &mut Api,
        paths: &Paths,
        now: Timestamp,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<(), ValhallaError> {
        let trip = self.trip(api, paths, now, interrupt)?;
        api.trip = Some(trip);
        Ok(())
    }

    fn trip(
        &self,
        api: &Api,
        paths: &Paths,
        now: Timestamp,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<Trip, ThorError> {
        let options = api
            .options
            .as_ref()
            .ok_or(ThorError::Options("the request has no options".to_owned()))?;
        let proto = options
            .costings
            .get(&options.costing_type)
            .cloned()
            .unwrap_or(valhalla_proto::Costing {
                r#type: options.costing_type,
                ..valhalla_proto::Costing::default()
            });
        // Trip building only asks the costing for edge speeds and travel types,
        // which path search's adjustments (passes and hierarchy limits) don't change.
        let costing = AnyCosting::new(&proto).map_err(|e| ThorError::Options(e.to_string()))?;
        let controller = AttributesController::new(options);
        let ctx = LegContext {
            provider: self.provider(),
            options,
            controller: &controller,
            costing: &costing,
            now,
        };
        let mut trip = Trip::default();
        for route in &paths.routes {
            let mut legs = Vec::with_capacity(route.legs.len());
            for leg in &route.legs {
                if interrupt() {
                    return Err(ThorError::Interrupted);
                }
                legs.push(build_leg(&ctx, leg)?);
            }
            trip.routes.push(TripRoute {
                legs,
                ..TripRoute::default()
            });
        }
        Ok(trip)
    }
}
