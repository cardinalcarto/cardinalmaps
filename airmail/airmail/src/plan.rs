//! Search, as `docs/query-design.md` describes it. The tagger's readings say
//! what's sought and where; each reading is searched by fixed rules; and one
//! formula with bounded terms ranks what they retrieve:
//!
//! ```text
//! score    = P(reading) × candidate × match × mass × nearness
//! match    = coverage × rarity, by name; CATEGORY_MATCH × p / top, by category
//! mass     = 1 + MASS_WEIGHT × importance
//! nearness = 1 / (1 + distance / reach)
//! reach    = scale × 10^(REACH_POWER × importance)
//! ```
//!
//! Around a neighbourhood or locality, `candidate` is discounted by `OUTSIDE`.
//! A place keeps its best score.

use std::collections::{BTreeSet, HashMap, hash_map::Entry};
use std::rc::Rc;
use std::sync::Arc;

use airmail_tagger::labels::Tag;
use anyhow::Result;
use geo::Rect;
use log::trace;
use tantivy::{
    DocAddress, Searcher, Term,
    collector::{Count, TopDocs},
    query::{BooleanQuery, ConstScoreQuery, DisjunctionMaxQuery, FuzzyTermQuery, Occur, Query, QueryClone, TermQuery},
    schema::IndexRecordOption,
};

use crate::categories::{self, QueryCategories};
use crate::geometry;
use crate::index::{
    AirmailIndex, EARTH_RADIUS_KM, FIELD_ADMIN_IDS, FIELD_AREA_NAME, FIELD_CENTER, FIELD_HOUSENUMBER, FIELD_IMPORTANCE, FIELD_INDEXED_TAG,
    FIELD_KIND, FIELD_NAME, FIELD_POSTCODE, FIELD_STREET, FIELD_UNIT, SearchHit, SearchRequest, SearchResults, admin_field, area_key,
};
use crate::poi::{AdminBand, Kind};
use crate::within::Within;

// How much is searched.
/// The readings asked of the tagger, to be grouped.
const MAX_READINGS: usize = 8;
const READING_COVERAGE: f64 = 0.98;
/// The most reading groups searched, and the belief they're searched until.
const MAX_GROUPS: usize = 3;
const GROUP_COVERAGE: f64 = 0.9;
/// The most candidate areas searched, and the smallest share of their weight
/// one may have.
const MAX_CANDIDATES: usize = 3;
const MIN_CANDIDATE_SHARE: f64 = 0.05;
/// How many results each search contributes, beyond the limit.
const EXTRA_RESULTS: usize = 20;
/// Searching further and further from the anchor for a category.
const RING_FACTORS: [f64; 3] = [1.0, 8.0, 64.0];
/// How far outside a neighbourhood's or locality's box places are found.
const AREA_MARGIN_KM: f64 = 5.0;

// How it's ranked.
/// For a place around a candidate but not in it.
const OUTSIDE: f64 = 0.5;
const MASS_WEIGHT: f64 = 2.0;
const REACH_POWER: f64 = 5.0;
const MIN_SCALE_KM: f64 = 5.0;
/// The most a category match counts: a whole name counts for more.
const CATEGORY_MATCH: f32 = 0.7;
/// `rarity = 1 + RARITY_WEIGHT × log10(max(1, RARITY_COMMON_SHARE / share))`,
/// where `share` is the share of documents with all the words in their names.
const RARITY_WEIGHT: f32 = 15.0;
const RARITY_COMMON_SHARE: f64 = 4e-6;
/// `coverage` when a place was found without the words in any of its names, as
/// by a transliteration or a typo.
const UNSEEN_COVERAGE: f32 = 0.5;
const DIET_BONUS: f32 = 1.2;
/// For matches found only with typos, and only by words in any field.
const TYPO_DISCOUNT: f64 = 0.5;
const ANY_FIELD_DISCOUNT: f64 = 0.25;
/// How likely the tagger's best reading must be to take its word: nothing found
/// then means there's nothing to find (`100 market street`), rather than that
/// it misread the query.
const TRUSTED_READING: f64 = 0.9;

impl AirmailIndex {
    /// Searches, most relevant first, as the module says.
    ///
    /// # Errors
    /// If the search fails or a stored document can't be decoded.
    pub fn search(&self, request: &SearchRequest) -> Result<SearchResults> {
        Run::new(self, request)?.search()
    }
}

