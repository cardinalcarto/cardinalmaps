//! How prominent a place is regardless of any query, from 0 to 1. Search
//! multiplies text relevance by `1 + IMPORTANCE_WEIGHT * importance`, so
//! importance reorders matches but can't make a poor match outrank a good one
//! by much.

/// How much importance can raise a result's score.
pub const IMPORTANCE_WEIGHT: f32 = 2.0;

/// The prior for an administrative area of each placetype, before its
/// population is known.
fn placetype_prior(placetype: &str) -> f64 {
    match placetype {
        "continent" | "country" => 0.9,
        "dependency" | "ocean" => 0.8,
        "empire" => 0.6,
        "macroregion" | "region" => 0.55,
        "disputed" | "marinearea" => 0.5,
        "locality" => 0.5,
        "localadmin" | "borough" => 0.4,
        "macrocounty" => 0.35,
        "county" => 0.3,
        "macrohood" => 0.35,
        "neighbourhood" => 0.3,
        "microhood" => 0.2,
        _ => 0.3,
    }
}

/// Whether a placetype is a settlement, where people live and go, rather
/// than an area that groups them. A settlement's population says how much
/// people look for it; an area's mostly says how many settlements it holds.
fn is_settlement(placetype: &str) -> bool {
    matches!(placetype, "locality" | "localadmin" | "borough" | "macrohood" | "neighbourhood" | "microhood")
}

/// An administrative area's importance from its placetype and Who's On
/// First's `wof:population_rank`, from 0 (unknown or tiny) to 18 (the
/// largest megacities).
#[must_use]
pub fn admin_importance(placetype: &str, population_rank: Option<u32>) -> f64 {
    // For settlements population counts for more than placetype, so a big
    // city outranks a small region of the same name; for areas less, so a
    // county doesn't outrank its seat.
    let prior = placetype_prior(placetype);
    let population = population_rank.map_or(prior * 0.5, |rank| f64::from(rank.min(18)) / 18.0);
    let population_weight = if is_settlement(placetype) { 0.7 } else { 0.4 };
    ((1.0 - population_weight) * prior + population_weight * population).clamp(0.0, 1.0)
}

/// A neighbourhood's importance, which is at least a share of its city's:
/// neighbourhoods rarely have populations, and a big city's are better known
/// than small towns elsewhere.
#[must_use]
pub fn neighbourhood_importance(own: f64, city: f64) -> f64 {
    own.max(0.65 * city)
}

/// Whether a placetype is part of a city, inheriting its importance.
#[must_use]
pub fn is_neighbourhood(placetype: &str) -> bool {
    matches!(placetype, "borough" | "macrohood" | "neighbourhood" | "microhood")
}

/// What's known about a POI's prominence.
#[derive(Debug, Clone, Copy, Default)]
pub struct PoiSignals {
    /// Whether it has a `wikidata` tag.
    pub has_wikidata: bool,
    /// Its entity's QRank (page views of its Wikipedia articles over a
    /// year), if known.
    pub qrank: Option<u64>,
    /// The geodesic area of its shape in square meters, zero for points and
    /// lines.
    pub area_m2: f64,
    /// Whether it's a transit stop or platform, of which there are very many
    /// with names that also name the streets and places around them.
    pub transit_stop: bool,
}

/// A POI's importance.
#[must_use]
pub fn poi_importance(signals: PoiSignals) -> f64 {
    // QRank spans about 1 to 10^9; a Wikidata entity without a QRank has
    // next to no page views but still counts for something.
    let wikidata = match (signals.qrank, signals.has_wikidata) {
        (Some(qrank), _) => (0.2 + 0.8 * ((qrank.max(1) as f64).log10() / 8.0)).min(1.0),
        (None, true) => 0.2,
        (None, false) => 0.0,
    };
    // A building is around 10^2.5 m², a campus or park 10^5 to 10^6, a large
    // national park 10^9 and up.
    let area = ((signals.area_m2.max(1.0)).log10() / 8.0).clamp(0.0, 1.0);
    // At most about the importance of a mid-sized city, so a famous lake or
    // airport doesn't outrank the city it's named after.
    let mut importance = 0.08 + 0.38 * wikidata + 0.10 * area;
    if signals.transit_stop {
        importance *= 0.5;
    }
    importance.clamp(0.0, 1.0)
}

/// An address's importance.
pub const ADDRESS_IMPORTANCE: f64 = 0.02;

/// Whether OSM tags describe a transit stop or platform.
#[must_use]
pub fn is_transit_stop(tags: &[(String, String)]) -> bool {
    tags.iter().any(|(k, v)| {
        matches!(
            (k.as_str(), v.as_str()),
            ("highway", "bus_stop")
                | ("public_transport", "platform" | "stop_position")
                | ("railway", "platform" | "tram_stop")
        )
    })
}

/// A Wikidata ID such as `Q60` as a number.
#[must_use]
pub fn wikidata_number(id: &str) -> Option<u64> {
    id.trim().strip_prefix('Q')?.parse().ok()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn bigger_cities_are_more_important() {
        assert!(admin_importance("locality", Some(11)) > admin_importance("locality", Some(5)));
        assert!(admin_importance("country", Some(14)) > admin_importance("neighbourhood", Some(14)));
    }

    #[test]
    fn a_county_doesnt_outrank_its_seat() {
        // Spokane County and Spokane.
        assert!(admin_importance("locality", Some(10)) > admin_importance("county", Some(12)));
    }

    #[test]
    fn a_big_citys_neighbourhoods_outrank_small_towns() {
        // Ballard, Seattle and Ballard, Utah.
        let ballard = neighbourhood_importance(admin_importance("neighbourhood", None), admin_importance("locality", Some(12)));
        assert!(ballard > admin_importance("locality", Some(3)));
    }

    #[test]
    fn a_big_city_outranks_a_small_region() {
        // Portland, Oregon and Portland Parish, Jamaica.
        assert!(admin_importance("locality", Some(11)) > admin_importance("region", Some(7)));
    }

    #[test]
    fn well_known_pois_are_more_important() {
        let plain = poi_importance(PoiSignals::default());
        let wikidata = poi_importance(PoiSignals { has_wikidata: true, ..Default::default() });
        let famous = poi_importance(PoiSignals { has_wikidata: true, qrank: Some(10_000_000), ..Default::default() });
        let campus = poi_importance(PoiSignals { area_m2: 2.0e6, ..Default::default() });
        assert!(plain < wikidata && wikidata < famous);
        assert!(plain < campus);
        assert!(famous <= 1.0);
    }

    #[test]
    fn transit_stops_are_less_important() {
        let stop = poi_importance(PoiSignals { transit_stop: true, ..Default::default() });
        assert!(stop < poi_importance(PoiSignals::default()));
    }

    #[test]
    fn parses_wikidata_ids() {
        assert_eq!(wikidata_number("Q60"), Some(60));
        assert_eq!(wikidata_number("60"), None);
    }
}
