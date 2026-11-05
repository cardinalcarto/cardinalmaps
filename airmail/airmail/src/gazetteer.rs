//! The index's administrative areas as the tagger's gazetteer: which spans
//! of a query's words name areas in which bands, how well, how well known
//! they are and how far away.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use airmail_tagger::features::{AreaMatch, Gazetteer, Query, Quality};
use geo::{Distance, Haversine, Point};
use tantivy::collector::TopDocs;
use levenshtein_automata::{DFA, LevenshteinAutomatonBuilder, SINK_STATE};
use tantivy::query::{BooleanQuery, TermQuery};
use tantivy::schema::IndexRecordOption;
use tantivy::{Order, Searcher, Term};

use crate::index::{
    AirmailIndex, FIELD_ADMIN_IDS, FIELD_AREA_NAME, FIELD_CENTER, FIELD_IMPORTANCE, FIELD_KIND, FIELD_NAME, FIELD_NAME_EXACT, FIELD_STREET,
    FIELD_WOF_ID, admin_field, area_key,
};
use crate::poi::AdminBand;

/// The most words an area's name in a query may have.
const MAX_SPAN: usize = 6;
/// The most words a name matched with typos may have.
const MAX_TYPO_SPAN: usize = 1;
/// The fewest characters of a word being typed it's looked up by.
const MIN_PREFIX_CHARS: usize = 3;
/// How many of the best-known areas matching a name are looked at for the
/// nearest to the focus.
const CANDIDATES: usize = 32;

/// A name's best match in a band: how, the importance of the best-known
/// area, and where the best-known few are.
#[derive(Debug, Clone)]
struct Found {
    quality: Quality,
    importance: f32,
    centers: Vec<Point<f64>>,
}

/// The gazetteer of an index's areas. Lookups are cached, since the same
/// names come up again and again; it's meant for one thread.
pub struct IndexGazetteer<'a> {
    index: &'a AirmailIndex,
    searcher: Searcher,
    cache: RefCell<HashMap<(String, AdminBand, bool), Option<Found>>>,
    known_words: RefCell<HashMap<String, bool>>,
    /// A word's near misses in each band, and their edit distances.
    typos: RefCell<HashMap<String, BTreeMap<AdminBand, Vec<(String, u8)>>>>,
    /// How many places are named exactly so.
    named: RefCell<HashMap<String, u64>>,
    /// Postcode lookups: whether there's a postcode of the code, and one of
    /// it in an area of a band and name.
    postcodes: RefCell<HashMap<(String, Option<(AdminBand, String)>), bool>>,
    /// The ancestors of the best-known areas of a band and name.
    ancestors: RefCell<HashMap<(AdminBand, String), Vec<u64>>>,
    /// Whether an area of a band and name is in one of another.
    within: RefCell<HashMap<((AdminBand, String), (AdminBand, String)), bool>>,
}

/// The band of matches that are places' exact names rather than areas':
/// `fred meyer` is many stores' name, `ballard fred meyer` none's.
pub const NAME_BAND: &str = airmail_tagger::features::NAME_BAND;
pub use airmail_tagger::features::{POSTCODE_ALONE, POSTCODE_BAND, POSTCODE_IN_AREA_BESIDE, POSTCODE_NOT_IN_AREA_BESIDE};

/// How many places have a name, as a match's importance, so that its
/// buckets are 1, 2 to 9, 10 to 99 and 100 or more.
fn name_importance(count: u64) -> f32 {
    match count {
        0 | 1 => 0.05,
        2..=9 => 0.15,
        10..=99 => 0.35,
        _ => 0.5,
    }
}

/// The most near misses of a word in a band looked up.
const MAX_TYPO_KEYS: usize = 64;

/// Area keys (`<band>/<name>`) of any band whose name starts with a word's
/// first character and whose rest is within the DFA's distance of the
/// word's: one pass over the term dictionary for every band. Keeping the
/// first character, as Lucene does by default, is what makes it fast, and
/// typos are seldom there.
struct NearMisses<'d> {
    first: Vec<u8>,
    rest: &'d DFA,
}