/// Readings that search alike: the same words sought, and the same runs of
/// words as areas, whichever band they're read as.
#[derive(Debug, Clone, PartialEq, Default)]
struct Group {
    probability: f64,
    venue: Vec<usize>,
    road: Vec<usize>,
    numbers: Vec<usize>,
    postcodes: Vec<usize>,
    areas: Vec<AreaRun>,
    /// Whether it has a word like `near`.
    near: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct AreaRun {
    words: Vec<usize>,
    /// How likely each band is.
    bands: HashMap<&'static str, f64>,
}

impl Group {
    fn same_search(&self, other: &Group) -> bool {
        self.venue == other.venue
            && self.road == other.road
            && self.numbers == other.numbers
            && self.postcodes == other.postcodes
            && self.near == other.near
            && self.areas.len() == other.areas.len()
            && self.areas.iter().zip(&other.areas).all(|(a, b)| a.words == b.words)
    }

    fn only_areas(&self) -> bool {
        self.venue.is_empty() && self.road.is_empty() && self.numbers.is_empty() && self.postcodes.is_empty() && !self.areas.is_empty()
    }

    /// The area searched: the most specific, others narrowing it.
    fn most_specific_area(&self) -> Option<usize> {
        let rank = |run: &AreaRun| {
            run.bands
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .and_then(|(band, _)| AdminBand::ALL.iter().position(|b| b.as_str() == *band))
                .unwrap_or(0)
        };
        (0..self.areas.len()).min_by_key(|&i| rank(&self.areas[i]))
    }

    /// The words sought by name: what's sought, or a road without a number, or
    /// if it's only areas, the most specific: an area's name is a name.
    fn name_words(&self) -> &[usize] {
        if !self.venue.is_empty() {
            &self.venue
        } else if !self.road.is_empty() && self.numbers.is_empty() {
            &self.road
        } else if self.only_areas() {
            &self.areas[self.most_specific_area().expect("an area")].words
        } else {
            &[]
        }
    }
}

/// `readings` as groups, likeliest first.
fn groups(readings: &[(Vec<Tag>, f64)]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (tags, probability) in readings {
        let mut group = Group { probability: *probability, ..Group::default() };
        for (i, tag) in tags.iter().enumerate() {
            match tag {
                Tag::Venue => group.venue.push(i),
                Tag::Road => group.road.push(i),
                Tag::HouseNumber => group.numbers.push(i),
                Tag::Postcode => group.postcodes.push(i),
                Tag::Near => group.near = true,
                Tag::Unit | Tag::Other => {}
                Tag::Neighbourhood | Tag::Locality | Tag::County | Tag::Region | Tag::Country => {
                    let band = tag.band().expect("an area's band");
                    match group.areas.last_mut() {
                        Some(run) if run.words.last() == Some(&(i - 1)) && run.bands.contains_key(band) => run.words.push(i),
                        _ => group.areas.push(AreaRun { words: vec![i], bands: HashMap::from([(band, *probability)]) }),
                    }
                }
            }
        }
        match groups.iter_mut().find(|g| g.same_search(&group)) {
            Some(same) => {
                same.probability += probability;
                for (run, new) in same.areas.iter_mut().zip(group.areas) {
                    for (band, p) in new.bands {
                        *run.bands.entry(band).or_default() += p;
                    }
                }
            }
            None => groups.push(group),
        }
    }
    groups.sort_by(|a, b| b.probability.total_cmp(&a.probability));
    groups
}

/// Where nearness is measured from.
#[derive(Clone, Copy)]
struct Anchor {
    center: s2::point::Point,
    lon_lat: [f64; 2],
    scale_km: f64,
}

impl Anchor {
    fn new(lon_lat: [f64; 2], scale_km: f64) -> Self {
        let [lon, lat] = lon_lat;
        let center = s2::point::Point::from(s2::latlng::LatLng::from_degrees(lat, lon));
        Self { center, lon_lat, scale_km: scale_km.max(MIN_SCALE_KM) }
    }

    fn of_box([west, south, east, north]: [f64; 4]) -> Self {
        let width = (east - west).rem_euclid(360.0);
        let center = [wrap(west + width / 2.0), f64::midpoint(south, north)];
        let corner = Self::new([west, south], 0.0);
        let half_diagonal = corner.km_to(Self::new([east, north], 0.0).center) / 2.0;
        Self::new(center, half_diagonal)
    }

    fn km_to(&self, point: s2::point::Point) -> f64 {
        self.center.distance(&point).rad() * EARTH_RADIUS_KM
    }

