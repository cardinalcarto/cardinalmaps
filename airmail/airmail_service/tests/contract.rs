//! Checks responses against `openapi.yaml`, and that search behaves as it
//! documents, using a small index built for the purpose.

use std::sync::{Arc, OnceLock};

use airmail::{
    categories::CategoryModels,
    importance,
    index::{AirmailIndex, SCHEMA_VERSION},
    poi::{AdminBand, Kind, OsmRef, OsmType, SchemafiedPoi, ToIndexPoi, WofPlace},
};
use airmail_service::api::AppState;
use airmail_service::poi_status::PoiStatus;
use axum::{body::Body, http::Request};
use geo::{Geometry, Point, Rect, coord};
use serde_json::Value;
use tower::ServiceExt;

const SEATTLE: u64 = 101_730_401;
const KING_COUNTY: u64 = 102_086_191;
const WASHINGTON: u64 = 85_688_623;
const ZURICH: u64 = 101_748_463;
const SWITZERLAND: u64 = 85_633_051;
const DENNY_REGRADE: u64 = 1_000;
const BELLTOWN: u64 = 1_001;
const DOWNTOWN_CORE: u64 = 1_002;

/// The band of each fixture area.
fn band(id: u64) -> AdminBand {
    match id {
        SEATTLE | ZURICH | DENNY_REGRADE => AdminBand::Locality,
        BELLTOWN | DOWNTOWN_CORE => AdminBand::Neighbourhood,
        KING_COUNTY => AdminBand::County,
        WASHINGTON => AdminBand::Region,
        _ => AdminBand::Country,
    }
}