#[derive(Clone, Copy)]
enum NearMiss {
    Band,
    First(usize),
    Rest(u32),
    Dead,
}

impl tantivy_fst::Automaton for NearMisses<'_> {
    type State = NearMiss;

    fn start(&self) -> NearMiss {
        NearMiss::Band
    }

    fn is_match(&self, state: &NearMiss) -> bool {
        matches!(state, NearMiss::Rest(s) if matches!(self.rest.distance(*s), levenshtein_automata::Distance::Exact(_)))
    }

    fn can_match(&self, state: &NearMiss) -> bool {
        match state {
            NearMiss::Dead => false,
            NearMiss::Rest(s) => *s != SINK_STATE,
            _ => true,
        }
    }

    fn accept(&self, state: &NearMiss, byte: u8) -> NearMiss {
        match *state {
            NearMiss::Band if byte == b'/' => NearMiss::First(0),
            NearMiss::Band => NearMiss::Band,
            NearMiss::First(i) if byte == self.first[i] => {
                if i + 1 == self.first.len() { NearMiss::Rest(self.rest.initial_state()) } else { NearMiss::First(i + 1) }
            }
            NearMiss::Rest(s) => NearMiss::Rest(self.rest.transition(s, byte)),
            NearMiss::First(_) | NearMiss::Dead => NearMiss::Dead,
        }
    }
}

/// Levenshtein automata builders, with transpositions, by distance: slow to
/// make, so made once.
fn builder(distance: u8) -> &'static LevenshteinAutomatonBuilder {
    static BUILDERS: OnceLock<[LevenshteinAutomatonBuilder; 2]> = OnceLock::new();
    &BUILDERS.get_or_init(|| [LevenshteinAutomatonBuilder::new(1, true), LevenshteinAutomatonBuilder::new(2, true)])
        [usize::from(distance.clamp(1, 2) - 1)]
}

impl<'a> IndexGazetteer<'a> {
    /// # Errors
    /// If the index can't be read.
    pub fn new(index: &'a AirmailIndex) -> tantivy::Result<Self> {
        Ok(Self {
            index,
            searcher: index.searcher()?,
            cache: RefCell::default(),
            known_words: RefCell::default(),
            typos: RefCell::default(),
            named: RefCell::default(),
            postcodes: RefCell::default(),
            ancestors: RefCell::default(),
            within: RefCell::default(),
        })
    }

    /// Forgets its cached lookups if there are more than `most`, so a
    /// gazetteer used for many queries stays small.
    pub fn forget_if_over(&self, most: usize) {
        let held = self.cache.borrow().len()
            + self.known_words.borrow().len()
            + self.typos.borrow().len()
            + self.named.borrow().len()
            + self.postcodes.borrow().len()
            + self.ancestors.borrow().len()
            + self.within.borrow().len();
        if held > most {
            self.cache.borrow_mut().clear();
            self.known_words.borrow_mut().clear();
            self.typos.borrow_mut().clear();
            self.named.borrow_mut().clear();
            self.postcodes.borrow_mut().clear();
            self.ancestors.borrow_mut().clear();
            self.within.borrow_mut().clear();
        }
    }

    /// The importance of the best-known area a query matches and the centers
    /// of the best-known few, if any match.
    fn best(&self, query: &dyn tantivy::query::Query) -> Option<(f32, Vec<Point<f64>>)> {
        let collector = TopDocs::with_limit(CANDIDATES).order_by_fast_field::<f64>(FIELD_IMPORTANCE, Order::Desc);
        let found = self.searcher.search(query, &collector).ok()?;
        let importance = found.first()?.0.unwrap_or(0.0) as f32;
        let centers = found
            .iter()
            .filter_map(|(_, address)| {
                let column = self.searcher.segment_reader(address.segment_ord).fast_fields().u64(FIELD_CENTER).ok()?;
                let center = s2::latlng::LatLng::from(s2::cellid::CellID(column.first(address.doc_id)?));
                Some(Point::new(center.lng.deg(), center.lat.deg()))
            })
            .collect();
        Some((importance, centers))
    }