    /// The box `factor` times its scale each way, unless that's the whole world.
    fn surroundings(&self, factor: f64) -> Option<[f64; 4]> {
        let [lon, lat] = self.lon_lat;
        widened([lon, lat, lon, lat], self.scale_km * factor)
    }
}

fn wrap(lon: f64) -> f64 {
    (lon + 180.0).rem_euclid(360.0) - 180.0
}

/// `[west, south, east, north]` with `km` around them, unless that's the whole
/// world around.
fn widened([west, south, east, north]: [f64; 4], km: f64) -> Option<[f64; 4]> {
    const KM_PER_DEGREE: f64 = 111.2;
    let latitude = f64::midpoint(south, north).to_radians().cos().max(0.01);
    let lon = km / (KM_PER_DEGREE * latitude);
    if (east - west).rem_euclid(360.0) + 2.0 * lon >= 360.0 {
        return None;
    }
    let lat = km / KM_PER_DEGREE;
    Some([wrap(west - lon), (south - lat).max(-90.0), wrap(east + lon), (north + lat).min(90.0)])
}

fn mass(importance: f64) -> f64 {
    1.0 + MASS_WEIGHT * importance
}

fn nearness(km: f64, scale_km: f64, importance: f64) -> f64 {
    1.0 / (1.0 + km / (scale_km * 10f64.powf(REACH_POWER * importance)))
}

/// An area a query may mean.
struct Candidate {
    id: u64,
    share: f64,
    bounds: [f64; 4],
    /// A neighbourhood, locality or point, with a margin and an anchor of its
    /// own, as opposed to a county or larger, in which places are.
    small: bool,
}

/// Part of the world a group is searched in.
struct Scope {
    share: f64,
    filter: Option<Box<dyn Query>>,
    anchor: Option<Anchor>,
}

#[derive(Clone, Copy, PartialEq)]
enum Pass {
    AsTyped,
    /// By name only, allowing typos.
    Typos,
}

/// The words a group seeks by name.
struct Sought {
    words: Vec<String>,
    /// Whether the last is unfinished.
    unfinished: bool,
    rarity: f32,
}

/// Something a search retrieved.
struct Retrieved {
    address: DocAddress,
    /// All of its score but what takes reading the document.
    score: f64,
    /// What it was found by, if by name.
    sought: Option<Rc<Sought>>,
    diets: Rc<Vec<String>>,
}

fn constant(query: Box<dyn Query>, score: f32) -> Box<dyn Query> {
    Box::new(ConstScoreQuery::new(query, score))
}

fn require(mut clauses: Vec<(Occur, Box<dyn Query>)>, filters: impl IntoIterator<Item = Box<dyn Query>>) -> BooleanQuery {
    clauses.extend(filters.into_iter().map(|filter| (Occur::Must, constant(filter, 0.0))));
    BooleanQuery::new(clauses)
}

/// What a search found, with what it's scored so far.
type Matches = Vec<(f32, DocAddress)>;

/// The best of what `query` matches: its score × `base` × mass × nearness;
/// only what's in `within` if given, checked exactly as it's collected.
fn ranked(searcher: &Searcher, query: &dyn Query, anchor: Option<Anchor>, base: f64, limit: usize, within: Option<&Arc<Vec<Rect<f64>>>>) -> Result<Matches> {
    let collector = TopDocs::with_limit(limit).tweak_score(move |segment: &tantivy::SegmentReader| {
        let importance = segment.fast_fields().f64(FIELD_IMPORTANCE).ok();
        let center = segment.fast_fields().u64(FIELD_CENTER).ok();
        move |doc, score: f32| {
            let importance = importance.as_ref().and_then(|column| column.first(doc)).unwrap_or(0.0);
            let near = match (&anchor, center.as_ref().and_then(|column| column.first(doc))) {
                (Some(anchor), Some(center)) => {
                    nearness(anchor.km_to(s2::point::Point::from(s2::cellid::CellID(center))), anchor.scale_km, importance)
                }
                _ => 1.0,
            };
            (f64::from(score) * base * mass(importance) * near) as f32
        }
    });
    Ok(match within {
        Some(rects) => searcher.search(query, &Within::new(rects.clone(), collector))?,
        None => searcher.search(query, &collector)?,
    })
}

/// One search.
struct Run<'a> {
    index: &'a AirmailIndex,
    searcher: Searcher,
    request: &'a SearchRequest<'a>,
    words: Vec<String>,
    readings: Vec<(Vec<Tag>, f64)>,
    /// Whether the tagger is sure enough of its best reading to take its word.
    trusted: bool,
    typing: bool,
    view: Option<Anchor>,
    located: Option<Anchor>,
    filters: Vec<Box<dyn Query>>,
    /// The request's box, which results must be in.
    bbox: Option<Arc<Vec<Rect<f64>>>>,
    limit: usize,
    retrieved: Vec<Retrieved>,
    categories: Option<QueryCategories>,
}

