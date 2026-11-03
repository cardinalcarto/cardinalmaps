//! How prominent a place is regardless of any query, from 0 to 1. Search
//! multiplies text relevance by `1 + IMPORTANCE_WEIGHT * importance`, so
//! importance reorders matches but can't make a poor match outrank a good one
//! by much.

/// How much importance can raise a result's score.
pub const IMPORTANCE_WEIGHT: f32 = 2.0;

/// Whether a placetype is a settlement, where people live and go, rather
/// than an area that groups them. A settlement's population says how much
/// people look for it; an area's mostly says how many settlements it holds.
fn is_settlement(placetype: &str) -> bool {
    matches!(placetype, "locality" | "localadmin" | "borough" | "macrohood" | "neighbourhood" | "microhood")
}

/// The most importance an administrative area gets: about the top of what
/// POIs get, so even the biggest cities and countries only win queries they
/// match well, not every vague prefix the world over.
const MAX_ADMIN_IMPORTANCE: f64 = 0.6;

/// An administrative area's importance from its population, on the same
/// scale as POIs': a town of a thousand people ranks with an ordinary POI,
/// Seattle (about 750,000) around 0.5, above most POIs named after it, and
/// cities of a few million and up at [`MAX_ADMIN_IMPORTANCE`]. An area that groups settlements (a county, a
/// region, a country) counts a tenth of its population, since that's mostly
/// its settlements' and they shouldn't be outranked by it. Without a
/// population, Who's On First's `wof:population_rank` (0 to 18) stands in for
/// it; without either, an area ranks as an ordinary POI.
#[must_use]
pub fn admin_importance(placetype: &str, population: Option<u64>, population_rank: Option<u32>) -> f64 {
    // Rank 11 is Seattle's, 18 the largest megacities'.
    let decades = match (population, population_rank) {
        (Some(population), _) if population > 0 => (population as f64).log10(),
        (_, Some(rank)) => 0.5 * f64::from(rank.min(18)) + 0.3,
        _ => return 0.08,
    };
    let decades = if is_settlement(placetype) { decades } else { decades - 1.0 };
    // 0.1 at a thousand people, rising 0.14 a decade.
    (0.1 + 0.14 * (decades - 3.0)).clamp(0.05, MAX_ADMIN_IMPORTANCE)
}

/// A neighbourhood's importance, which is at least a share of its city's:
/// neighbourhoods rarely have populations, and a big city's are better known
/// than small towns elsewhere, though not than cities: a megacity's come to
/// about a city of 50,000.
#[must_use]
pub fn neighbourhood_importance(own: f64, city: f64) -> f64 {
    own.max(0.45 * city)
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

/// A postcode: above the shops and buildings named with one (`10119`), below
/// towns.
pub const POSTCODE_IMPORTANCE: f64 = 0.25;

/// A street, by its OSM `highway` class: above addresses on it, and more for
/// the bigger roads.
#[must_use]
pub fn street_importance(highway: Option<&str>) -> f64 {
    match highway {
        Some("motorway" | "trunk") => 0.25,
        Some("primary") => 0.2,
        Some("secondary") => 0.17,
        Some("tertiary") => 0.14,
        Some("pedestrian" | "living_street" | "residential" | "unclassified") => 0.1,
        _ => 0.06,
    }
}

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
        assert!(admin_importance("locality", Some(755_078), None) > admin_importance("locality", Some(5_000), None));
        assert!(admin_importance("locality", None, Some(11)) > admin_importance("locality", None, Some(5)));
    }

    #[test]
    fn a_county_doesnt_outrank_its_seat() {
        // Spokane County and Spokane.
        assert!(admin_importance("locality", Some(229_000), None) > admin_importance("county", Some(540_000), None));
    }

    #[test]
    fn a_big_citys_neighbourhoods_outrank_small_towns() {
        // Ballard, Seattle and Ballard, Utah.
        let ballard = neighbourhood_importance(
            admin_importance("neighbourhood", None, None),
            admin_importance("locality", Some(755_078), None),
        );
        assert!(ballard > admin_importance("locality", Some(1_500), None));
    }

    #[test]
    fn a_big_city_outranks_a_small_region() {
        // Portland, Oregon and Portland Parish, Jamaica.
        assert!(admin_importance("locality", Some(650_000), None) > admin_importance("region", Some(82_000), None));
    }

    #[test]
    fn a_megacitys_neighbourhoods_dont_outrank_cities() {
        // The neighbourhood in Manhattan also called Bellevue, and Bellevue, Washington.
        let manhattan = neighbourhood_importance(0.08, admin_importance("locality", Some(8_300_000), None));
        assert!(manhattan < admin_importance("locality", Some(150_000), None));
    }

    #[test]
    fn tiny_towns_rank_below_ordinary_pois_and_megacities_stay_in_range() {
        let cafe = poi_importance(PoiSignals::default());
        // Starbuck, Washington.
        assert!(admin_importance("locality", Some(130), None) < cafe);
        // Tokyo, and China: no more than a famous POI could get.
        let famous = poi_importance(PoiSignals { has_wikidata: true, qrank: Some(1_000_000_000), area_m2: 1e9, ..Default::default() });
        assert!(admin_importance("locality", Some(37_000_000), None) <= MAX_ADMIN_IMPORTANCE);
        assert!(admin_importance("country", Some(1_400_000_000), None) <= famous + 0.05);
        // Without a population, an ordinary POI's.
        assert!((admin_importance("county", None, None) - cafe).abs() < 0.01);
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