    /// How many places are named exactly `name` (words as the index keys
    /// them, separated by spaces).
    fn named(&self, name: &str) -> u64 {
        if let Some(count) = self.named.borrow().get(name) {
            return *count;
        }
        let term = Term::from_field_text(self.index.field(FIELD_NAME_EXACT), name);
        let count = self.searcher.doc_freq(&term).unwrap_or(0);
        self.named.borrow_mut().insert(name.to_string(), count);
        count
    }

    /// Whether any name, street or area has the word.
    fn known(&self, word: &str) -> bool {
        if let Some(known) = self.known_words.borrow().get(word) {
            return *known;
        }
        let fields = std::iter::once(FIELD_NAME).chain(std::iter::once(FIELD_STREET)).chain(AdminBand::ALL.map(admin_field));
        let known = fields.into_iter().any(|field| {
            self.searcher.doc_freq(&Term::from_field_text(self.index.field(field), word)).is_ok_and(|f| f > 0)
        });
        self.known_words.borrow_mut().insert(word.to_string(), known);
        known
    }

    /// How a name matches an area in a band, if it does: exactly, else (if
    /// it's being typed) as the beginning of one, else with typos.
    fn lookup(&self, name: &str, band: AdminBand, typing: bool, words: usize) -> Option<Found> {
        let key = (name.to_string(), band, typing);
        if let Some(found) = self.cache.borrow().get(&key) {
            return found.clone();
        }
        let mut found = None;
        let terms: Vec<Term> =
            area_terms(self.index, band, name).into_iter().filter(|t| self.searcher.doc_freq(t).is_ok_and(|f| f > 0)).collect();
        if !terms.is_empty() {
            found = self.best(&*any_of(terms)).map(|b| (Quality::Exact, b));
        }
        // A beginning too short to say anything, like `s`, matches millions.
        let last_word = name.rsplit(' ').next().unwrap_or(name);
        if found.is_none() && typing && last_word.chars().count() >= MIN_PREFIX_CHARS {
            let keys = self.keys_beginning(&area_key(band, name));
            if !keys.is_empty() {
                found = self.best(&*any_of(keys)).map(|b| (Quality::Prefix, b));
            }
        }
        let distance = typos(name);
        // Only for a word the index doesn't have, which a typo usually makes:
        // looking for typos is far slower than looking up a name.
        let unknown = || name.split(' ').any(|word| !self.known(word));
        // Nor for the word being typed: cut short, it's not misspelled.
        if found.is_none() && !typing && distance > 0 && words <= MAX_TYPO_SPAN && unknown() {
            found = self.near_misses(name, distance).get(&band).and_then(|keys| {
                let nearest = keys.iter().map(|(_, d)| *d).min()?;
                let quality = if nearest <= 1 { Quality::Typo } else { Quality::Typos };
                let terms = keys
                    .iter()
                    .filter(|(_, d)| *d == nearest)
                    .take(MAX_TYPO_KEYS)
                    .map(|(key, _)| -> Box<dyn tantivy::query::Query> {
                        let term = Term::from_field_text(self.index.field(FIELD_AREA_NAME), key);
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                    })
                    .collect();
                self.best(&BooleanQuery::union(terms)).map(|b| (quality, b))
            });
        }
        let found = found.map(|(quality, (importance, centers))| Found { quality, importance, centers });
        self.cache.borrow_mut().insert(key, found.clone());
        found
    }
}