impl<'a> Run<'a> {
    fn new(index: &'a AirmailIndex, request: &'a SearchRequest<'a>) -> Result<Self> {
        let view = request.viewport.map(Anchor::of_box).or_else(|| request.focus.map(|focus| Anchor::new(focus, MIN_SCALE_KM)));
        let (words, readings) = index.tagged(request.text, request.prefix, view.map(|v| v.lon_lat), MAX_READINGS, READING_COVERAGE);
        let (readings, trusted): (Vec<(Vec<Tag>, f64)>, bool) = match readings {
            Some(readings) if !readings.is_empty() => {
                let trusted = readings.iter().any(|r| r.probability >= TRUSTED_READING);
                (readings.into_iter().map(|r| (r.tags, r.probability)).collect(), trusted)
            }
            // Without a tagger, all of it is what's sought, for all we know.
            _ => (vec![(vec![Tag::Venue; words.len()], 1.0)], false),
        };
        let mut run = Self {
            index,
            searcher: index.searcher()?,
            request,
            words,
            readings,
            trusted,
            typing: request.prefix && !request.text.ends_with(char::is_whitespace),
            view,
            located: request.location.map(|location| Anchor::new(location, MIN_SCALE_KM)),
            filters: Vec::new(),
            bbox: request.bbox.map(|bbox| Arc::new(geometry::bbox_rects(bbox))),
            limit: request.limit.max(1),
            retrieved: Vec::new(),
            categories: None,
        };
        run.filters = run.request_filters();
        Ok(run)
    }

    fn search(mut self) -> Result<SearchResults> {
        let groups = groups(&self.readings);
        // Readings that find nothing don't count: `bookshop fremont` isn't two
        // neighbourhoods, as there's no such area.
        let (mut searched, mut covered) = (0, 0.0);
        for group in &groups {
            if searched == MAX_GROUPS || covered >= GROUP_COVERAGE {
                break;
            }
            let before = self.retrieved.len();
            self.search_group(group, Pass::AsTyped)?;
            if self.retrieved.len() > before {
                searched += 1;
                covered += group.probability;
            }
        }
        if self.request.typos
            && !self.retrieved.iter().any(|r| r.sought.is_some())
            && let Some(group) = groups.iter().find(|g| !g.name_words().is_empty())
        {
            self.search_group(group, Pass::Typos)?;
        }
        if self.retrieved.is_empty() && !self.words.is_empty() && !self.trusted {
            self.search_any_field()?;
        }
        self.ranked_results()
    }

    fn unfinished(&self, indexes: &[usize]) -> bool {
        self.typing && indexes.last() == Some(&self.words.len().saturating_sub(1))
    }

    /// Retrieves what a group seeks, in each of its candidate areas.
    fn search_group(&mut self, group: &Group, pass: Pass) -> Result<()> {
        let specific = group.most_specific_area();
        // A reading of only areas seeks the most specific by name; otherwise
        // what's sought is in them.
        let candidates = if group.only_areas() { Vec::new() } else { self.candidates(group, specific)? };
        trace!("{:?}: {group:?}: {:?}", self.request.text, candidates.iter().map(|c| (c.id, c.share)).collect::<Vec<_>>());
        if !group.areas.is_empty() && !group.only_areas() && candidates.is_empty() {
            return Ok(());
        }
        let default_anchor = if group.near { self.located.or(self.view) } else { self.view.or(self.located) };
        let scopes: Vec<Scope> = if candidates.is_empty() {
            vec![Scope { share: 1.0, filter: None, anchor: default_anchor }]
        } else {
            candidates.iter().flat_map(|candidate| self.scopes_of(candidate, group, default_anchor)).collect()
        };

        let address = self.address_query(group);
        let mut required: Vec<Box<dyn Query>> = self.filters.iter().map(|f| f.box_clone()).collect();
        if group.only_areas() {
            required.extend(self.narrowing(group, specific));
        } else if !group.venue.is_empty() {
            required.extend(address.iter().map(|a| a.box_clone()));
        }

        let categories = if group.venue.is_empty() || pass == Pass::Typos {
            None
        } else {
            let text = group.venue.iter().map(|&i| self.words[i].as_str()).collect::<Vec<_>>().join(" ");
            self.request.model.map(|model| model.understand(&text))
        };
        if self.categories.is_none() {
            self.categories.clone_from(&categories);
        }
        let diets: Rc<Vec<String>> = Rc::new(categories.iter().flat_map(|c| c.diets.iter().map(|(diet, _)| diet.clone())).collect());
        let by_category = categories.as_ref().and_then(|c| self.category_query(c));

        let name_words = group.name_words();
        let sought = if name_words.is_empty() { None } else { Some(Rc::new(self.sought(name_words, pass)?)) };
        let discount = if pass == Pass::Typos { TYPO_DISCOUNT } else { 1.0 };
        let wanted = self.limit + EXTRA_RESULTS;

        for scope in scopes {
            let base = group.probability * scope.share * discount;
            let in_scope = || required.iter().map(|f| f.box_clone()).chain(scope.filter.iter().map(|f| f.box_clone()));
            let mut found: Vec<(Matches, Option<Rc<Sought>>)> = Vec::new();
            let by_other = if group.venue.is_empty() && group.road.is_empty() && !group.postcodes.is_empty() {
                Some(self.postcode_query(group))
            } else if group.venue.is_empty() {
                address.as_ref().map(|a| a.box_clone())
            } else {
                None
            };
            if let Some(sought) = &sought {
                let matching = self.index.all_name_words(&sought.words, sought.unfinished, pass == Pass::Typos);
                let query = require(vec![(Occur::Must, constant(matching, 1.0))], in_scope());
                found.push((ranked(&self.searcher, &query, scope.anchor, base, wanted, self.bbox.as_ref())?, Some(sought.clone())));
            } else if let Some(other) = by_other {
                let query = require(vec![(Occur::Must, constant(other, 1.0))], in_scope());
                found.push((ranked(&self.searcher, &query, scope.anchor, base, wanted, self.bbox.as_ref())?, None));
            }
            if let Some(by_category) = &by_category {
                let query = require(vec![(Occur::Must, by_category.box_clone())], in_scope());
                found.push((self.nearest_rings(&query, &scope, base, wanted)?, None));
            }
            for (matches, sought) in found {
                self.retrieved.extend(matches.into_iter().map(|(score, address)| Retrieved {
                    address,
                    score: f64::from(score),
                    sought: sought.clone(),
                    diets: diets.clone(),
                }));
            }
        }
        Ok(())
    }

