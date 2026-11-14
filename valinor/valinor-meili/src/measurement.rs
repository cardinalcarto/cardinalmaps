//! A GPS measurement (mainline's `meili::Measurement`), and the measurements of a request's
//! shape (what Thor's `parse_measurements` builds).

use thiserror::Error;
use valhalla_graphtile::midgard::{PointLL, narrow};
use valhalla_proto::Location;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::{HasAccuracy, HasRadius, HasTime, Type as LocationType};

use crate::config::MeiliConfig;

/// A measurement mainline's constructor rejects.
///
/// Thor reports either as error 424 ("Failed to parse shape").
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum MeasurementError {
    /// "non-negative `gps_accuracy` required".
    #[error("make a measurement: non-negative gps_accuracy required (got {0})")]
    GpsAccuracy(f32),
    /// "non-negative `search_radius` required".
    #[error("make a measurement: non-negative search_radius required (got {0})")]
    SearchRadius(f32),
}

/// One point of a trace (mainline's `meili::Measurement`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    lnglat: PointLL,
    gps_accuracy: f32,
    search_radius: f32,
    epoch_time: f64,
    stop_type: LocationType,
}

impl Measurement {
    /// A measurement at `lnglat` (longitude and latitude), with its GPS accuracy and search
    /// radius in meters, its time in seconds since the epoch (or -1 for none), and the type of
    /// the shape point it comes from (`break`, `through`, `via`, or `break_through`).
    ///
    /// # Errors
    ///
    /// Fails, as mainline's constructor throws, if the accuracy or radius is negative
    /// (NaN passes, as in mainline).
    pub fn new(
        lnglat: (f64, f64),
        gps_accuracy: f32,
        search_radius: f32,
        epoch_time: f64,
        stop_type: LocationType,
    ) -> Result<Self, MeasurementError> {
        if gps_accuracy < 0.0 {
            return Err(MeasurementError::GpsAccuracy(gps_accuracy));
        }
        if search_radius < 0.0 {
            return Err(MeasurementError::SearchRadius(search_radius));
        }
        Ok(Self {
            lnglat: PointLL::new(lnglat.0, lnglat.1),
            gps_accuracy,
            search_radius,
            epoch_time,
            stop_type,
        })
    }