impl IndexGazetteer<'_> {
    /// Whether there's a postcode `code` (words as the index keys them), and
    /// with `area`, one in an area of that band and name: libpostal's
    /// postcodes and their admin contexts, from the index.
    fn postcode(&self, code: &str, area: Option<(AdminBand, &str)>) -> bool {
        let key = (code.to_string(), area.map(|(band, name)| (band, name.to_string())));
        if let Some(found) = self.postcodes.borrow().get(&key) {
            return *found;
        }
        let mut terms: Vec<Box<dyn tantivy::query::Query>> = vec![
            Box::new(TermQuery::new(Term::from_field_text(self.index.field(FIELD_NAME_EXACT), code), IndexRecordOption::Basic)),
            Box::new(TermQuery::new(
                Term::from_field_text(self.index.field(FIELD_KIND), crate::poi::Kind::Postcode.as_str()),
                IndexRecordOption::Basic,
            )),
        ];
        if let Some((band, name)) = area {
            for word in name.split(' ') {
                let term = Term::from_field_text(self.index.field(admin_field(band)), word);
                terms.push(Box::new(TermQuery::new(term, IndexRecordOption::Basic)));
            }
        }
        let found = self.searcher.search(&BooleanQuery::intersection(terms), &tantivy::collector::Count).is_ok_and(|n| n > 0);
        self.postcodes.borrow_mut().insert(key, found);
        found
    }

    /// Area keys within `distance` edits of a word, keeping its first
    /// character, by band, with their distances.
    fn near_misses(&self, word: &str, distance: u8) -> BTreeMap<AdminBand, Vec<(String, u8)>> {
        if let Some(found) = self.typos.borrow().get(word) {
            return found.clone();
        }
        let mut found: BTreeMap<AdminBand, Vec<(String, u8)>> = BTreeMap::new();
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            let rest = chars.as_str();
            let dfa = builder(distance).build_dfa(rest);
            let mut buffer = [0; 4];
            let automaton = NearMisses { first: first.encode_utf8(&mut buffer).as_bytes().to_vec(), rest: &dfa };
            let field = self.index.field(FIELD_AREA_NAME);
            let mut keys = std::collections::BTreeSet::new();
            for segment in self.searcher.segment_readers() {
                let Ok(inverted) = segment.inverted_index(field) else { continue };
                let Ok(mut stream) = inverted.terms().search(&automaton).into_stream() else { continue };
                while stream.advance() {
                    keys.insert(stream.key().to_vec());
                }
            }
            for key in keys {
                let Ok(key) = String::from_utf8(key) else { continue };
                let Some((band, name)) = key.split_once('/') else { continue };
                let Some(band) = AdminBand::ALL.into_iter().find(|b| b.as_str() == band) else { continue };
                let rest_of_name = name.char_indices().nth(1).map_or("", |(i, _)| &name[i..]);
                let edits = match dfa.eval(rest_of_name) {
                    levenshtein_automata::Distance::Exact(d) => d,
                    levenshtein_automata::Distance::AtLeast(_) => continue,
                };
                found.entry(band).or_default().push((key.clone(), edits));
            }
        }
        self.typos.borrow_mut().insert(word.to_string(), found.clone());
        found
    }
}

/// Edits tolerated in a name of this many characters.
fn typos(name: &str) -> u8 {
    match name.chars().count() {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    }
}

impl Gazetteer for IndexGazetteer<'_> {
    fn areas(&self, query: &Query) -> Vec<AreaMatch> {
        let n = query.words.len();
        let focus = query.focus.map(|[lon, lat]| Point::new(lon, lat));
        let mut matches = Vec::new();
        for start in 0..n {
            for end in start + 1..=n.min(start + MAX_SPAN) {
                let name = query.words[start..end].join(" ");
                let typing = query.typing && end == n;
                let count = self.named(&name);
                if count > 0 {
                    matches.push(AreaMatch {
                        start,
                        end,
                        band: NAME_BAND,
                        quality: Quality::Exact,
                        importance: name_importance(count),
                        distance_km: None,
                        inside: Vec::new(),
                    });
                }
                for band in AdminBand::ALL {
                    let Some(found) = self.lookup(&name, band, typing, end - start) else { continue };
                    let distance_km = focus.and_then(|focus| {
                        found.centers.iter().map(|c| Haversine.distance(focus, *c) / 1000.0).reduce(f64::min).map(|km| km as f32)
                    });
                    matches.push(AreaMatch {
                        start,
                        end,
                        band: band.as_str(),
                        quality: found.quality,
                        importance: found.importance,
                        distance_km,
                        inside: Vec::new(),
                    });
                }
            }
        }
        self.containment(query, &mut matches);
        self.postcodes_in(query, &mut matches);
        matches
    }
}