    /// The best of `query`, in widening rings around the anchor if nothing but
    /// the anchor limits it.
    fn nearest_rings(&self, query: &BooleanQuery, scope: &Scope, base: f64, wanted: usize) -> Result<Matches> {
        if let (None, Some(anchor)) = (&scope.filter, scope.anchor) {
            for factor in RING_FACTORS {
                let Some(surroundings) = anchor.surroundings(factor) else { break };
                let cells = geometry::query_cells(&geometry::bbox_rects(surroundings));
                let within = require(vec![(Occur::Must, query.box_clone())], [self.index.covering_query(&cells)]);
                let found = ranked(&self.searcher, &within, scope.anchor, base, wanted, self.bbox.as_ref())?;
                if found.len() >= self.limit {
                    return Ok(found);
                }
            }
        }
        ranked(&self.searcher, query, scope.anchor, base, wanted, self.bbox.as_ref())
    }

    /// Every word in any field, for a misread query.
    fn search_any_field(&mut self) -> Result<()> {
        let last = self.words.len() - 1;
        let every_word = self.words.iter().enumerate().map(|(i, word)| -> Box<dyn Query> {
            let fields = [FIELD_NAME, FIELD_STREET, FIELD_HOUSENUMBER, FIELD_UNIT, FIELD_POSTCODE].into_iter().chain(AdminBand::ALL.map(admin_field));
            let unfinished = self.typing && i == last;
            Box::new(BooleanQuery::union(fields.map(|field| self.index.word_query(field, word, unfinished, self.request.typos)).collect()))
        });
        let matching: Box<dyn Query> = Box::new(BooleanQuery::intersection(every_word.collect()));
        let query = require(vec![(Occur::Must, constant(matching, 1.0))], self.filters.iter().map(|f| f.box_clone()));
        let found = ranked(&self.searcher, &query, self.view.or(self.located), ANY_FIELD_DISCOUNT, self.limit + EXTRA_RESULTS, self.bbox.as_ref())?;
        let none = Rc::new(Vec::new());
        self.retrieved.extend(found.into_iter().map(|(score, address)| Retrieved {
            address,
            score: f64::from(score),
            sought: None,
            diets: none.clone(),
        }));
        Ok(())
    }

