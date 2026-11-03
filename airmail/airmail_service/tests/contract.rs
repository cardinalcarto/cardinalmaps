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
use axum::{body::Body, http::Request};
use geo::{Geometry, Point, Rect, coord};
use serde_json::Value;
use tower::ServiceExt;

const SEATTLE: u64 = 101_730_401;
const KING_COUNTY: u64 = 102_086_191;
const WASHINGTON: u64 = 85_688_623;
const ZURICH: u64 = 101_748_463;
const SWITZERLAND: u64 = 85_633_051;

/// The band of each fixture area.
fn band(id: u64) -> AdminBand {
    match id {
        SEATTLE | ZURICH => AdminBand::Locality,
        KING_COUNTY => AdminBand::County,
        WASHINGTON => AdminBand::Region,
        _ => AdminBand::Country,
    }
}

fn rect(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Geometry<f64> {
    Geometry::Polygon(Rect::new(coord! { x: min_x, y: min_y }, coord! { x: max_x, y: max_y }).to_polygon())
}

fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn admin(id: u64, placetype: &str, name: &str, abbreviation: Option<&str>, shape: Geometry<f64>, ancestors: &[(u64, &str)], population: u64) -> ToIndexPoi {
    let mut names = vec![name.to_string()];
    names.extend(abbreviation.map(str::to_string));
    let mut doc = ToIndexPoi::new(Kind::Admin, names, shape, Vec::new()).unwrap();
    doc.wof = Some(WofPlace {
        id,
        placetype: placetype.into(),
        population: Some(population),
        default_name: name.into(),
        names: [("eng".to_string(), name.to_string())].into(),
        abbreviation: abbreviation.map(str::to_string),
        country: Some("US".into()),
    });
    doc.admin_ids = ancestors.iter().map(|(id, _)| *id).collect();
    doc.admin_names = ancestors.iter().map(|(id, name)| (band(*id), name.to_string())).collect();
    doc.importance = importance::admin_importance(placetype, Some(population), None);
    doc
}

fn osm(kind: Kind, names: &[&str], shape: Geometry<f64>, pairs: &[(&str, &str)], osm: OsmRef) -> ToIndexPoi {
    let mut doc = ToIndexPoi::new(kind, names.iter().map(|n| n.to_string()).collect(), shape, tags(pairs)).unwrap();
    doc.osm = Some(osm);
    doc.admin_ids = vec![SEATTLE, KING_COUNTY, WASHINGTON];
    doc.admin_names = [(SEATTLE, "Seattle"), (KING_COUNTY, "King County"), (WASHINGTON, "Washington"), (WASHINGTON, "WA")]
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
    OsmRef { osm_type: OsmType::Node, id, version: Some(7) }
}

/// Zürich, in Switzerland, which only Who's On First places contain.
fn zurich() -> Vec<ToIndexPoi> {
    let mut switzerland = admin(SWITZERLAND, "country", "Switzerland", Some("CH"), rect(5.96, 45.82, 10.49, 47.81), &[], 8_800_000);
    let mut zurich = admin(ZURICH, "locality", "Zurich", None, rect(8.45, 47.32, 8.63, 47.43), &[(SWITZERLAND, "Switzerland")], 420_000);
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
        admin(WASHINGTON, "region", "Washington", Some("WA"), rect(-124.8, 45.5, -116.9, 49.0), &[], 7_700_000),
        admin(KING_COUNTY, "county", "King County", None, rect(-122.5, 47.1, -121.0, 47.8), &[washington], 2_300_000),
        admin(SEATTLE, "locality", "Seattle", None, rect(-122.44, 47.49, -122.24, 47.73), &[king, washington, (WASHINGTON, "WA")], 737_015),
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
            &[("amenity", "university"), ("name", "University of Washington"), ("short_name", "UW"), ("wikidata", "Q219563")],
            OsmRef { osm_type: OsmType::Relation, id: 5_843_919, version: Some(31) },
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
            &[("highway", "bus_stop"), ("name", "Seattle Center & Thomas St")],
            node(3),
        ),
        osm(
            Kind::Poi,
            &["Seattle Pacific University Station"],
            Geometry::Point(Point::new(-122.36, 47.65)),
            &[("public_transport", "platform"), ("name", "Seattle Pacific University Station")],
            node(4),
        ),
        // The same chain downtown and in Bellevue, and a weak nearby match
        // for a city far away.
        osm(
            Kind::Poi,
            &["Starbucks"],
            Geometry::Point(Point::new(-122.3365, 47.6101)),
            &[("amenity", "cafe"), ("name", "Starbucks"), ("brand", "Starbucks")],
            node(5),
        ),
        osm(
            Kind::Poi,
            &["Starbucks"],
            Geometry::Point(Point::new(-122.2005, 47.6168)),
            &[("amenity", "cafe"), ("name", "Starbucks"), ("brand", "Starbucks")],
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
    for (id, name) in [(8, "Кафе Пушкин"), (9, "Café Malmö"), (10, "Bäckerei Müller"), (11, "渋谷カフェ"), (12, "Rue de l'Arsenal Bistro")] {
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
            category_include_threshold: 0.4,
            schema_version: SCHEMA_VERSION,
            documents: 13,
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
        let mut found: Vec<_> = gazetteer.areas(q).into_iter().map(|a| (a.start, a.end, a.band, a.quality)).collect();
        found.sort_by_key(|(s, e, b, _)| (*s, *e, *b));
        found
    };
    let areas = found(&query(&["coffee", "seattle", "wa"], false));
    assert!(areas.contains(&(1, 2, "locality", Quality::Exact)), "{areas:?}");
    assert!(areas.contains(&(2, 3, "region", Quality::Exact)), "{areas:?}");
    assert!(!areas.iter().any(|a| a.0 == 0), "{areas:?}");
    assert!(found(&query(&["seatle"], false)).contains(&(0, 1, "locality", Quality::Typo)));
    assert!(found(&query(&["coffee", "seatt"], true)).contains(&(1, 2, "locality", Quality::Prefix)));
    assert!(found(&query(&["king", "county"], false)).contains(&(0, 2, "county", Quality::Exact)));
    // A country by its code.
    assert!(found(&query(&["zurich", "ch"], false)).contains(&(1, 2, "country", Quality::Exact)));
    // Importance and distance come with matches.
    let seattle = gazetteer.areas(&query(&["seattle"], false)).into_iter().find(|a| a.band == "locality").unwrap();
    assert!(seattle.importance > 0.3 && seattle.distance_km.unwrap() < 30.0, "{seattle:?}");
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
    let errors: Vec<String> = validator.iter_errors(value).map(|e| format!("{} at {}", e, e.instance_path())).collect();
    assert!(errors.is_empty(), "response doesn't match {name}:\n{}\n{value:#}", errors.join("\n"));
}

async fn get(uri: &str) -> (u16, String, Value) {
    let response = router().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let body = axum::body::to_bytes(response.into_body(), 10 << 20).await.unwrap();
    (status, content_type, serde_json::from_slice(&body).unwrap_or(Value::Null))
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
    let admins: Vec<&str> = seattle["admins"].as_array().unwrap().iter().map(|a| a["placetype"].as_str().unwrap()).collect();
    assert_eq!(admins, vec!["county", "region"]);
    assert_eq!(seattle["admins"][1]["abbreviation"], "WA");
    assert_eq!(seattle["feature"]["geometry"]["type"], "Polygon");
}

#[tokio::test]
async fn trailing_admin_words_qualify_a_name() {
    assert_eq!(ids(&search("q=seattle%20wa").await)[0], "wof:101730401");
    assert_eq!(ids(&search("q=coffee%20works%20seattle").await)[0], "poi:28349");
}

#[tokio::test]
async fn poi_results_carry_their_sources_apart() {
    let results = search("q=seattle%20coffee%20works").await;
    let cafe = &results[0];
    assert_eq!(cafe["id"], "poi:28349");
    assert_eq!(cafe["poi"], serde_json::json!({ "id": 28349, "version": 7 }));
    assert_eq!(cafe["osm"]["type"], "node");
    assert_eq!(cafe["osm"]["tags"]["amenity"], "cafe");
    assert!(cafe["osm"]["tags"].get("cardinal:poi_id").is_none());
    assert_eq!(cafe["address"]["street"], "Pike Street");
    assert!(cafe["feature"]["properties"].is_null());
    let admins: Vec<&str> = cafe["admins"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(admins, vec!["Seattle", "King County", "Washington"]);
}

#[tokio::test]
async fn names_are_found_in_their_script_and_forgivingly() {
    let first = |results: Vec<Value>| results.first().and_then(|r| r["name"].as_str().map(str::to_string));
    for (query, name) in [
        ("q=%D0%BA%D0%B0%D1%84%D0%B5%20%D0%BF%D1%83%D1%88%D0%BA%D0%B8%D0%BD", "Кафе Пушкин"),
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
    let corner = search("q=university%20of%20washington&bbox=-122.296,47.657,-122.290,47.661").await;
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
    assert!(search("q=seattle&kind=address").await.iter().all(|r| r["kind"] == "address"));
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
    assert_eq!(ids(&search("q=seattle%20cof&prefix=true").await)[0], "poi:28349");
    assert!(search("q=seatle").await.is_empty());
    assert!(search("q=seatle&prefix=true").await.is_empty());
    assert_eq!(ids(&search("q=seatle&typos=true").await)[0], "wof:101730401");
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
    let names: Vec<&str> = body["invalid_params"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["q", "bbox", "limit"]);
}

const DOWNTOWN: &str = "-122.3321,47.6062";
const BELLEVUE: &str = "-122.2015,47.6101";

#[tokio::test]
async fn focus_ranks_nearby_places_first() {
    assert_eq!(ids(&search(&format!("q=starbucks&focus={DOWNTOWN}")).await)[0], "osm:node/5");
    assert_eq!(ids(&search(&format!("q=starbucks&focus={BELLEVUE}")).await)[0], "osm:node/6");
    assert_eq!(ids(&search(&format!("q=starb&prefix=true&focus={BELLEVUE}")).await)[0], "osm:node/6");
}

#[tokio::test]
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
    assert_eq!(body["query"]["focus"], serde_json::json!([-122.2015, 47.6101]));
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
        assert_matches_schema(&body, "Problem");
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
    }
}

#[tokio::test]
async fn malformed_place_ids_are_problems() {
    for query in ["", "id=", "id=28349", "id=poi:x", "id=osm:area/1", "id=wof:1&geometry=line"] {
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
    let response = router().oneshot(Request::get("/openapi.yaml").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.headers()["content-type"], "application/yaml");
}