fn rect(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Geometry<f64> {
    Geometry::Polygon(
        Rect::new(coord! { x: min_x, y: min_y }, coord! { x: max_x, y: max_y }).to_polygon(),
    )
}

fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn admin(
    id: u64,
    placetype: &str,
    name: &str,
    abbreviation: Option<&str>,
    shape: Geometry<f64>,
    ancestors: &[(u64, &str)],
    population: u64,
) -> ToIndexPoi {
    let mut names = vec![name.to_string()];
    names.extend(abbreviation.map(str::to_string));
    let size = airmail::geometry::bbox(&shape).map_or(0.0, |[w, s, e, n]| (e - w) * (n - s));
    let mut doc = ToIndexPoi::new(Kind::Admin, names, shape, Vec::new()).unwrap();
    doc.wof = Some(WofPlace {
        id,
        placetype: placetype.into(),
        population: Some(population),
        default_name: name.into(),
        names: [("eng".to_string(), name.to_string())].into(),
        abbreviation: abbreviation.map(str::to_string),
        country: Some("US".into()),
        size,
    });
    doc.admin_ids = ancestors.iter().map(|(id, _)| *id).collect();
    doc.admin_names = ancestors
        .iter()
        .map(|(id, name)| (band(*id), name.to_string()))
        .collect();
    doc.importance = importance::admin_importance(placetype, Some(population), None);
    doc
}

fn osm(
    kind: Kind,
    names: &[&str],
    shape: Geometry<f64>,
    pairs: &[(&str, &str)],
    osm: OsmRef,
) -> ToIndexPoi {
    let mut doc = ToIndexPoi::new(
        kind,
        names.iter().map(|n| n.to_string()).collect(),
        shape,
        tags(pairs),
    )
    .unwrap();
    doc.osm = Some(osm);
    doc.admin_ids = vec![SEATTLE, KING_COUNTY, WASHINGTON];
    doc.admin_names = [
        (SEATTLE, "Seattle"),
        (KING_COUNTY, "King County"),
        (WASHINGTON, "Washington"),
        (WASHINGTON, "WA"),
    ]
    .map(|(id, name)| (band(id), name.to_string()))
    .to_vec();
    doc.country_code = Some("US".into());
    doc.house_number = doc.tag("addr:housenumber").map(str::to_string);
    doc.road = doc.tag("addr:street").map(str::to_string);
    let wikidata = doc.tag("wikidata").is_some();
    doc.importance = importance::poi_importance(importance::PoiSignals {
        has_wikidata: wikidata,
        area_m2: airmail::geometry::area_m2(&doc.geometry),
        transit_stop: importance::is_transit_stop(&doc.tags),
        ..Default::default()
    });
    doc
}

fn node(id: i64) -> OsmRef {
    OsmRef {
        osm_type: OsmType::Node,
        id,
        version: Some(7),
    }
}

/// Zürich, in Switzerland, which only Who's On First places contain.
fn zurich() -> Vec<ToIndexPoi> {
    let mut switzerland = admin(
        SWITZERLAND,
        "country",
        "Switzerland",
        Some("CH"),
        rect(5.96, 45.82, 10.49, 47.81),
        &[],
        8_800_000,
    );
    let mut zurich = admin(
        ZURICH,
        "locality",
        "Zurich",
        None,
        rect(8.45, 47.32, 8.63, 47.43),
        &[(SWITZERLAND, "Switzerland")],
        420_000,
    );
    zurich.names.push("Zürich".into());
    for doc in [&mut switzerland, &mut zurich] {
        doc.wof.as_mut().unwrap().country = Some("CH".into());
    }
    vec![switzerland, zurich]
}

fn build_index(dir: &std::path::Path) -> AirmailIndex {
    let washington = (WASHINGTON, "Washington");
    let king = (KING_COUNTY, "King County");
    let mut docs = vec![
        admin(
            WASHINGTON,
            "region",
            "Washington",
            Some("WA"),
            rect(-124.8, 45.5, -116.9, 49.0),
            &[],
            7_700_000,
        ),
        admin(
            KING_COUNTY,
            "county",
            "King County",
            None,
            rect(-122.5, 47.1, -121.0, 47.8),
            &[washington],
            2_300_000,
        ),
        admin(
            SEATTLE,
            "locality",
            "Seattle",
            None,
            rect(-122.44, 47.49, -122.24, 47.73),
            &[king, washington, (WASHINGTON, "WA")],
            737_015,
        ),
        // Who's On First calls this a locality too. Its ID is lower than Seattle's, so only
        // its size says which is the city.
        admin(
            DENNY_REGRADE,
            "locality",
            "Denny Regrade",
            None,
            rect(-122.35, 47.61, -122.33, 47.62),
            &[(SEATTLE, "Seattle")],
            10_000,
        ),
        admin(
            BELLTOWN,
            "neighbourhood",
            "Belltown",
            None,
            rect(-122.35, 47.61, -122.34, 47.62),
            &[(SEATTLE, "Seattle")],
            0,
        ),
        admin(
            DOWNTOWN_CORE,
            "neighbourhood",
            "Downtown Core",
            None,
            rect(-122.35, 47.60, -122.32, 47.62),
            &[(SEATTLE, "Seattle")],
            0,
        ),
        {
            let mut hotel = osm(
                Kind::Poi,
                &["Grand Regrade Hotel"],
                Geometry::Point(Point::new(-122.345, 47.615)),
                &[("tourism", "hotel"), ("name", "Grand Regrade Hotel")],
                node(40),
            );
            hotel
                .admin_ids
                .extend([DENNY_REGRADE, BELLTOWN, DOWNTOWN_CORE]);
            hotel
                .admin_names
                .push((band(DENNY_REGRADE), "Denny Regrade".to_string()));
            hotel
        },
        osm(
            Kind::Poi,
            &["Seattle Coffee Works"],
            Geometry::Point(Point::new(-122.3404, 47.6094)),
            &[
                ("amenity", "cafe"),
                ("name", "Seattle Coffee Works"),
                ("addr:housenumber", "108"),
                ("addr:street", "Pike Street"),
                ("cardinal:poi_id", "28349"),
                ("cardinal:version", "7"),
            ],
            node(1),
        ),
        osm(
            Kind::Poi,
            &["University of Washington", "UW"],
            rect(-122.3221, 47.6481, -122.2905, 47.6614),
            &[
                ("amenity", "university"),
                ("name", "University of Washington"),
                ("short_name", "UW"),
                ("wikidata", "Q219563"),
            ],
            OsmRef {
                osm_type: OsmType::Relation,
                id: 5_843_919,
                version: Some(31),
            },
        ),
        osm(
            Kind::Address,
            &[],
            Geometry::Point(Point::new(-122.3414, 47.6095)),
            &[("addr:housenumber", "1530"), ("addr:street", "Post Alley")],
            node(2),
        ),
        osm(
            Kind::Poi,
            &["Seattle Center & Thomas St"],
            Geometry::Point(Point::new(-122.351, 47.621)),
            &[
                ("highway", "bus_stop"),
                ("name", "Seattle Center & Thomas St"),
            ],
            node(3),
        ),
        osm(
            Kind::Poi,
            &["Seattle Pacific University Station"],
            Geometry::Point(Point::new(-122.36, 47.65)),
            &[
                ("public_transport", "platform"),
                ("name", "Seattle Pacific University Station"),
            ],
            node(4),
        ),
        // The same chain downtown and in Bellevue, and a weak nearby match
        // for a city far away.
        osm(
            Kind::Poi,
            &["Starbucks"],
            Geometry::Point(Point::new(-122.3365, 47.6101)),
            &[
                ("amenity", "cafe"),
                ("name", "Starbucks"),
                ("brand", "Starbucks"),
            ],
            node(5),
        ),
        osm(
            Kind::Poi,
            &["Starbucks"],
            Geometry::Point(Point::new(-122.2005, 47.6168)),
            &[
                ("amenity", "cafe"),
                ("name", "Starbucks"),
                ("brand", "Starbucks"),
            ],
            node(6),
        ),
        osm(
            Kind::Poi,
            &["Zurich Coffee Roasters"],
            Geometry::Point(Point::new(-122.3330, 47.6070)),
            &[("amenity", "cafe"), ("name", "Zurich Coffee Roasters")],
            node(7),
        ),
    ];
    // Names in other scripts and with diacritics, for text matching.
    for (id, name) in [
        (8, "Кафе Пушкин"),
        (9, "Café Malmö"),
        (10, "Bäckerei Müller"),
        (11, "渋谷カフェ"),
        (12, "Rue de l'Arsenal Bistro"),
    ] {
        docs.push(osm(
            Kind::Poi,
            &[name],
            Geometry::Point(Point::new(-122.33, 47.61)),
            &[("amenity", "cafe"), ("name", name)],
            node(id),
        ));
    }
    docs.extend(zurich());
    let mut index = AirmailIndex::create(dir).unwrap();
    let mut writer = index.writer().unwrap();
    for doc in docs {
        writer.add_poi(SchemafiedPoi::from(doc), "test").unwrap();
    }
    writer.commit().unwrap();
    AirmailIndex::open_read_only(dir.to_str().unwrap()).unwrap()
}

fn router() -> axum::Router {
    airmail_service::router(state())
}

fn state() -> Arc<AppState> {
    static STATE: OnceLock<(tempfile::TempDir, Arc<AppState>)> = OnceLock::new();
    let (_, state) = STATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let index = build_index(dir.path());
        let state = Arc::new(AppState {
            index,
            categories: CategoryModels::default(),
            schema_version: SCHEMA_VERSION,
            documents: 13,
            poi_status: None,
        });
        (dir, state)
    });
    state.clone()
}