    /// The areas a group's most specific area words may mean, by how likely
    /// the tagger makes their bands, importance and nearness to the map.
    fn candidates(&self, group: &Group, specific: Option<usize>) -> Result<Vec<Candidate>> {
        let Some(specific) = specific else { return Ok(Vec::new()) };
        let run = &group.areas[specific];
        let name = run.words.iter().map(|&i| self.words[i].as_str()).collect::<Vec<_>>().join(" ");
        let narrowing = self.narrowing(group, Some(specific));
        let mut matches: Vec<(f64, DocAddress, AdminBand)> = Vec::new();
        for band in AdminBand::ALL {
            let Some(weight) = run.bands.get(band.as_str()) else { continue };
            let term = Term::from_field_text(self.index.field(FIELD_AREA_NAME), &area_key(band, &name));
            let named: Box<dyn Query> = if self.unfinished(&run.words) {
                Box::new(FuzzyTermQuery::new_prefix(term, 0, true))
            } else {
                Box::new(TermQuery::new(term, IndexRecordOption::Basic))
            };
            let query = require(vec![(Occur::Must, constant(named, 1.0))], narrowing.iter().map(|q| q.box_clone()));
            for (score, address) in ranked(&self.searcher, &query, self.view, 1.0, MAX_CANDIDATES, None)? {
                matches.push((f64::from(score) * weight, address, band));
            }
        }
        matches.sort_by(|a, b| b.0.total_cmp(&a.0));
        matches.truncate(MAX_CANDIDATES);
        let total: f64 = matches.iter().map(|m| m.0).sum();
        let mut candidates = Vec::new();
        for (weight, address, band) in matches {
            let share = weight / total;
            if share < MIN_CANDIDATE_SHARE {
                continue;
            }
            let hit = self.index.hit(&self.searcher, address, 0.0)?;
            let (Some(id), Some(bounds)) = (hit.wof.as_ref().map(|w| w.id), geometry::bbox(&hit.geometry)) else { continue };
            let is_point = bounds[0] == bounds[2] && bounds[1] == bounds[3];
            candidates.push(Candidate { id, share, bounds, small: is_point || matches!(band, AdminBand::Neighbourhood | AdminBand::Locality) });
        }
        Ok(candidates)
    }

    /// Where to search for a candidate: in it, and if it's small, around it for
    /// less, as where a neighbourhood ends is a matter of opinion.
    fn scopes_of(&self, candidate: &Candidate, group: &Group, default_anchor: Option<Anchor>) -> Vec<Scope> {
        let anchor = if candidate.small && !group.near { Some(Anchor::of_box(candidate.bounds)) } else { default_anchor };
        let in_it = Term::from_field_u64(self.index.field(FIELD_ADMIN_IDS), candidate.id);
        let mut scopes = vec![Scope {
            share: candidate.share,
            filter: Some(Box::new(TermQuery::new(in_it, IndexRecordOption::Basic))),
            anchor,
        }];
        if candidate.small
            && let Some(bounds) = widened(candidate.bounds, AREA_MARGIN_KM)
        {
            let around = self.index.covering_query(&geometry::query_cells(&geometry::bbox_rects(bounds)));
            scopes.push(Scope { share: candidate.share * OUTSIDE, filter: Some(around), anchor });
        }
        scopes
    }

    /// Requires the words of a group's areas other than `specific` in a place's
    /// hierarchy.
    fn narrowing(&self, group: &Group, specific: Option<usize>) -> Vec<Box<dyn Query>> {
        let last = self.words.len().saturating_sub(1);
        let mut narrowing: Vec<Box<dyn Query>> = Vec::new();
        for (i, other) in group.areas.iter().enumerate() {
            if Some(i) == specific {
                continue;
            }
            for &word in &other.words {
                let fields = AdminBand::ALL
                    .iter()
                    .map(|band| self.index.word_query(admin_field(*band), &self.words[word], self.typing && word == last, false))
                    .collect();
                narrowing.push(Box::new(BooleanQuery::union(fields)));
            }
        }
        narrowing
    }

    fn words_in(&self, field: &str, indexes: &[usize]) -> Box<dyn Query> {
        let last = self.words.len().saturating_sub(1);
        Box::new(BooleanQuery::intersection(
            indexes.iter().map(|&i| self.index.word_query(field, &self.words[i], self.typing && i == last, false)).collect(),
        ))
    }

    /// A group's road and house number in a place's address, if it has both or
    /// what's sought is on that road.
    fn address_query(&self, group: &Group) -> Option<Box<dyn Query>> {
        if group.road.is_empty() || (group.numbers.is_empty() && group.venue.is_empty()) {
            return None;
        }
        let mut parts = vec![self.words_in(FIELD_STREET, &group.road)];
        if !group.numbers.is_empty() {
            parts.push(self.words_in(FIELD_HOUSENUMBER, &group.numbers));
        }
        Some(Box::new(BooleanQuery::intersection(parts)))
    }

    /// A postcode's own, or a place's address's.
    fn postcode_query(&self, group: &Group) -> Box<dyn Query> {
        Box::new(BooleanQuery::union(vec![self.words_in(FIELD_NAME, &group.postcodes), self.words_in(FIELD_POSTCODE, &group.postcodes)]))
    }

    fn sought(&self, indexes: &[usize], pass: Pass) -> Result<Sought> {
        let words: Vec<String> = indexes.iter().map(|&i| self.words[i].clone()).collect();
        let unfinished = self.unfinished(indexes);
        let rarity = if pass == Pass::Typos {
            1.0
        } else {
            let matching = self.searcher.search(&self.index.all_name_words(&words, unfinished, false), &Count)?;
            let share = matching.max(1) as f64 / self.searcher.num_docs().max(1) as f64;
            1.0 + RARITY_WEIGHT * (RARITY_COMMON_SHARE / share).max(1.0).log10() as f32
        };
        Ok(Sought { words, unfinished, rarity })
    }