/// Abbreviated words of place names and what they abbreviate, from
/// libpostal's dictionaries (the first form of each line): `mt` is `mount`,
/// `st` `saint`, `ft` `fort`. Areas are indexed by their names as written,
/// and queries abbreviate them.
fn written_out() -> &'static HashMap<String, Vec<String>> {
    static WRITTEN_OUT: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    WRITTEN_OUT.get_or_init(|| {
        // Per abbreviation, what it abbreviates and in how many dictionaries.
        let mut counts: HashMap<String, HashMap<String, usize>> = HashMap::new();
        for kind in ["synonyms", "personal_titles"] {
            for (_, contents) in airmail_tagger::dictionaries::of_type(kind) {
                for line in contents.lines() {
                    let forms: Vec<String> = line.split('|').map(|f| crate::text::fold(f.trim()).trim_end_matches('.').to_string()).collect();
                    let Some((full, abbreviations)) = forms.split_first() else { continue };
                    if full.is_empty() || full.contains(' ') {
                        continue;
                    }
                    for abbreviation in abbreviations.iter().filter(|a| !a.is_empty() && !a.contains(' ') && *a != full) {
                        *counts.entry(abbreviation.clone()).or_default().entry(full.clone()).or_default() += 1;
                    }
                }
            }
        }
        // The most widely shared first: `st` is `sankt` and `saint` before
        // Danish `store`.
        counts
            .into_iter()
            .map(|(abbreviation, fulls)| {
                let mut fulls: Vec<(String, usize)> = fulls.into_iter().collect();
                fulls.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                (abbreviation, fulls.into_iter().map(|(full, _)| full).collect())
            })
            .collect()
    })
}

/// The most area keys a name being typed is looked up by, the first in
/// order: the shortest names it begins.
const MAX_PREFIX_KEYS: usize = 256;

impl IndexGazetteer<'_> {
    /// The area keys beginning with `prefix`, as terms: from the term
    /// dictionaries' ranges, as a prefix `FuzzyTermQuery` would match them
    /// but without its walk of the dictionary by automaton and its bitset of
    /// every document.
    fn keys_beginning(&self, prefix: &str) -> Vec<Term> {
        let field = self.index.field(FIELD_AREA_NAME);
        let mut keys = std::collections::BTreeSet::new();
        for segment in self.searcher.segment_readers() {
            let Ok(inverted) = segment.inverted_index(field) else { continue };
            let Ok(mut stream) = inverted.terms().range().ge(prefix.as_bytes()).into_stream() else { continue };
            let mut taken = 0;
            while taken < MAX_PREFIX_KEYS && stream.advance() {
                let key = stream.key();
                if !key.starts_with(prefix.as_bytes()) {
                    break;
                }
                keys.insert(key.to_vec());
                taken += 1;
            }
        }
        keys.into_iter()
            .take(MAX_PREFIX_KEYS)
            .filter_map(|key| String::from_utf8(key).ok())
            .map(|key| Term::from_field_text(field, &key))
            .collect()
    }
}

/// The most forms of a name looked up.
const MAX_FORMS: usize = 6;

/// A name, and as many as `MAX_FORMS` in all with its abbreviations
/// written out.
fn forms(name: &str) -> Vec<String> {
    let mut forms = vec![String::new()];
    for (i, word) in name.split(' ').enumerate() {
        let mut next = Vec::new();
        for form in &forms {
            let sep = if i == 0 { "" } else { " " };
            next.push(format!("{form}{sep}{word}"));
            for full in written_out().get(word).into_iter().flatten() {
                next.push(format!("{form}{sep}{full}"));
            }
        }
        next.truncate(MAX_FORMS);
        forms = next;
    }
    forms
}

/// The gazetteer's keys of a name's forms in a band.
fn area_terms(index: &AirmailIndex, band: AdminBand, name: &str) -> Vec<Term> {
    forms(name).iter().map(|form| Term::from_field_text(index.field(FIELD_AREA_NAME), &area_key(band, form))).collect()
}

/// Any of the terms.
fn any_of(terms: Vec<Term>) -> Box<dyn tantivy::query::Query> {
    let mut queries: Vec<Box<dyn tantivy::query::Query>> =
        terms.into_iter().map(|t| Box::new(TermQuery::new(t, IndexRecordOption::Basic)) as Box<dyn tantivy::query::Query>).collect();
    if queries.len() == 1 { queries.pop().expect("one") } else { Box::new(BooleanQuery::union(queries)) }
}