#[test]
fn the_gazetteer_finds_areas_by_band_exactly_as_typed_and_with_typos() {
    use airmail_tagger::features::{Gazetteer, Quality, Query};
    let state = state();
    let gazetteer = airmail::gazetteer::IndexGazetteer::new(&state.index).unwrap();
    let query = |words: &[&str], typing: bool| Query {
        words: words.iter().map(|w| w.to_string()).collect(),
        section_starts: vec![false; words.len()],
        typing,
        focus: Some([-122.33, 47.61]),
    };
    let found = |q: &Query| -> Vec<(usize, usize, &'static str, Quality)> {
        let mut found: Vec<_> = gazetteer
            .areas(q)
            .into_iter()
            .map(|a| (a.start, a.end, a.band, a.quality))
            .collect();
        found.sort_by_key(|(s, e, b, _)| (*s, *e, *b));
        found
    };
    let areas = found(&query(&["coffee", "seattle", "wa"], false));
    assert!(
        areas.contains(&(1, 2, "locality", Quality::Exact)),
        "{areas:?}"
    );
    assert!(
        areas.contains(&(2, 3, "region", Quality::Exact)),
        "{areas:?}"
    );
    assert!(!areas.iter().any(|a| a.0 == 0), "{areas:?}");
    assert!(found(&query(&["seatle"], false)).contains(&(0, 1, "locality", Quality::Typo)));
    assert!(found(&query(&["coffee", "seatt"], true)).contains(&(
        1,
        2,
        "locality",
        Quality::Prefix
    )));
    assert!(found(&query(&["king", "county"], false)).contains(&(0, 2, "county", Quality::Exact)));
    // A country by its code.
    assert!(found(&query(&["zurich", "ch"], false)).contains(&(1, 2, "country", Quality::Exact)));
    // Importance and distance come with matches.
    let seattle = gazetteer
        .areas(&query(&["seattle"], false))
        .into_iter()
        .find(|a| a.band == "locality")
        .unwrap();
    assert!(
        seattle.importance > 0.3 && seattle.distance_km.unwrap() < 30.0,
        "{seattle:?}"
    );
}