    /// Places with the most specific signal, scored `CATEGORY_MATCH` for the
    /// likeliest and in proportion for the others, as the model's belief may
    /// split across equivalent tags.
    fn category_query(&self, understood: &QueryCategories) -> Option<Box<dyn Query>> {
        let tags: Vec<(String, f32)> = if !understood.cuisines.is_empty() {
            understood.cuisines.iter().map(|(c, p)| (format!("cuisine={c}"), *p)).collect()
        } else if !understood.categories.is_empty() {
            understood.categories.clone()
        } else {
            understood
                .diets
                .iter()
                .flat_map(|(diet, p)| categories::DIET_POSITIVE_VALUES.iter().map(move |value| (format!("diet:{diet}={value}"), *p)))
                .collect()
        };
        let top = tags.iter().map(|(_, p)| *p).fold(0.0, f32::max);
        if top <= 0.0 {
            return None;
        }
        let alternatives = tags
            .iter()
            .map(|(tag, p)| {
                let term = Term::from_field_text(self.index.field(FIELD_INDEXED_TAG), tag);
                constant(Box::new(TermQuery::new(term, IndexRecordOption::Basic)), CATEGORY_MATCH * p / top)
            })
            .collect();
        Some(Box::new(DisjunctionMaxQuery::new(alternatives)))
    }

    /// What a request itself requires.
    fn request_filters(&self) -> Vec<Box<dyn Query>> {
        let term = |field: &str, value: &str| -> Box<dyn Query> {
            Box::new(TermQuery::new(Term::from_field_text(self.index.field(field), value), IndexRecordOption::Basic))
        };
        let mut filters: Vec<Box<dyn Query>> = self.request.tags.iter().map(|tag| term(FIELD_INDEXED_TAG, tag)).collect();
        if !self.request.kinds.is_empty() && self.request.kinds.len() < Kind::ALL.len() {
            filters.push(Box::new(BooleanQuery::union(self.request.kinds.iter().map(|kind| term(FIELD_KIND, kind.as_str())).collect())));
        }
        if let Some(within) = self.request.within {
            filters.push(Box::new(TermQuery::new(Term::from_field_u64(self.index.field(FIELD_ADMIN_IDS), within), IndexRecordOption::Basic)));
        }
        if let Some(bbox) = self.request.bbox {
            filters.push(self.index.covering_query(&geometry::query_cells(&geometry::bbox_rects(bbox))));
        }
        filters
    }

    /// How much of a place's name the words are: the most of any of its names
    /// with them all.
    fn coverage(&self, hit: &SearchHit, sought: &Sought) -> f32 {
        let mut names: Vec<&str> = Vec::new();
        if let Some(wof) = &hit.wof {
            names.push(&wof.default_name);
            names.extend(wof.names.values().map(String::as_str));
        }
        names.extend(hit.tags.iter().filter(|(k, _)| k == "name" || k.starts_with("name:") || k.ends_with("_name")).map(|(_, v)| v.as_str()));
        let last = sought.words.len() - 1;
        names
            .into_iter()
            .filter_map(|name| {
                let tokens = self.index.tokens(&name.replace("'s", "s"));
                let has_all = sought.words.iter().enumerate().all(|(i, word)| {
                    tokens.iter().any(|t| if sought.unfinished && i == last { t.starts_with(word.as_str()) } else { t == word })
                });
                (has_all && !tokens.is_empty()).then(|| (sought.words.len() as f32 / tokens.len() as f32).min(1.0))
            })
            .fold(None, |best: Option<f32>, coverage| Some(best.map_or(coverage, |b| b.max(coverage))))
            .unwrap_or(UNSEEN_COVERAGE)
    }