/// The most words of other areas' names between an area and one it's in.
const MAX_BETWEEN: usize = 3;

/// The most areas of a name whose ancestors containment looks at, the
/// best-known first.
const MAX_CHILDREN: usize = 64;

impl IndexGazetteer<'_> {
    /// The ancestors' IDs of the best-known areas of a band and name.
    fn ancestors(&self, band: AdminBand, name: &str) -> Vec<u64> {
        let key = (band, name.to_string());
        if let Some(found) = self.ancestors.borrow().get(&key) {
            return found.clone();
        }
        let collector = TopDocs::with_limit(MAX_CHILDREN).order_by_fast_field::<f64>(FIELD_IMPORTANCE, Order::Desc);
        let field = self.index.field(FIELD_ADMIN_IDS);
        let mut ids: Vec<u64> = self
            .searcher
            .search(&*any_of(area_terms(self.index, band, name)), &collector)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(_, address)| self.searcher.doc::<tantivy::TantivyDocument>(address).ok())
            .flat_map(|doc| doc.get_all(field).filter_map(|v| tantivy::schema::Value::as_u64(&v)).collect::<Vec<_>>())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        self.ancestors.borrow_mut().insert(key, ids.clone());
        ids
    }

    /// Whether one of the best-known areas of a band and name is in an area
    /// of another band and name.
    fn is_within(&self, child: (AdminBand, &str), parent: (AdminBand, &str)) -> bool {
        let key = ((child.0, child.1.to_string()), (parent.0, parent.1.to_string()));
        if let Some(found) = self.within.borrow().get(&key) {
            return *found;
        }
        let ancestors = self.ancestors(child.0, child.1);
        let found = !ancestors.is_empty() && {
            let named = any_of(area_terms(self.index, parent.0, parent.1));
            let field = self.index.field(FIELD_WOF_ID);
            // Term queries, not a `TermSetQuery`, which builds an FST of the
            // terms and a bitset of every document each time.
            let ids = any_of(ancestors.iter().map(|id| Term::from_field_u64(field, *id)).collect());
            let query = BooleanQuery::intersection(vec![named, ids]);
            self.searcher.search(&query, &tantivy::collector::Count).is_ok_and(|n| n > 0)
        };
        self.within.borrow_mut().insert(key, found);
        found
    }

    /// Marks each exact area match with the exact matches of bigger bands
    /// that one of its areas is in: right after it (`chelsea new york`) or
    /// right before it (`new york chelsea`, as Chinese, Japanese and Korean
    /// write), or past other areas' names between them (`chelsea new york
    /// ny`).
    fn containment(&self, query: &Query, matches: &mut [AreaMatch]) {
        let band_of = |m: &AreaMatch| AdminBand::ALL.into_iter().find(|b| b.as_str() == m.band);
        let exact: Vec<(usize, usize, AdminBand)> =
            matches.iter().filter(|m| m.quality == Quality::Exact).filter_map(|m| Some((m.start, m.end, band_of(m)?))).collect();
        // Whether every word of `from..to` is in an area's name.
        let named = |from: usize, to: usize| (from..to).all(|w| exact.iter().any(|(s, e, _)| *s <= w && w < *e));
        for m in matches.iter_mut().filter(|m| m.quality == Quality::Exact) {
            let Some(band) = band_of(m) else { continue };
            let child = query.words[m.start..m.end].join(" ");
            let parents = exact.iter().filter(|(s, e, b)| {
                *b > band && ((*s >= m.end && *s - m.end <= MAX_BETWEEN && named(m.end, *s)) || (*e <= m.start && m.start - *e <= MAX_BETWEEN && named(*e, m.start)))
            });
            for (start, end, parent_band) in parents {
                let parent = query.words[*start..*end].join(" ");
                let span = (*start, *end, parent_band.as_str());
                if !m.inside.contains(&span) && self.is_within((band, &child), (*parent_band, &parent)) {
                    m.inside.push(span);
                }
            }
        }
    }
}