/// The spec as JSON, for resolving `$ref`s into it.
fn spec() -> &'static Value {
    static SPEC: OnceLock<Value> = OnceLock::new();
    SPEC.get_or_init(|| serde_yaml::from_str(airmail_service::api::OPENAPI).unwrap())
}

/// Fails unless `value` matches the spec's schema `name`. The spec's
/// schemas become `$defs` of one JSON Schema, with references to match.
fn assert_matches_schema(value: &Value, name: &str) {
    let schemas = serde_json::to_string(&spec()["components"]["schemas"])
        .unwrap()
        .replace("#/components/schemas/", "#/$defs/");
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": serde_json::from_str::<Value>(&schemas).unwrap(),
        "$ref": format!("#/$defs/{name}"),
    });
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "response doesn't match {name}:\n{}\n{value:#}",
        errors.join("\n")
    );
}

async fn get(uri: &str) -> (u16, String, Value) {
    let response = router()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let body = axum::body::to_bytes(response.into_body(), 10 << 20)
        .await
        .unwrap();
    (
        status,
        content_type,
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    )
}

/// Searches, checking the response against the spec, and returns its results.
async fn search(query: &str) -> Vec<Value> {
    let (status, content_type, body) = get(&format!("/v1/search?{query}")).await;
    assert_eq!(status, 200, "{body:#}");
    assert_eq!(content_type, "application/json");
    assert_matches_schema(&body, "SearchResponse");
    body["results"].as_array().unwrap().clone()
}