    /// What was retrieved, scored in full and ranked.
    fn ranked_results(self) -> Result<SearchResults> {
        // Collecting checked the box already; this checks what's decoded.
        let bbox = self.bbox.clone();
        let mut hits: HashMap<DocAddress, SearchHit> = HashMap::new();
        let mut best: HashMap<DocAddress, (f32, bool)> = HashMap::new();
        for item in &self.retrieved {
            let hit = match hits.entry(item.address) {
                Entry::Occupied(hit) => hit.into_mut(),
                Entry::Vacant(place) => place.insert(self.index.hit(&self.searcher, item.address, 0.0)?),
            };
            let hit = &*hit;
            if bbox.as_ref().is_some_and(|rects| !geometry::intersects_any(&hit.geometry, rects)) {
                continue;
            }
            let coverage = item.sought.as_ref().map(|sought| self.coverage(hit, sought));
            let matched = item.sought.as_ref().zip(coverage).map_or(1.0, |(sought, coverage)| coverage * sought.rarity);
            let has_diet = item.diets.iter().any(|diet| {
                hit.tag(&format!("diet:{diet}")).is_some_and(|value| categories::DIET_POSITIVE_VALUES.contains(&value))
            });
            let score = item.score as f32 * matched * if has_diet { DIET_BONUS } else { 1.0 };
            let entry = best.entry(item.address).or_insert((0.0, false));
            entry.0 = entry.0.max(score);
            entry.1 |= coverage.is_some_and(|c| c >= 1.0);
        }
        trace!("{:?}: {} retrieved, {} places", self.request.text, self.retrieved.len(), best.len());

        let mut ranked: Vec<(DocAddress, f32, bool)> = best.into_iter().map(|(address, (score, whole))| (address, score, whole)).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        // The best place with exactly the name stays in reach, however many
        // places of its kind are nearer.
        if ranked.len() > self.limit
            && !ranked[..self.limit].iter().any(|r| r.2)
            && let Some(position) = ranked.iter().position(|r| r.2)
        {
            ranked.swap(self.limit - 1, position);
        }
        let mut sorted: Vec<SearchHit> = Vec::with_capacity(ranked.len());
        for (address, score, _) in ranked {
            let mut hit = hits.remove(&address).expect("a decoded hit");
            hit.score = score;
            sorted.push(hit);
        }
        crate::index::merge_duplicates(&mut sorted);
        sorted.truncate(self.limit);
        let admin_ids: BTreeSet<u64> = sorted.iter().flat_map(|hit| hit.admin_ids.iter().copied()).collect();
        let admins = self.index.admins(&self.searcher, &admin_ids)?;
        Ok(SearchResults { hits: sorted, admins, categories: self.categories })
    }
}

impl AirmailIndex {
    /// All `words` in names.
    fn all_name_words(&self, words: &[String], unfinished: bool, typos: bool) -> Box<dyn Query> {
        let last = words.len().saturating_sub(1);
        Box::new(BooleanQuery::intersection(
            words.iter().enumerate().map(|(i, word)| self.word_query(FIELD_NAME, word, unfinished && i == last, typos)).collect(),
        ))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn readings_differing_only_in_band_are_one_search() {
        let readings = vec![
            (vec![Tag::Venue, Tag::Neighbourhood], 0.36),
            (vec![Tag::Venue, Tag::Locality], 0.23),
            (vec![Tag::Venue, Tag::Venue], 0.15),
            (vec![Tag::Neighbourhood, Tag::Neighbourhood], 0.14),
        ];
        let groups = groups(&readings);
        assert_eq!(groups.len(), 3);
        assert!((groups[0].probability - 0.59).abs() < 1e-9);
        assert_eq!(groups[0].venue, [0]);
        assert_eq!(groups[0].areas[0].words, [1]);
        assert!((groups[0].areas[0].bands["locality"] - 0.23).abs() < 1e-9);
        assert_eq!(groups[2].areas[0].words, [0, 1], "one run");
    }

    #[test]
    fn a_reading_of_only_areas_seeks_the_most_specific_by_name() {
        let group = &groups(&[(vec![Tag::Locality, Tag::Neighbourhood], 1.0)])[0];
        assert_eq!(group.most_specific_area(), Some(1));
        assert_eq!(group.name_words(), [1]);
    }

    #[test]
    fn a_road_with_a_number_is_an_address_not_a_name() {
        let group = &groups(&[(vec![Tag::HouseNumber, Tag::Road, Tag::Road], 1.0)])[0];
        assert!(group.name_words().is_empty());
        let group = &groups(&[(vec![Tag::Road, Tag::Road], 1.0)])[0];
        assert_eq!(group.name_words(), [0, 1]);
    }

    #[test]
    fn famous_places_reach_further() {
        assert!(nearness(8000.0, 5.0, 0.45) > 0.09);
        assert!(nearness(8000.0, 5.0, 0.08) < 0.002);
    }

    #[test]
    fn boxes_widen_and_wrap() {
        let [west, south, east, north] = widened([-122.41, 47.656, -122.361, 47.698], AREA_MARGIN_KM).unwrap();
        assert!(west < -122.47 && east > -122.30 && south < 47.62 && north > 47.73);
        let [west, _, east, _] = widened([179.99, 0.0, -179.99, 0.01], AREA_MARGIN_KM).unwrap();
        assert!(west > 179.0 && east < -179.0);
        assert!(widened([0.0, 0.0, 0.0, 0.0], 30_000.0).is_none());
    }
}
