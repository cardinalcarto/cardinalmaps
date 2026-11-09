//! Vehicle details and lookup by ID.

use valinor_gtfs_rt::{FeedId, ScopedId, VehicleDetails, VehicleRef};

use crate::response::build::{feed_report, stop_name_of};
use crate::response::{Vehicle, VehicleStop, VehicleUpdate};
use crate::state::TransitSnapshot;

/// A [`Vehicle`] from what the overlay stored, with a descriptor for what it doesn't.
pub(crate) fn vehicle_from(
    snapshot: &TransitSnapshot,
    details: Option<&VehicleDetails>,
    reference: Option<&VehicleRef>,
) -> Vehicle {
    let identity = details.map(|d| &d.vehicle).or(reference);
    Vehicle {
        feed: details
            .and_then(|d| d.feed.as_ref())
            .map(ToString::to_string),
        id: identity.and_then(|v| v.id.clone()),
        label: identity.and_then(|v| v.label.clone()),
        license_plate: identity.and_then(|v| v.license_plate.clone()),
        wheelchair_accessible: identity
            .and_then(|v| v.wheelchair)
            .map(|w| lower(w.as_str_name())),
        trip_id: details.and_then(|d| d.trip_id.clone()),
        route_id: details.and_then(|d| d.route_id.clone()),
        latitude: details.and_then(|d| d.latitude),
        longitude: details.and_then(|d| d.longitude),
        bearing: details.and_then(|d| d.bearing),
        speed_meters_per_second: details.and_then(|d| d.speed),
        odometer_meters: details.and_then(|d| d.odometer),
        status: details
            .and_then(|d| d.current_status)
            .map(|s| lower(s.as_str_name())),
        current_stop: details.and_then(|d| {
            let stop_id = d.stop_id.clone()?;
            let feed = d.feed.as_ref()?;
            let name = stop_name_of(snapshot, feed, &stop_id);
            Some(VehicleStop { stop_id, name })
        }),
        current_stop_sequence: details.and_then(|d| d.current_stop_sequence),
        occupancy: details
            .and_then(|d| d.occupancy)
            .map(|o| lower(o.as_str_name())),
        occupancy_percentage: details.and_then(|d| d.occupancy_percentage),
        congestion: details
            .and_then(|d| d.congestion)
            .map(|c| lower(c.as_str_name())),
        timestamp: details.and_then(|d| d.timestamp),
    }
}

pub(crate) fn vehicle_update(
    snapshot: &TransitSnapshot,
    now: i64,
    feed: &str,
    id: &str,
) -> VehicleUpdate {
    let details = FeedId::new(feed)
        .ok()
        .and_then(|feed| snapshot.rider.vehicle(&ScopedId::new(&feed, id)));
    let vehicle = details.map(|details| vehicle_from(snapshot, Some(details), None));
    VehicleUpdate {
        vehicle,
        feeds: snapshot
            .feed_status_at(now)
            .iter()
            .filter(|status| status.feed.as_str() == feed)
            .map(feed_report)
            .collect(),
        built_at: snapshot.built_at,
    }
}

/// GTFS-RT's enum names, lowercased, for JSON.
pub(crate) fn lower(name: &str) -> String {
    name.to_ascii_lowercase()
}