/// The most words a postcode has (`sw1a 1aa`).
const MAX_POSTCODE_WORDS: usize = 2;

impl IndexGazetteer<'_> {
    /// Spans of words that are a postcode, as matches in the postcode band,
    /// their importance saying whether an area named next to it has a
    /// postcode of that code (see `POSTCODE_IN_AREA_BESIDE`).
    fn postcodes_in(&self, query: &Query, matches: &mut Vec<AreaMatch>) {
        let n = query.words.len();
        let areas: Vec<(usize, usize, AdminBand)> = matches
            .iter()
            .filter(|m| m.quality == Quality::Exact)
            .filter_map(|m| Some((m.start, m.end, AdminBand::ALL.into_iter().find(|b| b.as_str() == m.band)?)))
            .collect();
        for start in 0..n {
            for end in start + 1..=n.min(start + MAX_POSTCODE_WORDS) {
                let words = &query.words[start..end];
                if !words.iter().any(|w| w.chars().any(char::is_numeric)) {
                    continue;
                }
                let code = words.join(" ");
                if !self.postcode(&code, None) {
                    continue;
                }
                let mut beside = areas.iter().filter(|(s, e, _)| *e == start || *s == end).peekable();
                let alone = beside.peek().is_none();
                let context = beside.any(|(s, e, band)| self.postcode(&code, Some((*band, &query.words[*s..*e].join(" ")))));
                matches.push(AreaMatch {
                    start,
                    end,
                    band: POSTCODE_BAND,
                    quality: Quality::Exact,
                    importance: if context {
                        POSTCODE_IN_AREA_BESIDE
                    } else if alone {
                        POSTCODE_ALONE
                    } else {
                        POSTCODE_NOT_IN_AREA_BESIDE
                    },
                    distance_km: None,
                    inside: Vec::new(),
                });
            }
        }
    }
}

#[cfg(test)]
mod test {
    use tantivy_fst::{IntoStreamer, Map, Streamer};

    use super::*;

    #[test]
    fn names_are_looked_up_with_abbreviations_written_out() {
        let pleasant = forms("mt pleasant");
        assert_eq!(pleasant[0], "mt pleasant");
        assert!(pleasant.contains(&"mount pleasant".to_string()), "{pleasant:?}");
        assert!(forms("ft worth").contains(&"fort worth".to_string()));
        assert!(forms("st louis").contains(&"saint louis".to_string()));
        assert_eq!(forms("seattle"), vec!["seattle".to_string()]);
        assert!(forms("st st st st").len() <= MAX_FORMS);
    }

    fn near_misses(keys: &[&str], word: &str, distance: u8) -> Vec<String> {
        let mut keys = keys.to_vec();
        keys.sort_unstable();
        let map = Map::from_iter(keys.iter().map(|k| (*k, 0))).unwrap();
        let mut chars = word.chars();
        let first = chars.next().unwrap().to_string().into_bytes();
        let dfa = builder(distance).build_dfa(chars.as_str());
        let mut stream = map.search(NearMisses { first, rest: &dfa }).into_stream();
        let mut found = Vec::new();
        while let Some((key, _)) = stream.next() {
            found.push(String::from_utf8(key.to_vec()).unwrap());
        }
        found
    }

    #[test]
    fn near_misses_are_in_any_band_and_keep_the_first_character() {
        let keys = ["locality/seattle", "county/seattle", "locality/settle", "locality/battle", "region/seatac", "locality/zürich"];
        assert_eq!(near_misses(&keys, "seatle", 1), ["county/seattle", "locality/seattle", "locality/settle"]);
        // A typo in the first character isn't looked for.
        assert!(near_misses(&keys, "beattle", 1).iter().all(|k| k.ends_with("battle") || k.ends_with("beattle")));
        // Edits count characters, not bytes.
        assert_eq!(near_misses(&keys, "zurich", 1), ["locality/zürich"]);
        assert_eq!(near_misses(&keys, "zürch", 1), ["locality/zürich"]);
        assert_eq!(near_misses(&keys, "zrch", 1), Vec::<String>::new());
    }
}