    /// The measurement of a shape point, as Thor's `parse_measurements` makes it:
    /// the point's `accuracy` and `radius` if it has them, else the config's
    /// (merged) GPS accuracy and search radius; its `time` (-1 when the front set none);
    /// and its type.
    ///
    /// # Errors
    ///
    /// As for [`Measurement::new`].
    pub fn from_location(
        location: &Location,
        config: &MeiliConfig,
    ) -> Result<Self, MeasurementError> {
        let ll = location.ll.unwrap_or_default();
        let lng = match ll.has_lng {
            Some(HasLng::Lng(lng)) => lng,
            None => 0.0,
        };
        let lat = match ll.has_lat {
            Some(HasLat::Lat(lat)) => lat,
            None => 0.0,
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "mainline converts the uint32 to float"
        )]
        let gps_accuracy = match location.has_accuracy {
            Some(HasAccuracy::Accuracy(a)) => a as f32,
            None => config.emission_cost.gps_accuracy_meters,
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "mainline converts the uint32 to float"
        )]
        let search_radius = match location.has_radius {
            Some(HasRadius::Radius(r)) => r as f32,
            None => config.candidate_search.search_radius_meters,
        };
        let time = match location.has_time {
            Some(HasTime::Time(t)) => t,
            None => 0.0,
        };
        // A type mainline doesn't define behaves as `through` does here
        // (neither a break nor a via).
        let stop_type = LocationType::try_from(location.r#type).unwrap_or(LocationType::KThrough);
        Self::new((lng, lat), gps_accuracy, search_radius, time, stop_type)
    }

    /// The measurements of a request's shape (Thor's `parse_measurements`).
    ///
    /// # Errors
    ///
    /// As for [`Measurement::new`], for the first point that fails.
    pub fn from_shape(
        shape: &[Location],
        config: &MeiliConfig,
    ) -> Result<Vec<Self>, MeasurementError> {
        shape
            .iter()
            .map(|location| Self::from_location(location, config))
            .collect()
    }

    /// The longitude and latitude.
    #[inline]
    pub fn lnglat(&self) -> (f64, f64) {
        (self.lnglat.lng, self.lnglat.lat)
    }

    #[inline]
    pub(crate) const fn point(&self) -> PointLL {
        self.lnglat
    }

    /// The search radius in meters.
    #[inline]
    pub const fn search_radius(&self) -> f32 {
        self.search_radius
    }

    /// The squared search radius (`sq_search_radius`, in single precision).
    #[inline]
    pub fn sq_search_radius(&self) -> f32 {
        self.search_radius * self.search_radius
    }

    /// The GPS accuracy in meters.
    #[inline]
    pub const fn gps_accuracy(&self) -> f32 {
        self.gps_accuracy
    }

    /// The squared GPS accuracy (`sq_gps_accuracy`, in single precision).
    #[inline]
    pub fn sq_gps_accuracy(&self) -> f32 {
        self.gps_accuracy * self.gps_accuracy
    }

    /// The time in seconds since the epoch, or a negative value for none.
    #[inline]
    pub const fn epoch_time(&self) -> f64 {
        self.epoch_time
    }

    /// The type of the shape point.
    #[inline]
    pub const fn stop_type(&self) -> LocationType {
        self.stop_type
    }

    /// Whether this is a `break` or `break_through` point (`is_break_point`).
    #[inline]
    pub fn is_break_point(&self) -> bool {
        matches!(
            self.stop_type,
            LocationType::KBreak | LocationType::KBreakThrough
        )
    }

    /// The great circle distance to another measurement in meters, in single precision
    /// (`distance`).
    #[inline]
    pub(crate) fn distance(&self, other: &Self) -> f32 {
        narrow(self.lnglat.distance(other.lnglat))
    }

    /// The approximate squared distance to another measurement, in single precision
    /// (`distance_squared`).
    #[inline]
    pub(crate) fn distance_squared(&self, other: &Self) -> f32 {
        narrow(self.lnglat.distance_squared(other.lnglat))
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "the values are exact")]
mod tests {
    use super::*;
    use valhalla_proto::LatLng;

    #[test]
    fn rejects_negative_accuracy_and_radius() {
        let at = (1.5, 42.5);
        assert_eq!(
            Measurement::new(at, -1.0, 5.0, -1.0, LocationType::KVia),
            Err(MeasurementError::GpsAccuracy(-1.0))
        );
        assert_eq!(
            Measurement::new(at, 1.0, -0.5, -1.0, LocationType::KVia),
            Err(MeasurementError::SearchRadius(-0.5))
        );
        assert!(Measurement::new(at, 0.0, 0.0, -1.0, LocationType::KVia).is_ok());
    }

    #[test]
    fn reads_shape_points_as_thor_does() {
        let mut config = MeiliConfig::default();
        config.candidate_search.search_radius_meters = -2.0;
        let point = Location {
            ll: Some(LatLng {
                has_lat: Some(HasLat::Lat(42.5)),
                has_lng: Some(HasLng::Lng(1.5)),
            }),
            r#type: LocationType::KBreakThrough as i32,
            has_accuracy: Some(HasAccuracy::Accuracy(12)),
            has_time: Some(HasTime::Time(-1.0)),
            ..Location::default()
        };
        // The merged config's radius applies when the point has none, and is checked.
        assert_eq!(
            Measurement::from_location(&point, &config),
            Err(MeasurementError::SearchRadius(-2.0))
        );
        let point = Location {
            has_radius: Some(HasRadius::Radius(30)),
            ..point
        };
        let m = Measurement::from_location(&point, &config).unwrap();
        assert_eq!(m.lnglat(), (1.5, 42.5));
        assert_eq!(m.gps_accuracy(), 12.0);
        assert_eq!(m.search_radius(), 30.0);
        assert_eq!(m.epoch_time(), -1.0);
        assert!(m.is_break_point());
        let unknown = Location {
            r#type: 17,
            ..point
        };
        assert!(
            !Measurement::from_location(&unknown, &config)
                .unwrap()
                .is_break_point()
        );
    }
}