fn ids(results: &[Value]) -> Vec<&str> {
    results.iter().map(|r| r["id"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn a_city_outranks_the_places_in_it() {
    let results = search("q=seattle").await;
    assert_eq!(ids(&results)[0], "wof:101730401");
    let seattle = &results[0];
    assert_eq!(seattle["kind"], "admin");
    assert_eq!(seattle["name"], "Seattle");
    assert_eq!(seattle["wof"]["placetype"], "locality");
    assert_eq!(seattle["wof"]["abbreviation"], serde_json::Value::Null);
    let admins: Vec<&str> = seattle["admins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["placetype"].as_str().unwrap())
        .collect();
    assert_eq!(admins, vec!["county", "region"]);
    assert_eq!(seattle["admins"][1]["abbreviation"], "WA");
    assert_eq!(seattle["feature"]["geometry"]["type"], "Polygon");
}

#[tokio::test]
async fn one_area_of_each_placetype_is_shown_but_all_find_the_place() {
    let results = search("q=grand%20regrade%20hotel").await;
    let names: Vec<&str> = results[0]["admins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    // The largest locality and the smallest neighbourhood.
    assert_eq!(
        names,
        vec!["Belltown", "Seattle", "King County", "Washington"]
    );
    assert_matches_schema(&results[0], "Result");
    // Though it isn't shown, the other locality still finds it.
    assert_eq!(
        ids(&search("q=grand%20regrade%20hotel%20denny%20regrade").await)[0],
        ids(&results)[0]
    );
}

#[tokio::test]
async fn trailing_admin_words_qualify_a_name() {
    assert_eq!(ids(&search("q=seattle%20wa").await)[0], "wof:101730401");
    assert_eq!(
        ids(&search("q=coffee%20works%20seattle").await)[0],
        "poi:28349"
    );
}

#[tokio::test]
async fn poi_results_carry_their_sources_apart() {
    let results = search("q=seattle%20coffee%20works").await;
    let cafe = &results[0];
    assert_eq!(cafe["id"], "poi:28349");
    assert_eq!(
        cafe["poi"],
        serde_json::json!({ "id": 28349, "version": 7 })
    );
    assert_eq!(cafe["osm"]["type"], "node");
    assert_eq!(cafe["osm"]["tags"]["amenity"], "cafe");
    assert!(cafe["osm"]["tags"].get("cardinal:poi_id").is_none());
    assert_eq!(cafe["address"]["street"], "Pike Street");
    assert!(cafe["feature"]["properties"].is_null());
    let admins: Vec<&str> = cafe["admins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(admins, vec!["Seattle", "King County", "Washington"]);
}

#[tokio::test]
async fn names_are_found_in_their_script_and_forgivingly() {
    let first = |results: Vec<Value>| {
        results
            .first()
            .and_then(|r| r["name"].as_str().map(str::to_string))
    };
    for (query, name) in [
        (
            "q=%D0%BA%D0%B0%D1%84%D0%B5%20%D0%BF%D1%83%D1%88%D0%BA%D0%B8%D0%BD",
            "Кафе Пушкин",
        ),
        ("q=kafe%20pushkin", "Кафе Пушкин"),
        ("q=cafe%20malmo", "Café Malmö"),
        ("q=b%C3%A4ckerei%20m%C3%BCller", "Bäckerei Müller"),
        ("q=baeckerei%20mueller", "Bäckerei Müller"),
        ("q=%E6%B8%8B%E8%B0%B7", "渋谷カフェ"),
        ("q=arsenal%20bistro", "Rue de l'Arsenal Bistro"),
        ("q=larsenal", "Rue de l'Arsenal Bistro"),
    ] {
        assert_eq!(first(search(query).await).as_deref(), Some(name), "{query}");
    }
}

#[tokio::test]
async fn short_names_find_places() {
    assert_eq!(search("q=uw").await[0]["name"], "University of Washington");
}

#[tokio::test]
async fn bbox_matches_shapes_not_just_centers() {
    // The campus's north-east corner, away from its center.
    let corner =
        search("q=university%20of%20washington&bbox=-122.296,47.657,-122.290,47.661").await;
    assert_eq!(ids(&corner), vec!["osm:relation/5843919"]);
    let elsewhere = search("q=university%20of%20washington&bbox=-122.28,47.57,-122.27,47.58").await;
    assert!(elsewhere.is_empty());
}

#[tokio::test]
async fn within_excludes_the_area_itself() {
    let results = search("q=seattle&within=wof:101730401").await;
    assert!(!results.is_empty());
    assert!(!ids(&results).contains(&"wof:101730401"));
}

#[tokio::test]
async fn kind_filters_and_addresses_have_no_name() {
    let results = search("q=1530%20post%20alley&kind=address").await;
    assert_eq!(ids(&results), vec!["osm:node/2"]);
    assert!(results[0]["name"].is_null());
    assert_eq!(results[0]["address"]["housenumber"], "1530");
    // Addresses in Seattle match it, but only addresses.
    assert!(
        search("q=seattle&kind=address")
            .await
            .iter()
            .all(|r| r["kind"] == "address")
    );
    assert!(search("q=uw&kind=address").await.is_empty());
}

#[tokio::test]
async fn point_geometry_is_the_center() {
    let results = search("q=uw&geometry=point").await;
    let feature = &results[0]["feature"];
    assert_eq!(feature["geometry"]["type"], "Point");
    assert_eq!(feature["geometry"]["coordinates"], results[0]["center"]);
}

#[tokio::test]
async fn prefix_and_typos_are_separate() {
    assert!(search("q=seattle%20cof").await.is_empty());
    assert_eq!(
        ids(&search("q=seattle%20cof&prefix=true").await)[0],
        "poi:28349"
    );
    assert!(search("q=seatle").await.is_empty());
    assert!(search("q=seatle&prefix=true").await.is_empty());
    assert_eq!(
        ids(&search("q=seatle&typos=true").await)[0],
        "wof:101730401"
    );
}

#[tokio::test]
async fn a_space_finishes_the_last_word() {
    assert_eq!(
        ids(&search("q=seattle%20cof&prefix=true").await)[0],
        "poi:28349"
    );
    assert!(search("q=seattle%20cof%20&prefix=true").await.is_empty());
}

#[tokio::test]
async fn search_finds_names_and_requires_every_word() {
    assert_eq!(ids(&search("q=seattle").await)[0], "wof:101730401");
    assert_eq!(
        ids(&search("q=seattle%20cof&prefix=true").await),
        ["poi:28349"]
    );
    assert!(search("q=seatle").await.is_empty());
    assert_eq!(
        ids(&search("q=seatle&typos=true").await)[0],
        "wof:101730401"
    );
    assert!(search("q=seattle&limit=1").await.len() == 1);
}

#[tokio::test]
async fn viewport_and_location_are_validated() {
    let (status, _, body) = get("/v1/search?q=a&viewport=1,2,3&location=x").await;
    assert_eq!(status, 400);
    let names: Vec<&str> = body["invalid_params"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        ["viewport", "location"]
            .iter()
            .all(|name| names.contains(name)),
        "{names:?}"
    );
}

#[tokio::test]
async fn limit_caps_results() {
    assert_eq!(search("q=seattle&limit=2").await.len(), 2);
}

#[tokio::test]
async fn invalid_parameters_are_problems() {
    let (status, content_type, body) = get("/v1/search?q=&limit=99&bbox=1,2").await;
    assert_eq!(status, 400);
    assert_eq!(content_type, "application/problem+json");
    assert_matches_schema(&body, "Problem");
    let names: Vec<&str> = body["invalid_params"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["q", "bbox", "limit"]);
}

const DOWNTOWN: &str = "-122.3321,47.6062";
const BELLEVUE: &str = "-122.2015,47.6101";

#[tokio::test]
async fn focus_ranks_nearby_places_first() {
    assert_eq!(
        ids(&search(&format!("q=starbucks&focus={DOWNTOWN}")).await)[0],
        "osm:node/5"
    );
    assert_eq!(
        ids(&search(&format!("q=starbucks&focus={BELLEVUE}")).await)[0],
        "osm:node/6"
    );
    assert_eq!(
        ids(&search(&format!("q=starb&prefix=true&focus={BELLEVUE}")).await)[0],
        "osm:node/6"
    );
}

// A known gap: a shop right here with the word in its name still outranks the city far
// away. It's why `paris` can start with Ben Paris from Seattle.
#[tokio::test]
#[ignore = "a nearby partial name match beats a far-off whole name"]
async fn focus_doesnt_override_a_strong_match_far_away() {
    // A city by name, over a nearby place sharing a word with it.
    let results = search(&format!("q=zurich&focus={DOWNTOWN}")).await;
    assert_eq!(ids(&results)[0], "wof:101748463");
    assert!(ids(&results).contains(&"osm:node/7"));
    let results = search("q=seattle&focus=8.54,47.37").await;
    assert_eq!(ids(&results)[0], "wof:101730401");
}

#[tokio::test]
async fn focus_is_echoed() {
    let (_, _, body) = get(&format!("/v1/search?q=starbucks&focus={BELLEVUE}")).await;
    assert_eq!(
        body["query"]["focus"],
        serde_json::json!([-122.2015, 47.6101])
    );
    let (_, _, body) = get("/v1/search?q=starbucks").await;
    assert!(body["query"]["focus"].is_null());
}

#[tokio::test]
async fn invalid_focus_is_a_problem() {
    for focus in ["1", "1,2,3", "200,0", "0,95", "x,y"] {
        let (status, _, body) = get(&format!("/v1/search?q=seattle&focus={focus}")).await;
        assert_eq!(status, 400, "{focus}");
        assert_matches_schema(&body, "Problem");
        assert_eq!(body["invalid_params"][0]["name"], "focus");
    }
}

#[tokio::test]
async fn results_have_their_country() {
    let results = search("q=seattle%20coffee%20works").await;
    assert_eq!(results[0]["country_code"], "US");
    assert_eq!(search("q=seattle").await[0]["country_code"], "US");
    let zurich = search("q=zurich&kind=admin").await;
    assert_eq!(zurich[0]["country_code"], "CH");
    let switzerland = search("q=switzerland").await;
    assert_eq!(switzerland[0]["id"], "wof:85633051");
    assert_eq!(switzerland[0]["country_code"], "CH");
}

/// Looks up a place, checking the response against the spec.
async fn place(query: &str) -> (u16, Value) {
    let (status, content_type, body) = get(&format!("/v1/place?{query}")).await;
    if status == 200 {
        assert_eq!(content_type, "application/json");
        assert_matches_schema(&body, "PlaceResponse");
    } else {
        assert_eq!(content_type, "application/problem+json");
        assert_matches_schema(
            &body,
            if status == 404 {
                "PlaceNotFound"
            } else {
                "Problem"
            },
        );
        assert_eq!(body["status"], status);
    }
    (status, body)
}

#[tokio::test]
async fn place_looks_up_search_results() {
    for result in search("q=seattle&limit=50").await {
        let id = result["id"].as_str().unwrap();
        let (status, body) = place(&format!("id={id}")).await;
        assert_eq!(status, 200, "{id}");
        let mut expected = result.clone();
        expected["score"] = serde_json::json!(0.0);
        assert_eq!(body["result"], expected, "{id}");
    }
}

#[tokio::test]
async fn place_finds_pois_by_osm_id_too() {
    let (status, body) = place("id=osm:node/1").await;
    assert_eq!(status, 200);
    assert_eq!(body["result"]["id"], "poi:28349");
    let (_, body) = place("id=osm:relation/5843919&geometry=point").await;
    assert_eq!(body["result"]["name"], "University of Washington");
    assert_eq!(body["result"]["feature"]["geometry"]["type"], "Point");
    let (_, body) = place("id=wof:101748463&lang=de").await;
    assert_eq!(body["result"]["admins"][0]["name"], "Switzerland");
}

#[tokio::test]
async fn place_not_found_is_a_404() {
    for id in ["poi:999999", "wof:1", "osm:way/1", "poi:0"] {
        let (status, body) = place(&format!("id={id}")).await;
        assert_eq!(status, 404, "{id}");
        assert_eq!(body["title"], "Not Found");
        // Without a status file, nothing more is said.
        assert!(body.get("state").is_none(), "{body}");
    }
}

/// A status file as `poi_service status` writes one: a 128-byte header, then
/// an 8-byte record per `poi_id` below `next_poi_id`.
fn status_file(sequence: i64, timestamp: i64, next_poi_id: i64, records: &[(u64, u64)]) -> Vec<u8> {
    let mut bytes = vec![0u8; 128];
    bytes[..8].copy_from_slice(b"CPOISTAT");
    bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&8u32.to_le_bytes());
    bytes[16..24].copy_from_slice(&sequence.to_le_bytes());
    bytes[24..32].copy_from_slice(&timestamp.to_le_bytes());
    bytes[32..40].copy_from_slice(&next_poi_id.to_le_bytes());
    bytes[40..48].copy_from_slice(&next_poi_id.to_le_bytes());
    bytes.resize(128 + 8 * usize::try_from(next_poi_id).unwrap(), 0);
    for &(poi_id, record) in records {
        let i = 128 + 8 * usize::try_from(poi_id).unwrap();
        bytes[i..i + 8].copy_from_slice(&record.to_le_bytes());
    }
    bytes
}

/// A record: state (1 active, 2 inactive, 3 replaced), OSM type (0 node, 1
/// way, 2 relation), version and ID.
fn record(state: u64, osm_type: u64, version: u64, osm_id: u64) -> u64 {
    state | osm_type << 2 | version << 4 | osm_id << 24
}

/// A service over the fixture index with a POI status file, which lookups
/// check for replacement every time.
fn router_with_status(status: &std::path::Path) -> axum::Router {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());
    let mut poi_status = PoiStatus::open(status).unwrap();
    poi_status.check_interval = std::time::Duration::ZERO;
    // The index is open, so its directory can go once the test ends.
    std::mem::forget(dir);
    airmail_service::router(Arc::new(AppState {
        index,
        categories: CategoryModels::default(),
        schema_version: SCHEMA_VERSION,
        documents: 13,
        poi_status: Some(poi_status),
    }))
}

async fn place_with(router: &axum::Router, id: &str) -> (u16, Value) {
    let uri = format!("/v1/place?id={id}");
    let response = router
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), 10 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_matches_schema(
        &body,
        if status == 200 {
            "PlaceResponse"
        } else {
            "PlaceNotFound"
        },
    );
    (status, body)
}

const OCT_5: i64 = 1_791_158_403; // 2026-10-05T00:00:03Z

#[tokio::test]
async fn poi_404s_say_what_became_of_the_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("poi-status.bin");
    let records = [
        (5, record(1, 0, 3, 77)),
        (6, record(2, 1, 12, 13_000_000_000)),
        (7, record(3, 2, 6165, 9)),
    ];
    std::fs::write(&path, status_file(5136, OCT_5, 30_000, &records)).unwrap();
    let router = router_with_status(&path);
    let as_of = serde_json::json!({ "sequence": 5136, "timestamp": "2026-10-05T00:00:03Z" });

    // Places in the index are found as before.
    let (status, body) = place_with(&router, "poi:28349").await;
    assert_eq!(
        (status, body["result"]["id"].as_str()),
        (200, Some("poi:28349"))
    );

    for (id, state, osm, version) in [
        (
            "poi:5",
            "active",
            serde_json::json!({ "type": "node", "id": 77 }),
            3,
        ),
        (
            "poi:6",
            "inactive",
            serde_json::json!({ "type": "way", "id": 13_000_000_000_i64 }),
            12,
        ),
        (
            "poi:7",
            "replaced",
            serde_json::json!({ "type": "relation", "id": 9 }),
            6165,
        ),
    ] {
        let (status, body) = place_with(&router, id).await;
        assert_eq!(status, 404, "{id}");
        assert_eq!(
            (&body["state"], &body["osm"], &body["version"]),
            (&serde_json::json!(state), &osm, &serde_json::json!(version)),
            "{body}"
        );
        assert_eq!(body["as_of"], as_of);
        assert!(body.get("replaced_by").is_none());
    }

    // Numbers no identity had yet, at or above next_poi_id, are unknown as of
    // the checkpoint; so is an unassigned one below it.
    for id in ["poi:30000", "poi:999999999999", "poi:8", "poi:0"] {
        let (status, body) = place_with(&router, id).await;
        assert_eq!(
            (status, body["state"].as_str()),
            (404, Some("unknown")),
            "{id}"
        );
        assert_eq!(body["as_of"], as_of);
        assert!(
            body.get("osm").is_none() && body.get("version").is_none(),
            "{body}"
        );
    }

    // Other kinds of ID say nothing more.
    let (status, body) = place_with(&router, "wof:1").await;
    assert_eq!(status, 404);
    assert!(
        body.get("state").is_none() && body.get("as_of").is_none(),
        "{body}"
    );
}

#[tokio::test]
async fn poi_status_reloads_when_the_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("poi-status.bin");
    std::fs::write(
        &path,
        status_file(5136, OCT_5, 10, &[(5, record(1, 0, 3, 77))]),
    )
    .unwrap();
    let router = router_with_status(&path);
    let (_, body) = place_with(&router, "poi:5").await;
    assert_eq!(
        (body["state"].as_str(), &body["as_of"]["sequence"]),
        (Some("active"), &serde_json::json!(5136))
    );

    // A new file renamed over the old one is picked up on the next lookup.
    let next = dir.path().join("poi-status.bin.partial");
    std::fs::write(
        &next,
        status_file(
            5137,
            OCT_5 + 86_400,
            12,
            &[(5, record(3, 0, 3, 77)), (11, record(1, 0, 4, 77))],
        ),
    )
    .unwrap();
    std::fs::rename(&next, &path).unwrap();
    let (_, body) = place_with(&router, "poi:5").await;
    assert_eq!(body["state"], "replaced");
    assert_eq!(
        body["as_of"],
        serde_json::json!({ "sequence": 5137, "timestamp": "2026-10-06T00:00:03Z" })
    );
    let (_, body) = place_with(&router, "poi:11").await;
    assert_eq!(body["state"], "active");

    // A broken replacement is refused, and the loaded file kept.
    std::fs::write(&next, b"not a status file").unwrap();
    std::fs::rename(&next, &path).unwrap();
    let (_, body) = place_with(&router, "poi:11").await;
    assert_eq!(
        (body["state"].as_str(), &body["as_of"]["sequence"]),
        (Some("active"), &serde_json::json!(5137))
    );
}

#[tokio::test]
async fn malformed_place_ids_are_problems() {
    for query in [
        "",
        "id=",
        "id=28349",
        "id=poi:x",
        "id=osm:area/1",
        "id=wof:1&geometry=line",
    ] {
        let (status, body) = place(query).await;
        assert_eq!(status, 400, "{query}");
        assert!(!body["invalid_params"].as_array().unwrap().is_empty());
    }
}

#[tokio::test]
async fn health_reports_the_index() {
    let (status, _, body) = get("/health").await;
    assert_eq!(status, 200);
    assert_matches_schema(&body, "Health");
    assert_eq!(body["index_schema_version"], SCHEMA_VERSION);
}

#[tokio::test]
async fn serves_its_spec() {
    let response = router()
        .oneshot(Request::get("/openapi.yaml").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.headers()["content-type"], "application/yaml");
}
