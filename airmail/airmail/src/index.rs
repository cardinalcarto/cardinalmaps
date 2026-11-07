use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use geo::{Distance, Geometry, Intersects, Point};
use log::{trace, warn};
use serde::{Deserialize, Serialize};
use tantivy::schema::{FAST, STORED, STRING, Value};
use tantivy::{
    DocAddress, Order, Searcher, TantivyDocument, Term,
    collector::TopDocs,
    directory::{
        Directory, DirectoryLock, FileHandle, Lock, MmapDirectory, WatchCallback, WatchHandle,
        WritePtr,
        error::{DeleteError, LockError, OpenReadError, OpenWriteError},
    },
    query::{
        BooleanQuery, BoostQuery, ConstScoreQuery, DisjunctionMaxQuery, FuzzyTermQuery, Occur,
        PhrasePrefixQuery, PhraseQuery, Query, TermQuery,
    },
    schema::{
        IndexRecordOption, NumericOptions, OwnedValue, Schema, TextFieldIndexing, TextOptions,
    },
};
use tokio::task::spawn_blocking;

use crate::categories::{self, CategoryMode, QueryCategories};
use crate::cuisine;
use crate::error::AirmailError;
use crate::geometry;
use crate::importance::IMPORTANCE_WEIGHT;
use crate::interpret::{self, Hints, Part, QueryWords, Reading};
use airmail_tagger::labels::Tag;
use crate::poi::{AdminBand, Kind, OsmRef, PlaceId, SchemafiedPoi, WofPlace};
use crate::query::all_subsequences;

/// The version of the index's schema and of what its fields hold. Readers
/// refuse indexes of other versions.
pub const SCHEMA_VERSION: u32 = 7;

/// Records `SCHEMA_VERSION` in the index directory.
const VERSION_FILE: &str = "airmail-index.json";

// Searchable text.
pub const FIELD_NAME: &str = "name";
pub const FIELD_NAME_EXACT: &str = "name_exact";
/// Its road's name, in the forms it's written in.
pub const FIELD_STREET: &str = "street";
pub const FIELD_HOUSENUMBER: &str = "housenumber";
pub const FIELD_UNIT: &str = "unit";
/// As written and without spaces.
pub const FIELD_POSTCODE: &str = "postcode";
/// The names of the areas containing it, by band (see `admin_field`); the
/// country's also with its ISO codes.
pub const FIELD_ADMIN_NEIGHBOURHOOD: &str = "admin_neighbourhood";
pub const FIELD_ADMIN_LOCALITY: &str = "admin_locality";
pub const FIELD_ADMIN_COUNTY: &str = "admin_county";
pub const FIELD_ADMIN_REGION: &str = "admin_region";
pub const FIELD_ADMIN_COUNTRY: &str = "admin_country";
pub const FIELD_INDEXED_TAG: &str = "indexed_tag";
/// An administrative area's own names as `<band>/<words>`, abbreviations
/// and a country's ISO codes included: the gazetteer of areas by band, for
/// telling whether words name a locality, a region or both.
pub const FIELD_AREA_NAME: &str = "area_name";
/// An administrative area's band.
pub const FIELD_PLACETYPE: &str = "placetype";
/// The ISO 3166-1 alpha-2 code of the country it's in, uppercase.
pub const FIELD_COUNTRY_CODE: &str = "country_code";
// Filters and identity.
pub const FIELD_KIND: &str = "kind";
/// Whether a document is an administrative area (1) or not (0), for ranking
/// to tell while it scores.
pub const FIELD_IS_ADMIN: &str = "is_admin";
pub const FIELD_SOURCE: &str = "source";
pub const FIELD_ADMIN_IDS: &str = "admin_ids";
pub const FIELD_WOF_ID: &str = "wof_id";
/// Cardinal's stable POI identifier, for lookups.
pub const FIELD_POI_ID: &str = "poi_id";
/// The OSM element as `type/id`, for lookups.
pub const FIELD_OSM_ID: &str = "osm_id";
/// A postcode's key, `<country>/<postcode>` (see `poi::postcode_key`).
pub const FIELD_POSTCODE_ID: &str = "postcode_id";
pub const FIELD_S2_COVER: &str = "s2_cover";
pub const FIELD_S2_COVER_ANCESTORS: &str = "s2_cover_ancestors";
// Stored for results.
pub const FIELD_OSM: &str = "osm";
pub const FIELD_WOF: &str = "wof";
pub const FIELD_TAGS: &str = "tags";
pub const FIELD_CENTER: &str = "center";
pub const FIELD_GEOMETRY: &str = "geometry";
pub const FIELD_IMPORTANCE: &str = "importance";

/// The field with the names of a document's areas in `band`.
#[must_use]
pub fn admin_field(band: AdminBand) -> &'static str {
    match band {
        AdminBand::Neighbourhood => FIELD_ADMIN_NEIGHBOURHOOD,
        AdminBand::Locality => FIELD_ADMIN_LOCALITY,
        AdminBand::County => FIELD_ADMIN_COUNTY,
        AdminBand::Region => FIELD_ADMIN_REGION,
        AdminBand::Country => FIELD_ADMIN_COUNTRY,
    }
}

/// Fields with a place's own address, which a query word in them counts as
/// much as `ADDRESS_BOOST` for.
const ADDRESS_FIELDS: [&str; 4] = [FIELD_STREET, FIELD_HOUSENUMBER, FIELD_UNIT, FIELD_POSTCODE];

/// A gazetteer key: `<band>/<words>`.
#[must_use]
pub fn area_key(band: AdminBand, words: &str) -> String {
    format!("{}/{words}", band.as_str())
}

// How much a query word counts in each field: a name match counts most, so a
// place named after a city outranks the places in it.
const NAME_BOOST: f32 = 3.0;
const ADDRESS_BOOST: f32 = 1.5;
const ADMIN_BOOST: f32 = 1.0;
/// How much a word naming one of its admins counts for a POI that matches a
/// category search by its category alone. The category already accounts for
/// words like `bookstore`, so where a place is matters more there than its
/// name repeating the category: a bookshop in Fremont beats one named
/// "Bookstore" elsewhere.
const CATEGORY_ADMIN_BOOST: f32 = 6.0;
/// For a query that is exactly a name.
const EXACT_NAME_BOOST: f32 = 15.0;
/// For a query whose first words are exactly a name, with the rest matching
/// elsewhere, such as `seattle wa`: a constant, more than a phrase match, so
/// `seattle washington` is Seattle rather than something named with both
/// words. (30 to 120 score alike on the evals.)
const EXACT_NAME_PREFIX_SCORE: f32 = 60.0;
/// While the last word may be unfinished (with `prefix`), constant scores, not
/// scaled by how rare the word is: for the whole query being exactly a name,
/// and for the last word beginning a word as typed rather than with a typo.
const EXACT_NAME_WHILE_TYPING_SCORE: f32 = 5.0;
const TYPED_PREFIX_SCORE: f32 = 1.0;
/// For a query that's a name with typos, or its prefix.
const NEAR_EXACT_NAME_BOOST: f32 = 10.0;

/// A reading of the query as a subject in some areas (see `interpret`)
/// scores a result in them its subject's score times its weight times `1 +
/// AREA_BONUS * confidence`, so that being where the query says counts more
/// than having the area's name in its own: Chelsea in Manhattan beats a shop
/// called "CHELSEA New York" for `chelsea, new york`.
const AREA_BONUS: f32 = 0.75;
/// For a postcode in a reading's context that the result's address has.
const POSTCODE_SCORE: f32 = 15.0;
/// For an area in the band an interpretation reads it as: `new york` read
/// as a locality is New York City before the state.
const BAND_SCORE: f32 = 10.0;
/// How much the query taken literally is discounted by the probability that
/// the tagger's interpretations put what's sought in areas.
const LITERAL_DISCOUNT: f32 = 0.45;
/// The most interpretations searched for, and the share of the tagger's
/// belief they're searched for until they have.
const MAX_INTERPRETATIONS: usize = 4;
const INTERPRETATION_COVERAGE: f64 = 0.9;
/// How much more an address scores when the parser says the query is one, so
/// that `90 vermilyea avenue` is the address before the post office there.
const ADDRESS_KIND_FACTOR: f32 = 1.5;
/// A reading of the query as just the area its subject is in, so that `some
/// unknown street, berlin` still finds Berlin: this much of what Berlin would
/// score for `berlin`.
const AREA_FALLBACK_WEIGHT: f32 = 0.2;

/// With a focus point, search multiplies a result's score by
///
/// ```text
/// (1 + FOCUS_WEIGHT * FOCUS_SCALE_KM / (FOCUS_SCALE_KM + distance_km))
///     * (1 + REGION_WEIGHT * REGION_SCALE_KM / (REGION_SCALE_KM + distance_km))
/// ```
///
/// The first factor is local: up to `1 + FOCUS_WEIGHT` at the focus and half
/// that `FOCUS_SCALE_KM` away, so the nearest Starbucks comes first. The
/// second, for administrative areas only, is regional, fading over hundreds
/// of kilometres, so of two towns with the same name the one in your part of
/// the world wins, unless the other is far better known. POIs nearby are
/// already told apart by the first, and giving them the second too would let
/// them swamp a better-matching city a little further off. The factors
/// multiply so that the regional one doesn't water down the local one. Like
/// importance, neither can lift a poor match far above a good one: a city's
/// exact name outweighs a nearby place that only shares a word with it.
const FOCUS_WEIGHT: f32 = 1.5;
const FOCUS_SCALE_KM: f64 = 5.0;
const REGION_WEIGHT: f32 = 0.75;
const REGION_SCALE_KM: f64 = 300.0;
pub(crate) const EARTH_RADIUS_KM: f64 = 6371.0088;

/// Which fields `AirmailIndex::text_query_in` matches words in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fields {
    /// Names, addresses and areas: the query taken literally.
    All,
    /// Names: what's sought.
    Name,
    /// Street names, a street's own and a place's address's.
    Road,
    /// Postcodes, a postcode's own and a place's address's.
    Postcode,
}

/// A query's words as `AirmailIndex::text_query` builds them.
struct TextQueries {
    /// Matches documents with all the words.
    all: Box<dyn Query>,
    /// Each word, for scoring category matches.
    any_text: Vec<Box<dyn Query>>,
    /// Each word with admin matches weighted by `CATEGORY_ADMIN_BOOST`.
    any_text_located: Vec<Box<dyn Query>>,
}

#[derive(Serialize, Deserialize)]
struct VersionFile {
    schema_version: u32,
}

/// A directory that's only searched. Readers lock `meta.json` against
/// concurrent commits with a lock file, which can't be created on a read-only
/// filesystem; with no writer there's nothing to lock against.
#[derive(Clone, Debug)]
struct ReadOnlyDirectory(MmapDirectory);

impl Directory for ReadOnlyDirectory {
    fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
        self.0.get_file_handle(path)
    }

    fn delete(&self, path: &Path) -> Result<(), DeleteError> {
        self.0.delete(path)
    }

    fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
        self.0.exists(path)
    }

    fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        self.0.open_write(path)
    }

    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        self.0.atomic_read(path)
    }

    fn atomic_write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        self.0.atomic_write(path, data)
    }

    fn sync_directory(&self) -> std::io::Result<()> {
        self.0.sync_directory()
    }

    fn acquire_lock(&self, _lock: &Lock) -> Result<DirectoryLock, LockError> {
        Ok(DirectoryLock::from(Box::new(())))
    }

    fn watch(&self, watch_callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        self.0.watch(watch_callback)
    }
}

/// What to search for.
#[derive(Debug, Clone, Default)]
pub struct SearchRequest<'a> {
    pub text: &'a str,
    /// Match the last word as a prefix.
    pub prefix: bool,
    /// Tolerate typos in every word.
    pub typos: bool,
    /// Category tags results must all have, as `key=value`.
    pub tags: Vec<String>,
    pub categories: Option<(&'a QueryCategories, CategoryMode)>,
    /// `[min_lon, min_lat, max_lon, max_lat]` results' shapes must intersect.
    pub bbox: Option<[f64; 4]>,
    /// A Who's On First ID results must be inside.
    pub within: Option<u64>,
    /// Kinds to return; all if empty.
    pub kinds: Vec<Kind>,
    pub limit: usize,
    /// `[lon, lat]` to rank results near higher.
    pub focus: Option<[f64; 2]>,
    /// The tagger's interpretations of `text`, from `AirmailIndex::interpret`,
    /// if they've been made already.
    pub interpretations: Option<&'a [airmail_tagger::Interpretation]>,
}

/// A search result.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub score: f32,
    pub kind: Kind,
    /// OSM tags, without Cardinal's `cardinal:*` tags, which are in `poi`.
    pub tags: Vec<(String, String)>,
    /// Cardinal's stable identifier and the OSM version it was indexed at.
    pub poi: Option<(u64, u32)>,
    pub osm: Option<OsmRef>,
    pub wof: Option<WofPlace>,
    /// Who's On First IDs of the areas containing it.
    pub admin_ids: Vec<u64>,
    pub center: Point<f64>,
    pub geometry: Geometry<f64>,
    pub importance: f64,
}

impl SearchHit {
    /// The OSM tag's value, if it has the tag.
    #[must_use]
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Results, with the administrative areas they're in.
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub admins: HashMap<u64, WofPlace>,
}

#[derive(Clone)]
pub struct AirmailIndex {
    pub(crate) tantivy_index: Arc<tantivy::Index>,
    /// One reader for the index's life, made when first needed: making one
    /// opens every segment, far too slow to do for each search.
    reader: Arc<std::sync::OnceLock<tantivy::IndexReader>>,
    /// Reads queries, if a model was given.
    tagger: Option<Arc<airmail_tagger::Tagger>>,
}

impl AirmailIndex {
    fn schema() -> tantivy::schema::Schema {
        let mut schema_builder = Schema::builder();
        let text = |positions: bool| {
            TextOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    // Places have many names in many languages, and shouldn't
                    // be penalized for it.
                    .set_fieldnorms(false)
                    .set_tokenizer(crate::text::TOKENIZER)
                    .set_index_option(if positions {
                        IndexRecordOption::WithFreqsAndPositions
                    } else {
                        IndexRecordOption::WithFreqs
                    }),
            )
        };
        let raw = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_fieldnorms(false)
                .set_tokenizer("raw")
                .set_index_option(IndexRecordOption::Basic),
        );
        let indexed_u64 = NumericOptions::default().set_indexed();

        schema_builder.add_text_field(FIELD_NAME, text(true));
        schema_builder.add_text_field(FIELD_NAME_EXACT, raw.clone());
        schema_builder.add_text_field(FIELD_STREET, text(true));
        schema_builder.add_text_field(FIELD_HOUSENUMBER, text(false));
        schema_builder.add_text_field(FIELD_UNIT, text(false));
        schema_builder.add_text_field(FIELD_POSTCODE, text(false));
        for band in AdminBand::ALL {
            schema_builder.add_text_field(admin_field(band), text(false));
        }
        schema_builder.add_text_field(FIELD_INDEXED_TAG, raw.clone());
        schema_builder.add_text_field(FIELD_AREA_NAME, raw);
        schema_builder.add_text_field(FIELD_PLACETYPE, STRING);
        schema_builder.add_text_field(FIELD_COUNTRY_CODE, STRING);
        schema_builder.add_text_field(FIELD_KIND, STRING | STORED);
        schema_builder.add_u64_field(FIELD_IS_ADMIN, FAST);
        schema_builder.add_text_field(FIELD_SOURCE, STRING | STORED);
        schema_builder.add_u64_field(FIELD_ADMIN_IDS, indexed_u64.clone().set_stored());
        schema_builder.add_u64_field(FIELD_WOF_ID, indexed_u64.clone());
        schema_builder.add_u64_field(FIELD_POI_ID, indexed_u64.clone());
        schema_builder.add_text_field(FIELD_OSM_ID, STRING);
        schema_builder.add_text_field(FIELD_POSTCODE_ID, STRING);
        schema_builder.add_u64_field(FIELD_S2_COVER, indexed_u64.clone());
        schema_builder.add_u64_field(FIELD_S2_COVER_ANCESTORS, indexed_u64);
        schema_builder.add_text_field(FIELD_OSM, STORED);
        schema_builder.add_text_field(FIELD_WOF, STORED);
        schema_builder.add_json_field(FIELD_TAGS, STORED);
        // Fast for ranking by distance from a focus point.
        schema_builder.add_u64_field(FIELD_CENTER, FAST | STORED);
        schema_builder.add_bytes_field(FIELD_GEOMETRY, STORED);
        schema_builder.add_f64_field(FIELD_IMPORTANCE, FAST | STORED);
        schema_builder.build()
    }

    pub(crate) fn field(&self, name: &str) -> tantivy::schema::Field {
        self.tantivy_index.schema().get_field(name).expect("schema field")
    }

    /// Opens the index in `index_dir` for writing, creating it if there's
    /// none.
    ///
    /// # Errors
    /// If there's an index of another schema version there.
    pub fn create(index_dir: &Path) -> Result<Self> {
        if !index_dir.exists() {
            trace!("Creating index at {:?}", index_dir);
            std::fs::create_dir_all(index_dir)?;
        }
        let version_path = index_dir.join(VERSION_FILE);
        if index_dir.join("meta.json").exists() {
            check_version(index_dir)?;
        } else {
            std::fs::write(&version_path, serde_json::to_vec(&VersionFile { schema_version: SCHEMA_VERSION })?)?;
        }
        let tantivy_index = tantivy::Index::open_or_create(MmapDirectory::open(index_dir)?, Self::schema())?;
        register_tokenizers(&tantivy_index);
        Ok(Self { tantivy_index: Arc::new(tantivy_index), reader: Arc::default(), tagger: None })
    }

    /// Opens an existing index.
    ///
    /// # Errors
    /// If there's no index of this schema version there.
    pub fn new(index_dir: &str) -> Result<Self> {
        check_version(Path::new(index_dir))?;
        let tantivy_index = tantivy::Index::open_in_dir(index_dir)?;
        register_tokenizers(&tantivy_index);
        Ok(Self { tantivy_index: Arc::new(tantivy_index), reader: Arc::default(), tagger: None })
    }

    /// Opens an index for searching only, which works on a read-only
    /// filesystem. Nothing may write to the index while it's open.
    ///
    /// # Errors
    /// If there's no index of this schema version there.
    pub fn open_read_only(index_dir: &str) -> Result<Self> {
        check_version(Path::new(index_dir))?;
        let directory = ReadOnlyDirectory(MmapDirectory::open(index_dir)?);
        let tantivy_index = tantivy::Index::open(directory)?;
        register_tokenizers(&tantivy_index);
        Ok(Self { tantivy_index: Arc::new(tantivy_index), reader: Arc::default(), tagger: None })
    }

    /// Reads queries with a tagger's interpretations of them, rather than by
    /// finding area names at their ends.
    #[must_use]
    /// A searcher of the index as it is now, from its one reader.
    ///
    /// # Errors
    /// If the index can't be read.
    pub fn searcher(&self) -> tantivy::Result<tantivy::Searcher> {
        if let Some(reader) = self.reader.get() {
            return Ok(reader.searcher());
        }
        let reader = self.tantivy_index.reader()?;
        Ok(self.reader.get_or_init(|| reader).searcher())
    }

    pub fn with_tagger(mut self, tagger: airmail_tagger::Tagger) -> Self {
        self.tagger = Some(Arc::new(tagger));
        self
    }

    pub fn writer(&mut self) -> Result<AirmailIndexWriter> {
        let tantivy_writer = self.tantivy_index.writer::<TantivyDocument>(2_000_000_000)?;
        Ok(AirmailIndexWriter { tantivy_writer, schema: self.tantivy_index.schema() })
    }

    pub async fn merge(&mut self) -> Result<()> {
        let ids = self.tantivy_index.searchable_segment_ids()?;
        self.tantivy_index.writer::<TantivyDocument>(2_000_000_000)?.merge(&ids).await?;
        Ok(())
    }

    pub async fn num_docs(&self) -> Result<u64> {
        let index = self.tantivy_index.clone();
        let count = spawn_blocking(move || index.reader().ok().map(|reader| reader.searcher().num_docs()));
        Ok(count.await?.ok_or(AirmailError::UnableToCount)?)
    }

    /// The words of `text` as the name field indexes them.
    pub(crate) fn tokens(&self, text: &str) -> Vec<String> {
        let mut analyzer = self.tantivy_index.tokenizer_for_field(self.field(FIELD_NAME)).expect("tokenizer");
        let mut tokens = Vec::new();
        let normalized = normalize_text(text);
        let mut stream = analyzer.token_stream(&normalized);
        stream.process(&mut |token| tokens.push(token.text.clone()));
        tokens
    }

    /// The query's words, by comma-separated section.
    fn query_words(&self, text: &str) -> QueryWords {
        let text = text.replace("'s", "s");
        QueryWords::from_sections(text.split(',').map(|section| self.tokens(section)).collect())
    }

    /// Whether an administrative area is named exactly `name`, and if so the
    /// greatest importance of one that is.
    fn admin_importance(&self, searcher: &Searcher, name: &str) -> Option<f64> {
        let term = Term::from_field_text(self.field(FIELD_NAME_EXACT), name);
        if searcher.doc_freq(&term).ok()? == 0 {
            return None;
        }
        let query = BooleanQuery::intersection(vec![
            Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            Box::new(TermQuery::new(
                Term::from_field_text(self.field(FIELD_KIND), Kind::Admin.as_str()),
                IndexRecordOption::Basic,
            )),
        ]);
        let collector = TopDocs::with_limit(1).order_by_fast_field::<f64>(FIELD_IMPORTANCE, Order::Desc);
        searcher.search(&query, &collector).ok()?.into_iter().next().map(|(importance, _)| importance.unwrap_or(0.0))
    }

    /// The query's readings with context (see `interpret`), with the
    /// parser's hints.
    fn readings(&self, searcher: &Searcher, words: &QueryWords, text: &str, prefix: bool) -> (Hints, Vec<Reading>) {
        let hints = Hints::parse(text, |t| self.tokens(t));
        let lookup = IndexLookup { index: self, searcher };
        let readings = interpret::readings(words, &hints, &lookup, prefix);
        (hints, readings)
    }

    /// The tagger's interpretations of a query, most likely first, with the
    /// words as the index reads them, if there's a tagger. `prefix` says the
    /// last word may be unfinished.
    #[must_use]
    pub fn interpret(&self, text: &str, prefix: bool, focus: Option<[f64; 2]>) -> Option<(Vec<String>, Vec<airmail_tagger::Interpretation>)> {
        let (words, readings) = self.tagged(text, prefix, focus, MAX_INTERPRETATIONS, INTERPRETATION_COVERAGE);
        Some((words, readings?))
    }

    /// The query's words, and the tagger's likeliest readings of them if
    /// there's a tagger, until together they're at least `coverage` likely or
    /// there are `max` of them.
    pub(crate) fn tagged(&self, text: &str, prefix: bool, focus: Option<[f64; 2]>, max: usize, coverage: f64) -> (Vec<String>, Option<Vec<airmail_tagger::Interpretation>>) {
        let words = self.query_words(text);
        let readings = self.tagger.as_ref().and_then(|tagger| {
            let prefix = prefix && !text.ends_with(char::is_whitespace);
            let mut query = airmail_tagger::tagger::query(text, prefix, focus);
            if query.words.len() != words.words.len() {
                return None;
            }
            query.words.clone_from(&words.words);
            let areas = airmail_tagger::features::Gazetteer::areas(&crate::gazetteer::IndexGazetteer::new(self).ok()?, &query);
            Some(tagger.interpret(&query, &areas, max, coverage))
        });
        (words.words, readings)
    }

    /// The interpretations of a request's query: given, or made.
    fn interpretations(&self, request: &SearchRequest) -> Option<Vec<airmail_tagger::Interpretation>> {
        match request.interpretations {
            Some(given) => Some(given.to_vec()),
            None => self.interpret(request.text, request.prefix, request.focus).map(|(_, i)| i),
        }
    }

    /// Matches one query word in one field.
    pub(crate) fn word_query(&self, field: &str, word: &str, prefix: bool, typos: bool) -> Box<dyn Query> {
        let term = Term::from_field_text(self.field(field), word);
        let distance = match (typos, prefix) {
            (false, _) => 0,
            (true, false) => typo_distance(word),
            (true, true) => typing_typo_distance(word),
        };
        // Without term frequencies, so a place with its name in fifty
        // languages scores no higher than one with a single name.
        let exact: Box<dyn Query> = Box::new(TermQuery::new(term.clone(), IndexRecordOption::Basic));
        if prefix {
            // A word still being typed: every word it begins matches alike,
            // with no bonus for the rare word that's exactly the text so far,
            // so `londo` is London before an obscure place called Londo. A
            // word it begins as typed counts a little more than one it
            // begins with a typo, so `wales` is Wales.
            let fuzzy: Box<dyn Query> = Box::new(FuzzyTermQuery::new_prefix(term.clone(), distance, true));
            if distance == 0 {
                return fuzzy;
            }
            let as_typed = Box::new(FuzzyTermQuery::new_prefix(term, 0, true));
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, fuzzy),
                (Occur::Should, Box::new(ConstScoreQuery::new(as_typed, TYPED_PREFIX_SCORE))),
            ]))
        } else if distance > 0 {
            with_exact_score(Box::new(FuzzyTermQuery::new(term.clone(), distance, true)), term)
        } else {
            exact
        }
    }

    /// Requires every word of the query, each in some text field, and ranks
    /// phrase and exact name matches higher. Also returns the per-word
    /// queries, for ranking category matches.
    /// Every word in some text field.
    pub(crate) fn any_field_query(&self, tokens: &[String], prefix: bool, typos: bool) -> Box<dyn Query> {
        self.text_query(tokens, prefix, typos).all
    }

    fn text_query(&self, tokens: &[String], prefix: bool, typos: bool) -> TextQueries {
        self.text_query_in(tokens, prefix, typos, Fields::All)
    }

    /// `text_query` matching words in only some fields.
    fn text_query_in(&self, tokens: &[String], prefix: bool, typos: bool, fields: Fields) -> TextQueries {
        let fields_with = |admin_boost: f32| -> Vec<(&'static str, f32)> {
            match fields {
                Fields::All => std::iter::once((FIELD_NAME, NAME_BOOST))
                    .chain(ADDRESS_FIELDS.map(|field| (field, ADDRESS_BOOST)))
                    .chain(AdminBand::ALL.map(|band| (admin_field(band), admin_boost)))
                    .collect(),
                Fields::Name => vec![(FIELD_NAME, NAME_BOOST)],
                Fields::Road => vec![(FIELD_NAME, NAME_BOOST), (FIELD_STREET, ADDRESS_BOOST)],
                Fields::Postcode => vec![(FIELD_NAME, NAME_BOOST), (FIELD_POSTCODE, ADDRESS_BOOST)],
            }
        };
        let words_with = |admin_boost: f32| -> Vec<Box<dyn Query>> {
            tokens
                .iter()
                .enumerate()
                .map(|(i, word)| {
                    let last = i + 1 == tokens.len();
                    let query = |(field, boost): (&str, f32)| -> Box<dyn Query> {
                        Box::new(BoostQuery::new(self.word_query(field, word, prefix && last, typos), boost))
                    };
                    let (bands, others): (Vec<_>, Vec<_>) =
                        fields_with(admin_boost).into_iter().partition(|(field, _)| AdminBand::ALL.iter().any(|b| admin_field(*b) == *field));
                    let mut per_field: Vec<Box<dyn Query>> = others.into_iter().map(query).collect();
                    // A word naming areas counts once, by its best band: a
                    // place in New York, New York County and New York State
                    // isn't three times as much in New York.
                    if !bands.is_empty() {
                        per_field.push(Box::new(DisjunctionMaxQuery::new(bands.into_iter().map(query).collect())));
                    }
                    Box::new(BooleanQuery::union(per_field)) as Box<dyn Query>
                })
                .collect()
        };
        let words = words_with(ADMIN_BOOST);

        let mut optional: Vec<Box<dyn Query>> = Vec::new();
        for subsequence in all_subsequences(tokens) {
            if subsequence.len() < 2 {
                continue;
            }
            let ends_query = tokens.ends_with(&subsequence);
            let boost = 1.05f32.powf(subsequence.iter().map(String::len).sum::<usize>() as f32);
            for (field, field_boost) in [(FIELD_NAME, NAME_BOOST), (FIELD_STREET, ADDRESS_BOOST)] {
                if !fields_with(ADMIN_BOOST).iter().any(|(f, _)| *f == field) {
                    continue;
                }
                let terms: Vec<Term> =
                    subsequence.iter().map(|word| Term::from_field_text(self.field(field), word)).collect();
                let phrase: Box<dyn Query> = if prefix && ends_query {
                    Box::new(PhrasePrefixQuery::new(terms))
                } else {
                    Box::new(PhraseQuery::new(terms))
                };
                optional.push(Box::new(BoostQuery::new(phrase, boost * field_boost)));
            }
        }
        // Whole-name matches: all the words, or the first few with the rest
        // naming where it is.
        for end in (1..=tokens.len()).rev() {
            let key = tokens[..end].join(" ");
            let term = Term::from_field_text(self.field(FIELD_NAME_EXACT), &key);
            // While the last word may be unfinished, a name that's exactly the
            // text so far is no likelier meant than one it begins, and an
            // exact match's score grows with how rare the name is: `londo`
            // would be a place called Londo before London. The prefix match
            // below counts both alike, leaving importance and nearness.
            let exact = Box::new(TermQuery::new(term.clone(), IndexRecordOption::Basic));
            if prefix && end == tokens.len() {
                // Still, the name as typed is a little likelier: `wales` is
                // Wales before somewhere it begins.
                optional.push(Box::new(ConstScoreQuery::new(exact, EXACT_NAME_WHILE_TYPING_SCORE)));
            } else if end < tokens.len() {
                // The first words being exactly a name is so or not, however
                // rare the name: scaled by rarity, the streets named `New`
                // outscore New York City for `new york`. While the last word
                // may be unfinished, no more than the whole text being a
                // name counts.
                let score = if prefix { EXACT_NAME_WHILE_TYPING_SCORE } else { EXACT_NAME_PREFIX_SCORE };
                optional.push(Box::new(ConstScoreQuery::new(exact, score)));
            } else {
                optional.push(Box::new(BoostQuery::new(exact, EXACT_NAME_BOOST)));
            }
            // The whole query as a name with typos, or a name's prefix.
            if end == tokens.len() && (prefix || typos) {
                let distance = match (typos, prefix) {
                    (false, _) => 0,
                    (true, false) => typo_distance(&key),
                    (true, true) => typing_typo_distance(&key),
                };
                let near: Box<dyn Query> = if prefix {
                    Box::new(FuzzyTermQuery::new_prefix(term, distance, true))
                } else {
                    Box::new(FuzzyTermQuery::new(term, distance, true))
                };
                optional.push(Box::new(BoostQuery::new(near, NEAR_EXACT_NAME_BOOST)));
            }
        }

        let any_text = words.iter().map(|q| q.box_clone()).collect();
        // Some word in its own name or address: a place matching only by
        // where it is (`new york` for everything in New York) can't rank,
        // and there can be millions of them to score.
        let own: Vec<Box<dyn Query>> = tokens
            .iter()
            .enumerate()
            .flat_map(|(i, word)| {
                let last = i + 1 == tokens.len();
                std::iter::once(FIELD_NAME)
                    .chain(ADDRESS_FIELDS)
                    .map(move |field| self.word_query(field, word, prefix && last, typos))
            })
            .collect();
        let all = Box::new(BooleanQuery::new(vec![
            (Occur::Must, Box::new(BooleanQuery::intersection(words)) as Box<dyn Query>),
            (Occur::Must, Box::new(ConstScoreQuery::new(Box::new(BooleanQuery::union(own)), 0.0))),
            (Occur::Should, Box::new(BooleanQuery::union(optional))),
        ]));
        TextQueries { all, any_text, any_text_located: words_with(CATEGORY_ADMIN_BOOST) }
    }

    /// Matches documents whose covering intersects the S2 cells.
    pub(crate) fn covering_query(&self, cells: &[s2::cellid::CellID]) -> Box<dyn Query> {
        let mut inside = BTreeSet::new();
        let mut containing = BTreeSet::new();
        for cell in cells {
            // Documents with a covering cell inside this one.
            inside.insert(cell.0);
            // Documents with a covering cell containing this one.
            containing.insert(cell.0);
            containing.extend(geometry::ancestors(*cell).into_iter().map(|c| c.0));
        }
        let term_query = |field: &str, value: u64| -> Box<dyn Query> {
            Box::new(TermQuery::new(Term::from_field_u64(self.field(field), value), IndexRecordOption::Basic))
        };
        let mut clauses: Vec<Box<dyn Query>> = inside.into_iter().map(|c| term_query(FIELD_S2_COVER_ANCESTORS, c)).collect();
        clauses.extend(containing.into_iter().map(|c| term_query(FIELD_S2_COVER, c)));
        Box::new(BooleanQuery::union(clauses))
    }

    /// Matches every word of the query taken literally, or the subject of
    /// one of its readings in that reading's context, scoring each result by
    /// the reading it fits best.
    ///
    /// With `fallback`, the area a reading's subject is in is a match too,
    /// scoring far less, for when nothing there matches the subject. Not for
    /// category searches, where finding no bookshop in Ballard shouldn't
    /// find Ballard.
    fn reading_query(
        &self,
        words: &QueryWords,
        readings: &[Reading],
        literal: Box<dyn Query>,
        typos: bool,
        fallback: bool,
    ) -> Box<dyn Query> {
        if readings.is_empty() {
            return literal;
        }
        let mut alternatives: Vec<Box<dyn Query>> =
            vec![Box::new(BoostQuery::new(literal, interpret::literal_weight(words, readings)))];
        for reading in readings {
            // The subject never has the last word, so it's finished.
            let subject = self.text_query(&words.words[..reading.subject], false, typos).all;
            let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, subject)];
            for part in &reading.context {
                match part {
                    Part::Area { words, country } => clauses.push((Occur::Must, self.area_query(words, country.as_deref()))),
                    Part::Postcode(word) => {
                        let term = Term::from_field_text(self.field(FIELD_POSTCODE), word);
                        let matches = Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                        clauses.push((Occur::Should, Box::new(ConstScoreQuery::new(matches, POSTCODE_SCORE))));
                    }
                    Part::Unknown => {}
                }
            }
            let boost = reading.weight * (1.0 + AREA_BONUS * reading.area_confidence);
            alternatives.push(Box::new(BoostQuery::new(Box::new(BooleanQuery::new(clauses)), boost)));
        }
        // The area alone, in the rest of the context, for when nothing in it
        // matches the subject: only after a comma, which says plainly what
        // the area is.
        if fallback
            && let Some(reading) = readings.first()
            && words.section_starts[reading.subject]
            && let Some(Part::Area { words: area, .. }) = reading.context.first()
        {
            let named = Term::from_field_text(self.field(FIELD_NAME_EXACT), &area.join(" "));
            let admin = Term::from_field_text(self.field(FIELD_KIND), Kind::Admin.as_str());
            let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![
                (Occur::Must, Box::new(BoostQuery::new(Box::new(TermQuery::new(named, IndexRecordOption::Basic)), EXACT_NAME_BOOST))),
                (Occur::Must, Box::new(ConstScoreQuery::new(Box::new(TermQuery::new(admin, IndexRecordOption::Basic)), 0.0))),
            ];
            for part in &reading.context[1..] {
                if let Part::Area { words, country } = part {
                    clauses.push((Occur::Must, self.area_query(words, country.as_deref())));
                }
            }
            alternatives.push(Box::new(BoostQuery::new(Box::new(BooleanQuery::new(clauses)), AREA_FALLBACK_WEIGHT)));
        }
        Box::new(DisjunctionMaxQuery::new(alternatives))
    }

    /// Matches what one interpretation of the query says it's for: the words
    /// it reads as what's sought in names, a road in streets, a house number
    /// and postcode in the address, and areas in the admins of their bands.
    /// With nothing sought but areas, the first area is what's sought, in
    /// the others. `None` if it reads nothing to search for.
    fn interpretation_query(&self, words: &[String], tags: &[Tag], prefix: bool, typos: bool) -> Option<Box<dyn Query>> {
        let n = words.len();
        let of = |tag: Tag| -> Vec<usize> { (0..n).filter(|&i| tags[i] == tag).collect() };
        let words_at = |indexes: &[usize]| -> Vec<String> { indexes.iter().map(|&i| words[i].clone()).collect() };
        let typing = |indexes: &[usize]| prefix && indexes.last() == Some(&(n - 1));
        // Areas, as runs of words with the same band.
        let mut areas: Vec<(Tag, Vec<usize>)> = Vec::new();
        for i in 0..n {
            if tags[i].band().is_none() {
                continue;
            }
            match areas.last_mut() {
                Some((tag, run)) if *tag == tags[i] && run.last() == Some(&(i - 1)) => run.push(i),
                _ => areas.push((tags[i], vec![i])),
            }
        }
        let (venue, road, numbers, units, postcodes) = (of(Tag::Venue), of(Tag::Road), of(Tag::HouseNumber), of(Tag::Unit), of(Tag::Postcode));
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        let subject = !venue.is_empty() || !road.is_empty() || !numbers.is_empty();
        if !venue.is_empty() {
            clauses.push((Occur::Must, self.text_query_in(&words_at(&venue), typing(&venue), typos, Fields::Name).all));
        }
        if !road.is_empty() {
            clauses.push((Occur::Must, self.text_query_in(&words_at(&road), typing(&road), typos, Fields::Road).all));
        }
        for &i in &numbers {
            let number = self.word_query(FIELD_HOUSENUMBER, &words[i], prefix && i == n - 1, false);
            clauses.push((Occur::Must, Box::new(BoostQuery::new(number, ADDRESS_BOOST))));
        }
        for &i in &units {
            let unit = Box::new(TermQuery::new(Term::from_field_text(self.field(FIELD_UNIT), &words[i]), IndexRecordOption::Basic));
            clauses.push((Occur::Should, Box::new(ConstScoreQuery::new(unit, POSTCODE_SCORE))));
        }
        if !postcodes.is_empty() {
            if subject {
                for &i in &postcodes {
                    let term = Term::from_field_text(self.field(FIELD_POSTCODE), &words[i]);
                    let matches = Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                    clauses.push((Occur::Should, Box::new(ConstScoreQuery::new(matches, POSTCODE_SCORE))));
                }
            } else {
                clauses.push((Occur::Must, self.text_query_in(&words_at(&postcodes), typing(&postcodes), typos, Fields::Postcode).all));
            }
        }
        let mut context = areas.as_slice();
        if !subject && postcodes.is_empty() {
            // Only areas: the first is what's sought.
            let ((tag, run), rest) = areas.split_first()?;
            clauses.push((Occur::Must, self.text_query_in(&words_at(run), typing(run), typos, Fields::Name).all));
            let admin = Term::from_field_text(self.field(FIELD_KIND), Kind::Admin.as_str());
            clauses.push((Occur::Must, Box::new(ConstScoreQuery::new(Box::new(TermQuery::new(admin, IndexRecordOption::Basic)), 0.0))));
            let band = Term::from_field_text(self.field(FIELD_PLACETYPE), tag.as_str());
            clauses.push((Occur::Should, Box::new(ConstScoreQuery::new(Box::new(TermQuery::new(band, IndexRecordOption::Basic)), BAND_SCORE))));
            context = rest;
        }
        for (tag, run) in context {
            let area = words_at(run);
            let country = (*tag == Tag::Country && area.len() == 1).then(|| crate::countries::alpha2(&area[0])).flatten();
            clauses.push((Occur::Must, self.area_query(&area, country)));
            // In the band the tagger read, more.
            let band = AdminBand::ALL.into_iter().find(|b| b.as_str() == tag.as_str()).expect("an area's band");
            let in_band: Vec<Box<dyn Query>> = area
                .iter()
                .map(|word| -> Box<dyn Query> {
                    Box::new(TermQuery::new(Term::from_field_text(self.field(admin_field(band)), word), IndexRecordOption::Basic))
                })
                .collect();
            clauses.push((Occur::Should, Box::new(ConstScoreQuery::new(Box::new(BooleanQuery::intersection(in_band)), BAND_SCORE))));
        }
        clauses.iter().any(|(occur, _)| *occur == Occur::Must).then(|| Box::new(BooleanQuery::new(clauses)) as Box<dyn Query>)
    }

    /// The query taken literally and as each of the tagger's interpretations
    /// that put what's sought in areas (or an area in others), scoring each
    /// result by its best. An interpretation's score is weighted by the root
    /// of its probability, and being in its areas counts `1 + AREA_BONUS`;
    /// the literal query is discounted by how likely such interpretations
    /// are, as `interpret::literal_weight` does readings by their confidence.
    /// An interpretation without areas adds nothing the literal query
    /// doesn't do.
    fn interpretations_query(
        &self,
        words: &[String],
        interpretations: &[airmail_tagger::Interpretation],
        literal: Box<dyn Query>,
        prefix: bool,
        typos: bool,
    ) -> Box<dyn Query> {
        // Whether an interpretation puts something in an area: an area and
        // something else sought, or two areas.
        let in_areas = |tags: &[Tag]| {
            let areas = tags.iter().filter(|t| t.band().is_some()).count();
            areas > 0 && (areas < tags.iter().filter(|t| !matches!(t, Tag::Near | Tag::Other)).count() || {
                let mut bands: Vec<Tag> = tags.iter().copied().filter(|t| t.band().is_some()).collect();
                bands.dedup();
                bands.len() > 1
            })
        };
        let mut alternatives: Vec<Box<dyn Query>> = Vec::new();
        let mut placed = 0.0;
        for interpretation in interpretations.iter().filter(|i| in_areas(&i.tags)) {
            let Some(query) = self.interpretation_query(words, &interpretation.tags, prefix, typos) else { continue };
            placed += interpretation.probability as f32;
            let weight = interpretation.probability.sqrt() as f32 * (1.0 + AREA_BONUS);
            alternatives.push(Box::new(BoostQuery::new(query, weight)));
        }
        if alternatives.is_empty() {
            return literal;
        }
        alternatives.push(Box::new(BoostQuery::new(literal, 1.0 - LITERAL_DISCOUNT * placed.min(1.0))));
        Box::new(DisjunctionMaxQuery::new(alternatives))
    }

    /// Matches places in an area named `words`, or in `country`, scoring
    /// nothing.
    fn area_query(&self, words: &[String], country: Option<&str>) -> Box<dyn Query> {
        let named: Box<dyn Query> = Box::new(BooleanQuery::intersection(
            words
                .iter()
                .map(|word| -> Box<dyn Query> {
                    Box::new(BooleanQuery::union(
                        AdminBand::ALL
                            .iter()
                            .map(|band| -> Box<dyn Query> {
                                let term = Term::from_field_text(self.field(admin_field(*band)), word);
                                Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                            })
                            .collect(),
                    ))
                })
                .collect(),
        ));
        let mut either = vec![named];
        if let Some(country) = country {
            let term = Term::from_field_text(self.field(FIELD_COUNTRY_CODE), country);
            either.push(Box::new(TermQuery::new(term, IndexRecordOption::Basic)));
        }
        Box::new(ConstScoreQuery::new(Box::new(BooleanQuery::union(either)), 0.0))
    }

    fn construct_query(
        &self,
        request: &SearchRequest,
        bbox_cells: Option<&[s2::cellid::CellID]>,
        words: &QueryWords,
        hints: &Hints,
        readings: &[Reading],
        interpretations: Option<&[airmail_tagger::Interpretation]>,
    ) -> Box<dyn Query> {
        let tokens = &words.words;
        let prefix = Self::prefix(request);
        // An address is never a category search, however its words read:
        // the model may take `clayallee 170 14191 berlin` for apartments.
        let categories = request.categories.filter(|_| !hints.address);
        let category_search = matches!(categories, Some((understood, CategoryMode::Include)) if !understood.is_empty());
        let mut text = self.text_query(tokens, prefix, request.typos);
        text.all = match interpretations {
            Some(interpretations) if !interpretations.is_empty() => {
                self.interpretations_query(tokens, interpretations, text.all, prefix, request.typos)
            }
            _ => self.reading_query(words, readings, text.all, request.typos, !category_search),
        };
        // An address, as the tagger reads it if there's one, else the parser.
        let address = match interpretations.and_then(|i| i.first()) {
            Some(best) => best.tags.contains(&Tag::HouseNumber) && best.tags.contains(&Tag::Road),
            None => hints.address,
        };
        if address {
            // The best of the score and, for addresses, more.
            let address = Box::new(TermQuery::new(
                Term::from_field_text(self.field(FIELD_KIND), Kind::Address.as_str()),
                IndexRecordOption::Basic,
            ));
            let as_address = Box::new(BooleanQuery::new(vec![
                (Occur::Must, text.all.box_clone()),
                (Occur::Must, Box::new(ConstScoreQuery::new(address, 0.0)) as Box<dyn Query>),
            ]));
            text.all = Box::new(DisjunctionMaxQuery::new(vec![
                text.all,
                Box::new(BoostQuery::new(as_address, ADDRESS_KIND_FACTOR)),
            ]));
        }

        // A query matches either by its words or by the categories it asks
        // for. A POI with all the words is scored as a name match, so the
        // Spokane station doesn't outrank Spokane for being in Spokane.
        let mut query: Box<dyn Query> = match categories {
            Some((categories, mode)) => match self.category_query(categories, text.any_text) {
                Some(category_query) => {
                    let matched = Box::new(BooleanQuery::new(vec![
                        (Occur::Must, text.all.box_clone()),
                        (Occur::Should, category_query),
                    ]));
                    match (mode, self.category_query(categories, text.any_text_located)) {
                        (CategoryMode::Include, Some(located)) => {
                            // A place found by its category alone is in the
                            // areas the query names, if it names any:
                            // `bakery fremont` is bakeries in Fremont, not
                            // places called Bakery elsewhere.
                            let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, located), (Occur::MustNot, text.all)];
                            let in_areas = readings
                                .iter()
                                .filter(|r| r.has_area())
                                .max_by(|a, b| a.area_confidence.total_cmp(&b.area_confidence));
                            for part in in_areas.map(|r| r.context.as_slice()).unwrap_or_default() {
                                if let Part::Area { words, country } = part {
                                    clauses.push((Occur::Must, self.area_query(words, country.as_deref())));
                                }
                            }
                            Box::new(BooleanQuery::union(vec![matched, Box::new(BooleanQuery::new(clauses))]))
                        }
                        _ => matched,
                    }
                }
                None => text.all,
            },
            None => text.all,
        };

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for tag in &request.tags {
            let term = Term::from_field_text(self.field(FIELD_INDEXED_TAG), tag);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }
        if !request.kinds.is_empty() && request.kinds.len() < Kind::ALL.len() {
            let kinds = request
                .kinds
                .iter()
                .map(|kind| -> Box<dyn Query> {
                    let term = Term::from_field_text(self.field(FIELD_KIND), kind.as_str());
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                })
                .collect();
            clauses.push((Occur::Must, Box::new(BooleanQuery::union(kinds))));
        }
        if let Some(within) = request.within {
            let inside = Term::from_field_u64(self.field(FIELD_ADMIN_IDS), within);
            let itself = Term::from_field_u64(self.field(FIELD_WOF_ID), within);
            clauses.push((Occur::Must, Box::new(TermQuery::new(inside, IndexRecordOption::Basic))));
            clauses.push((Occur::MustNot, Box::new(TermQuery::new(itself, IndexRecordOption::Basic))));
        }
        if let Some(cells) = bbox_cells {
            clauses.push((Occur::Must, self.covering_query(cells)));
        }
        if !clauses.is_empty() {
            clauses.insert(0, (Occur::Must, query));
            query = Box::new(BooleanQuery::new(clauses));
        }
        query
    }

    /// Whether the last word may be unfinished. A space after it means it's
    /// finished, however the text came.
    fn prefix(request: &SearchRequest) -> bool {
        request.prefix && !request.text.ends_with(char::is_whitespace)
    }

    /// Matches POIs with the most specific confident signal in `understood`
    /// (cuisines, else categories, else diets), boosted by confidence. Diets
    /// that aren't the primary signal and the query's words only add to the
    /// score, since diet tags are sparse.
    fn category_query(&self, understood: &QueryCategories, any_text: Vec<Box<dyn Query>>) -> Option<Box<dyn Query>> {
        let tag_query = |tag: &str, boost: f32| -> Box<dyn Query> {
            let term = Term::from_field_text(self.field(FIELD_INDEXED_TAG), tag);
            Box::new(BoostQuery::new(Box::new(TermQuery::new(term, IndexRecordOption::Basic)), boost))
        };
        let diet_queries = || -> Vec<Box<dyn Query>> {
            understood
                .diets
                .iter()
                .flat_map(|(diet, p)| {
                    categories::DIET_POSITIVE_VALUES
                        .iter()
                        .map(move |value| tag_query(&format!("diet:{diet}={value}"), *p))
                })
                .collect()
        };

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if !understood.cuisines.is_empty() {
            let cuisines = understood.cuisines.iter().map(|(c, p)| tag_query(&format!("cuisine={c}"), *p)).collect();
            clauses.push((Occur::Must, Box::new(BooleanQuery::union(cuisines))));
            clauses.push((Occur::Should, Box::new(BooleanQuery::union(diet_queries()))));
        } else if !understood.categories.is_empty() {
            let categories = understood.categories.iter().map(|(c, p)| tag_query(c, *p)).collect();
            clauses.push((Occur::Must, Box::new(BooleanQuery::union(categories))));
            clauses.push((Occur::Should, Box::new(BooleanQuery::union(diet_queries()))));
        } else if !understood.diets.is_empty() {
            clauses.push((Occur::Must, Box::new(BooleanQuery::union(diet_queries()))));
        } else {
            return None;
        }
        clauses.push((Occur::Should, Box::new(BooleanQuery::union(any_text))));
        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Searches, most relevant first.
    ///
    /// # Errors
    /// If the search fails or a stored document can't be decoded.
    pub async fn search(&self, request: &SearchRequest<'_>) -> Result<SearchResults> {
        let start = std::time::Instant::now();
        let bbox_rects = request.bbox.map(geometry::bbox_rects);
        let bbox_cells = bbox_rects.as_deref().map(geometry::query_cells);
        let limit = request.limit.max(1);
        // Coverings are larger than shapes, so some candidates in the box
        // are dropped below; duplicate admins are too, and reranking needs
        // the areas a result is named after among the candidates.
        let candidates = if bbox_rects.is_some() { (limit * 4).min(200) } else { (limit * 2).max(limit + RERANK_CANDIDATES) };

        let searcher = self.searcher()?;
        let words = self.query_words(request.text);
        let (hints, readings) = self.readings(&searcher, &words, request.text, Self::prefix(request));
        trace!("Readings of {:?}: {readings:?}, {hints:?}", request.text);
        let interpretations = self.interpretations(request);
        if let Some(interpretations) = &interpretations {
            trace!("Interpretations of {:?}: {interpretations:?}", request.text);
        }
        let query = self.construct_query(request, bbox_cells.as_deref(), &words, &hints, &readings, interpretations.as_deref());

        let index = self.clone();
        let focus = request.focus;
        let results = spawn_blocking(move || -> Result<SearchResults> {
            let focus = focus.map(|[lon, lat]| (Point::new(lon, lat), s2::point::Point::from(s2::latlng::LatLng::from_degrees(lat, lon))));
            let collector = TopDocs::with_limit(candidates).tweak_score(move |segment: &tantivy::SegmentReader| {
                let importance = segment.fast_fields().f64(FIELD_IMPORTANCE).ok();
                let center = focus.as_ref().and_then(|_| segment.fast_fields().u64(FIELD_CENTER).ok());
                let is_admin = focus.as_ref().and_then(|_| segment.fast_fields().u64(FIELD_IS_ADMIN).ok());
                move |doc, score: f32| {
                    let importance = importance.as_ref().and_then(|column| column.first(doc)).unwrap_or(0.0);
                    let near = match (&focus, center.as_ref().and_then(|column| column.first(doc))) {
                        (Some((_, focus)), Some(center)) => {
                            let admin = is_admin.as_ref().and_then(|column| column.first(doc)) == Some(1);
                            focus_boost(focus, center, admin)
                        }
                        _ => 1.0,
                    };
                    score * (1.0 + IMPORTANCE_WEIGHT * importance as f32) * near
                }
            });
            let mut hits = Vec::new();
            for (score, address) in searcher.search(&query, &collector)? {
                let hit = index.hit(&searcher, address, score)?;
                if let Some(rects) = &bbox_rects
                    && !geometry::intersects_any(&hit.geometry, rects)
                {
                    continue;
                }
                hits.push(hit);
            }
            if let Some(focus) = &focus {
                focus_inside_admins(&mut hits, focus);
            }
            if log::log_enabled!(log::Level::Trace) {
                for hit in hits.iter().take(8) {
                    trace!("  {:8.2} {:?} {:?}", hit.score, hit.tag("name").or(hit.wof.as_ref().map(|w| w.default_name.as_str())), hit.kind);
                }
            }
            rerank_admins(&mut hits);
            rank_areas_above_their_namesakes(&mut hits);
            hits.truncate(limit);
            let admin_ids: BTreeSet<u64> = hits.iter().flat_map(|hit| hit.admin_ids.iter().copied()).collect();
            let admins = index.admins(&searcher, &admin_ids)?;
            Ok(SearchResults { hits, admins })
        })
        .await?;
        let results = results.map_err(|e| {
            warn!("Search failed: {e:?}");
            e
        })?;
        trace!("Search took {:?} and yielded {} results", start.elapsed(), results.hits.len());
        Ok(results)
    }

    /// The place with this ID, with the administrative areas it's in, or no
    /// hits if there's none. Its score is 0.
    ///
    /// # Errors
    /// If the search fails or the stored document can't be decoded.
    pub async fn lookup(&self, id: PlaceId) -> Result<SearchResults> {
        let (field, term) = match &id {
            PlaceId::Poi(id) => (FIELD_POI_ID, Term::from_field_u64(self.field(FIELD_POI_ID), *id)),
            PlaceId::Wof(id) => (FIELD_WOF_ID, Term::from_field_u64(self.field(FIELD_WOF_ID), *id)),
            PlaceId::Osm(osm_type, id) => {
                (FIELD_OSM_ID, Term::from_field_text(self.field(FIELD_OSM_ID), &format!("{}/{id}", osm_type.as_str())))
            }
            PlaceId::Postcode(key) => (FIELD_POSTCODE_ID, Term::from_field_text(self.field(FIELD_POSTCODE_ID), key)),
        };
        trace!("Looking up {id} by {field}");
        let index = self.clone();
        spawn_blocking(move || -> Result<SearchResults> {
            let searcher = index.searcher()?;
            let query = TermQuery::new(term, IndexRecordOption::Basic);
            // An element is indexed once, but a POI ID could in principle be
            // on more than one; the first is as good as any.
            let Some((_, address)) = searcher.search(&query, &TopDocs::with_limit(1).order_by_score())?.into_iter().next() else {
                return Ok(SearchResults::default());
            };
            let hit = index.hit(&searcher, address, 0.0)?;
            let admins = index.admins(&searcher, &hit.admin_ids.iter().copied().collect())?;
            Ok(SearchResults { hits: vec![hit], admins })
        })
        .await?
    }

    pub(crate) fn hit(&self, searcher: &Searcher, address: DocAddress, score: f32) -> Result<SearchHit> {
        let doc: TantivyDocument = searcher.doc(address)?;
        let text = |field: &str| doc.get_first(self.field(field)).and_then(|v| v.as_str()).map(str::to_string);
        let kind = text(FIELD_KIND).as_deref().and_then(Kind::parse).context("document without a kind")?;
        let mut tags: Vec<(String, String)> = doc
            .get_first(self.field(FIELD_TAGS))
            .and_then(|v| v.as_object())
            .map(|object| object.map(|(k, v)| (k.to_string(), v.as_str().unwrap_or_default().to_string())).collect())
            .unwrap_or_default();
        let tag = |key: &str| tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let poi = tag("cardinal:poi_id")
            .and_then(|id| id.parse().ok())
            .zip(tag("cardinal:version").and_then(|v| v.parse().ok()));
        tags.retain(|(k, _)| !k.starts_with("cardinal:"));
        let osm = text(FIELD_OSM).map(|s| OsmRef::decode(&s)).transpose()?;
        let wof = text(FIELD_WOF).map(|s| serde_json::from_str::<WofPlace>(&s)).transpose()?;
        let admin_ids = doc.get_all(self.field(FIELD_ADMIN_IDS)).filter_map(|v| v.as_u64()).collect();
        let center = doc.get_first(self.field(FIELD_CENTER)).and_then(|v| v.as_u64()).context("document without a center")?;
        let center = s2::latlng::LatLng::from(s2::cellid::CellID(center));
        let geometry = doc
            .get_first(self.field(FIELD_GEOMETRY))
            .and_then(|v| v.as_bytes())
            .context("document without a geometry")?;
        Ok(SearchHit {
            score,
            kind,
            tags,
            poi,
            osm,
            wof,
            admin_ids,
            center: Point::new(center.lng.deg(), center.lat.deg()),
            geometry: geometry::from_wkb(geometry)?,
            importance: doc.get_first(self.field(FIELD_IMPORTANCE)).and_then(|v| v.as_f64()).unwrap_or(0.0),
        })
    }

    /// The Who's On First places with these IDs that are in the index.
    pub(crate) fn admins(&self, searcher: &Searcher, ids: &BTreeSet<u64>) -> Result<HashMap<u64, WofPlace>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let clauses: Vec<Box<dyn Query>> = ids
            .iter()
            .map(|id| -> Box<dyn Query> {
                Box::new(TermQuery::new(Term::from_field_u64(self.field(FIELD_WOF_ID), *id), IndexRecordOption::Basic))
            })
            .collect();
        let query = BooleanQuery::union(clauses);
        let mut admins = HashMap::new();
        for (_, address) in searcher.search(&query, &TopDocs::with_limit(ids.len()).order_by_score())? {
            let doc: TantivyDocument = searcher.doc(address)?;
            if let Some(wof) = doc.get_first(self.field(FIELD_WOF)).and_then(|v| v.as_str()) {
                let place: WofPlace = serde_json::from_str(wof)?;
                admins.insert(place.id, place);
            }
        }
        Ok(admins)
    }
}

/// How much being near the focus raises a result's score: see
/// `FOCUS_WEIGHT`. `center` is the result's center as an S2 cell ID.
fn focus_boost(focus: &s2::point::Point, center: u64, admin: bool) -> f32 {
    let center = s2::point::Point::from(s2::cellid::CellID(center));
    let km = focus.distance(&center).rad() * EARTH_RADIUS_KM;
    let local = 1.0 + FOCUS_WEIGHT * (FOCUS_SCALE_KM / (FOCUS_SCALE_KM + km)) as f32;
    if admin {
        local * (1.0 + REGION_WEIGHT * (REGION_SCALE_KM / (REGION_SCALE_KM + km)) as f32)
    } else {
        local
    }
}

/// Scores administrative areas containing the focus as if their centers
/// were at it: someone looking at a map of Vancouver, Washington means that
/// Vancouver, even far from its center. Only candidates are rescored, since
/// shapes aren't fast fields.
fn focus_inside_admins(hits: &mut [SearchHit], focus: &(Point<f64>, s2::point::Point)) {
    let (point, s2_focus) = focus;
    let mut changed = false;
    for hit in hits.iter_mut().filter(|hit| hit.kind == Kind::Admin) {
        if hit.geometry.intersects(point) {
            let center = s2::cellid::CellID::from(s2::latlng::LatLng::from_degrees(hit.center.y(), hit.center.x()));
            hit.score *= (1.0 + FOCUS_WEIGHT) * (1.0 + REGION_WEIGHT) / focus_boost(s2_focus, center.0, true);
            changed = true;
        }
    }
    if changed {
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
}

/// The least number of extra candidates reranked.
const RERANK_CANDIDATES: usize = 20;

/// What the index knows about words, for `interpret`.
struct IndexLookup<'a> {
    index: &'a AirmailIndex,
    searcher: &'a Searcher,
}

impl interpret::Lookup for IndexLookup<'_> {
    fn admin_importance(&self, name: &str) -> Option<f64> {
        self.index.admin_importance(self.searcher, name)
    }

    fn country_code(&self, code: &str) -> Option<String> {
        crate::countries::alpha2(code).map(str::to_string)
    }

    fn is_known(&self, word: &str) -> bool {
        std::iter::once(FIELD_NAME).chain(ADDRESS_FIELDS).chain(AdminBand::ALL.map(admin_field)).any(|field| {
            let term = Term::from_field_text(self.index.field(field), word);
            self.searcher.doc_freq(&term).is_ok_and(|n| n > 0)
        })
    }
}

/// Two admins this close with the same name and placetype are the same
/// place recorded twice.
const DUPLICATE_ADMIN_KM: f64 = 25.0;

/// Streets with the same name whose lines come this close, in degrees
/// (about 200 m), are pieces of the same street, indexed separately where
/// it's long.
const STREET_PIECE_DEGREES: f64 = 0.002;

/// Merges the pieces of each street into the best-ranked one, which gets
/// their shapes, so that a long street is one result shown whole.
fn merge_street_pieces(hits: &mut Vec<SearchHit>) {
    let streets: Vec<usize> = (0..hits.len()).filter(|&i| hits[i].kind == Kind::Street).collect();
    if streets.len() < 2 {
        return;
    }
    let names: Vec<Option<String>> = streets.iter().map(|&i| hits[i].tag("name").map(name_words)).collect();
    // Union-find over the streets, by name and nearness.
    let mut parent: Vec<usize> = (0..streets.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for a in 0..streets.len() {
        for b in a + 1..streets.len() {
            if names[a].is_some()
                && names[a] == names[b]
                && geo::Euclidean.distance(&hits[streets[a]].geometry, &hits[streets[b]].geometry) < STREET_PIECE_DEGREES
            {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                // The better-ranked piece, first in the list, is the root.
                parent[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut merged: HashMap<usize, Vec<geo::LineString<f64>>> = HashMap::new();
    let mut dropped = BTreeSet::new();
    for a in 0..streets.len() {
        let r = root(&mut parent, a);
        if r != a {
            dropped.insert(streets[a]);
            merged.entry(r).or_default().extend(lines(&hits[streets[a]].geometry));
        }
    }
    for (r, extra) in merged {
        let hit = &mut hits[streets[r]];
        let mut all = lines(&hit.geometry);
        all.extend(extra);
        hit.geometry = Geometry::MultiLineString(geo::MultiLineString(all));
    }
    let mut i = 0;
    hits.retain(|_| {
        i += 1;
        !dropped.contains(&(i - 1))
    });
}

/// A line shape's lines.
fn lines(geometry: &Geometry<f64>) -> Vec<geo::LineString<f64>> {
    match geometry {
        Geometry::LineString(line) => vec![line.clone()],
        Geometry::MultiLineString(multi) => multi.0.clone(),
        _ => Vec::new(),
    }
}

/// Drops admins and streets that duplicate a better-ranked one, and puts a place ahead
/// of a same-named area containing it, since `zurich` usually means the city
/// rather than the canton.
fn rerank_admins(hits: &mut Vec<SearchHit>) {
    merge_duplicates(hits);
    rerank_merged_admins(hits);
}

/// Merges a street's pieces and drops areas entered twice.
pub(crate) fn merge_duplicates(hits: &mut Vec<SearchHit>) {
    merge_street_pieces(hits);
    let key = |hit: &SearchHit| hit.wof.as_ref().map(|w| (normalize_text(&w.default_name).to_lowercase(), w.placetype.clone()));
    let mut kept: Vec<SearchHit> = Vec::with_capacity(hits.len());
    for hit in hits.drain(..) {
        let duplicate = key(&hit).is_some()
            && kept.iter().any(|k| {
                key(k) == key(&hit) && geo::Haversine.distance(k.center, hit.center) < DUPLICATE_ADMIN_KM * 1000.0
            });
        if !duplicate {
            kept.push(hit);
        }
    }
    *hits = kept;
}

fn rerank_merged_admins(hits: &mut Vec<SearchHit>) {
    // Its default and English names, so the city Luzern matches the canton
    // whose default name is Lucerne. Not every language's: some
    // neighbourhoods' preferred names in some languages are their city's.
    let names = |hit: &SearchHit| -> BTreeSet<String> {
        hit.wof
            .iter()
            .flat_map(|w| std::iter::once(&w.default_name).chain(w.names.get("eng")))
            .map(|n| normalize_text(n).to_lowercase())
            .collect()
    };
    // Each move puts a hit ahead of one of its ancestors. Bad data could
    // make two places each other's ancestors, so moves are limited.
    let mut moves = hits.len() * hits.len();
    let mut i = 0;
    while i < hits.len() {
        let moved = hits[i].wof.as_ref().and_then(|area| {
            let area_names = names(&hits[i]);
            // A town ahead of the larger area it's in, but not a part of a
            // town ahead of it: Shanghai the city, not a neighbourhood of
            // it called Shanghai.
            let part_of_town = |hit: &SearchHit| {
                hit.wof.as_ref().is_some_and(|w| NEIGHBOURHOOD_PLACETYPES.contains(&w.placetype.as_str()))
            };
            (i + 1..hits.len()).find(|&j| {
                hits[j].admin_ids.contains(&area.id)
                    && !part_of_town(&hits[j])
                    && !names(&hits[j]).is_disjoint(&area_names)
            })
        });
        match moved {
            Some(j) if moves > 0 => {
                moves -= 1;
                let inner = hits.remove(j);
                hits.insert(i, inner);
            }
            _ => i += 1,
        }
    }
}

/// Puts a place named after an area it's in (or just outside) below that
/// area, when both match: `spokane` means the city, not its station or
/// Spokane Falls, `lugano` the city, not Lago di Lugano, and `daly city` the
/// city, not its neighbourhood Original Daly City. A place whose name is
/// exactly the area's stays ahead if it's better known, unless it's a station
/// or the island the area is: Green Lake and Pike Place Market may well be
/// the place meant, not the neighbourhood named after them, but `capitol
/// hill` isn't its light rail station and `maui` is the county as much as the
/// island.
fn rank_areas_above_their_namesakes(hits: &mut Vec<SearchHit>) {
    let mut i = 0;
    while i < hits.len() {
        let place = &hits[i];
        let own = own_names(place);
        let admin = place.kind == Kind::Admin;
        // Only parts of a town are named after it: Vatican City isn't a
        // part of the Vatican.
        let part_of_town = place.wof.as_ref().is_some_and(|w| NEIGHBOURHOOD_PLACETYPES.contains(&w.placetype.as_str()));
        let same_name_goes_below = |area: &SearchHit| {
            !admin && (is_station(place) || is_island(place) || place.importance <= area.importance)
        };
        let below = (i + 1..hits.len()).rev().find(|&j| {
            let area = &hits[j];
            let Some(wof) = &area.wof else { return false };
            if area.score < NAMESAKE_MIN_SCORE_RATIO * place.score || (admin && !part_of_town) {
                return false;
            }
            let related = place.admin_ids.contains(&wof.id)
                || (!admin && geo::Haversine.distance(place.center, area.center) < NAMESAKE_KM * 1000.0);
            related
                && area_names(area).iter().any(|area_name| {
                    own.iter().any(|own| {
                        if own == area_name { same_name_goes_below(area) } else { contains_words(own, area_name) }
                    })
                })
        });
        match below {
            Some(j) => {
                // Just below the area, and scored so: each move is down the
                // list past a lower-ranked hit, so this ends.
                let mut place = hits.remove(i);
                place.score = place.score.min(hits[j - 1].score);
                hits.insert(j, place);
            }
            None => i += 1,
        }
    }
}

/// The words of a name, lowercased and transliterated, joined by spaces.
fn name_words(name: &str) -> String {
    name_key(name)
}

/// A hit's own names, as `name_words`: an area's default and English names,
/// or a place's name and English name.
fn own_names(hit: &SearchHit) -> BTreeSet<String> {
    let names: Vec<&str> = match &hit.wof {
        Some(wof) => std::iter::once(wof.default_name.as_str()).chain(wof.names.get("eng").map(String::as_str)).collect(),
        None => ["name", "name:en"].iter().filter_map(|key| hit.tag(key)).collect(),
    };
    names.into_iter().map(name_words).filter(|n| !n.is_empty()).collect()
}

/// A place only goes below an area it's named after that scores at least
/// this much of its score: one far down the list isn't what's meant either.
const NAMESAKE_MIN_SCORE_RATIO: f32 = 0.5;

/// Who's On First placetypes of parts of towns.
const NEIGHBOURHOOD_PLACETYPES: &[&str] = &["borough", "macrohood", "neighbourhood", "microhood"];

/// Words that say what kind of area an area is, which a place named after
/// it doesn't repeat: Maui is in Maui County.
const AREA_TYPE_WORDS: &[&str] = &[
    "county", "province", "region", "district", "municipality", "parish", "prefecture", "department", "state",
];

/// An area's names to compare a place's with: its own, and those without a
/// word saying what kind of area it is.
fn area_names(hit: &SearchHit) -> BTreeSet<String> {
    let mut names = own_names(hit);
    let bare: Vec<String> = names
        .iter()
        .filter_map(|name| {
            let words: Vec<&str> = name.split(' ').filter(|w| !AREA_TYPE_WORDS.contains(w)).collect();
            (!words.is_empty() && words.len() < name.split(' ').count()).then(|| words.join(" "))
        })
        .collect();
    names.extend(bare);
    names
}

/// Whether a hit is an island or archipelago, which an area may be all of.
fn is_island(hit: &SearchHit) -> bool {
    matches!(hit.tag("place"), Some("island" | "islet" | "archipelago"))
}

/// Whether `name` has the words of `part` in a row (and `part` isn't empty).
fn contains_words(name: &str, part: &str) -> bool {
    let part: Vec<&str> = part.split_whitespace().collect();
    let words: Vec<&str> = name.split_whitespace().collect();
    !part.is_empty() && words.windows(part.len()).any(|w| w == part.as_slice())
}

/// How far from an area's center a station may be and still be named after
/// it: stations are often just outside town.
const NAMESAKE_KM: f64 = 10.0;

/// Whether a hit is a transit station or stop.
fn is_station(hit: &SearchHit) -> bool {
    crate::importance::is_transit_stop(&hit.tags)
        || hit.tags.iter().any(|(k, v)| {
            matches!(
                (k.as_str(), v.as_str()),
                ("railway", "station" | "halt" | "stop")
                    | ("public_transport", "station" | "stop_area")
                    | ("building", "train_station" | "transportation")
                    | ("amenity", "bus_station" | "ferry_terminal")
            )
        })
}

/// Registers the tokenizer text fields use: it's a name in the schema, and
/// has to be supplied whenever an index is opened.
fn register_tokenizers(index: &tantivy::Index) {
    index.tokenizers().register(crate::text::TOKENIZER, tantivy::tokenizer::TextAnalyzer::from(crate::text::Tokenizer));
}

fn check_version(index_dir: &Path) -> Result<()> {
    let path = index_dir.join(VERSION_FILE);
    let file: VersionFile = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("reading {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("{} has no {VERSION_FILE}; it was built by an older airmail", index_dir.display())
        }
        Err(e) => return Err(e.into()),
    };
    if file.schema_version != SCHEMA_VERSION {
        bail!(
            "{} has schema version {}, but this airmail reads version {SCHEMA_VERSION}",
            index_dir.display(),
            file.schema_version
        );
    }
    Ok(())
}

/// The schema version of the index in `index_dir`.
///
/// # Errors
/// If it has no readable version file.
pub fn schema_version(index_dir: &Path) -> Result<u32> {
    let bytes = std::fs::read(index_dir.join(VERSION_FILE))?;
    Ok(serde_json::from_slice::<VersionFile>(&bytes)?.schema_version)
}

/// How many edits to tolerate in a word with `typos`.
/// Typos tolerated in a word still being typed: fewer, since a prefix with
/// typos matches far too much (`starb` would be a neighbourhood whose name in
/// Polish begins `stare`).
fn typing_typo_distance(word: &str) -> u8 {
    u8::from(word.chars().count() >= 6)
}

fn typo_distance(word: &str) -> u8 {
    match word.chars().count() {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    }
}

/// Text as it's indexed and searched: folded (see `text`), with abbreviated
/// German street suffixes written out. The tokenizer splits it into words.
#[must_use]
pub fn normalize_text(text: &str) -> String {
    crate::spelling::expand_abbreviations(&crate::text::fold(text))
}

/// A name's words joined by spaces, as `name_exact` and `area_name` hold
/// them and queries build them.
#[must_use]
pub fn name_key(name: &str) -> String {
    crate::text::words(&normalize_text(name)).into_iter().map(|(_, w)| w).collect::<Vec<_>>().join(" ")
}

/// The forms a name is indexed in: those that are exactly it (normalized,
/// with umlauts written out, with apostrophes as spaces, and the spelling
/// variants of each), and its transliteration and its variants, which find
/// it without being its name.
#[must_use]
pub fn name_forms(name: &str) -> (Vec<String>, Vec<String>) {
    let with_variants = |forms: Vec<String>| -> Vec<String> {
        let mut all: Vec<String> = Vec::new();
        for form in forms {
            all.extend(crate::spelling::variants(&form));
            all.push(form);
        }
        all.sort();
        all.dedup();
        all
    };
    let mut exact = vec![normalize_text(name)];
    exact.extend(crate::text::umlaut_variant(name).map(|v| normalize_text(&v)));
    exact.extend(crate::text::apostrophe_variant(name).map(|v| normalize_text(&v)));
    let exact = with_variants(exact);
    let other: Vec<String> = with_variants(crate::text::transliteration(name).map(|v| normalize_text(&v)).into_iter().collect())
        .into_iter()
        .filter(|form| !exact.contains(form))
        .collect();
    (exact, other)
}

/// Matches what `matcher` matches, adding a BM25 score for an exact match of
/// `term`. Fuzzy and prefix queries score every match the same, so without
/// this a rare word like a POI's name ranks no higher than a common one, and
/// below POIs matching only the query's categories.
fn with_exact_score(matcher: Box<dyn Query>, term: Term) -> Box<dyn Query> {
    Box::new(BooleanQuery::new(vec![
        (Occur::Must, matcher),
        (Occur::Should, Box::new(TermQuery::new(term, IndexRecordOption::Basic))),
    ]))
}

/// The `key=value` terms a POI is found under by tag: its category tags,
/// cuisines with every containing cuisine, and diets, with vegan implying
/// vegetarian. Values are split on `;` and lowercased.
fn indexed_tags(tags: &[(String, String)]) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    for (key, raw) in tags {
        for value in categories::split_values(raw) {
            if key == "cuisine" {
                terms.extend(cuisine::with_ancestors(&value).into_iter().map(|c| format!("cuisine={c}")));
            } else if key.starts_with("diet:") || categories::is_category_tag(key, &value) {
                terms.insert(format!("{key}={value}"));
            }
        }
    }
    let serves = |diet: &str| {
        categories::DIET_POSITIVE_VALUES
            .iter()
            .any(|value| terms.contains(&format!("diet:{diet}={value}")))
    };
    if serves("vegan") && !serves("vegetarian") {
        terms.insert("diet:vegetarian=yes".to_string());
    }
    terms
}

pub struct AirmailIndexWriter {
    tantivy_writer: tantivy::IndexWriter,
    schema: Schema,
}

impl AirmailIndexWriter {
    fn field(&self, name: &str) -> tantivy::schema::Field {
        self.schema.get_field(name).expect("schema field")
    }

    /// The words of a name, joined by spaces, as `name_exact` holds it.
    fn exact_key(&self, name: &str) -> String {
        name_key(name)
    }

    pub fn add_poi(&mut self, schemafied: SchemafiedPoi, source: &str) -> Result<()> {
        let poi = &schemafied.poi;
        let mut doc = TantivyDocument::default();
        let mut exact = BTreeSet::new();
        // Every form a name is in, for the gazetteer.
        let mut all_forms = BTreeSet::new();
        for name in &schemafied.names {
            let (exact_forms, other_forms) = name_forms(name);
            for form in exact_forms {
                exact.insert(self.exact_key(&form));
                all_forms.insert(form.clone());
                doc.add_text(self.field(FIELD_NAME), form);
            }
            for form in other_forms {
                all_forms.insert(form.clone());
                doc.add_text(self.field(FIELD_NAME), form);
            }
        }
        for name in &poi.variant_names {
            let (exact_forms, other_forms) = name_forms(name);
            for form in exact_forms.into_iter().chain(other_forms) {
                doc.add_text(self.field(FIELD_NAME), form);
            }
        }
        let mut street_forms = BTreeSet::new();
        for road in &schemafied.roads {
            let (exact_forms, other_forms) = name_forms(road);
            // A street is named by its road's forms too: `pike st` is
            // exactly Pike Street.
            if poi.kind == Kind::Street {
                for form in &exact_forms {
                    exact.insert(self.exact_key(form));
                }
            }
            street_forms.extend(exact_forms.into_iter().chain(other_forms));
        }
        for form in street_forms {
            doc.add_text(self.field(FIELD_STREET), form);
        }
        for key in &exact {
            if !key.is_empty() {
                doc.add_text(self.field(FIELD_NAME_EXACT), key);
            }
        }
        let values = |value: &Option<String>| -> Vec<String> {
            value.iter().flat_map(|v| v.split(';')).map(str::trim).filter(|v| !v.is_empty()).map(normalize_text).collect()
        };
        for number in values(&poi.house_number) {
            doc.add_text(self.field(FIELD_HOUSENUMBER), number);
        }
        for unit in values(&poi.unit) {
            doc.add_text(self.field(FIELD_UNIT), unit);
        }
        for postcode in values(&poi.postcode) {
            let compact: String = postcode.chars().filter(|c| !c.is_whitespace()).collect();
            if compact != postcode {
                doc.add_text(self.field(FIELD_POSTCODE), compact);
            }
            doc.add_text(self.field(FIELD_POSTCODE), postcode);
        }
        if let Some(key) = poi.tag("cardinal:postcode_id") {
            doc.add_text(self.field(FIELD_POSTCODE_ID), key);
        }
        let mut admin_forms = BTreeSet::new();
        for (band, name) in &poi.admin_names {
            let (exact_forms, other_forms) = name_forms(name);
            admin_forms.extend(exact_forms.into_iter().chain(other_forms).map(|form| (*band, form)));
        }
        for (band, form) in admin_forms {
            doc.add_text(self.field(admin_field(band)), form);
        }
        let country = poi.country_code.as_deref().or_else(|| poi.wof.as_ref().and_then(|w| w.country.as_deref()));
        if let Some(country) = country {
            doc.add_text(self.field(FIELD_COUNTRY_CODE), country.to_uppercase());
            for code in crate::countries::codes(country) {
                doc.add_text(self.field(FIELD_ADMIN_COUNTRY), code);
            }
        }
        // The gazetteer: an area's own names by its band.
        if let Some(band) = poi.wof.as_ref().and_then(|w| AdminBand::of(&w.placetype)) {
            doc.add_text(self.field(FIELD_PLACETYPE), band.as_str());
            let mut keys: BTreeSet<String> = all_forms.iter().map(|form| self.exact_key(form)).collect();
            if band == AdminBand::Country
                && let Some(country) = country
            {
                keys.extend(crate::countries::codes(country));
            }
            for key in keys.into_iter().filter(|k| !k.is_empty()) {
                doc.add_text(self.field(FIELD_AREA_NAME), area_key(band, &key));
            }
        }
        for term in indexed_tags(&poi.tags) {
            doc.add_text(self.field(FIELD_INDEXED_TAG), &term);
        }
        doc.add_text(self.field(FIELD_KIND), poi.kind.as_str());
        doc.add_u64(self.field(FIELD_IS_ADMIN), u64::from(poi.kind == Kind::Admin));
        doc.add_text(self.field(FIELD_SOURCE), source);
        for id in &poi.admin_ids {
            doc.add_u64(self.field(FIELD_ADMIN_IDS), *id);
        }
        if let Some(osm) = &poi.osm {
            doc.add_text(self.field(FIELD_OSM), osm.encode());
            doc.add_text(self.field(FIELD_OSM_ID), format!("{}/{}", osm.osm_type.as_str(), osm.id));
        }
        if let Some(poi_id) = poi.tag("cardinal:poi_id").and_then(|id| id.parse::<u64>().ok()) {
            doc.add_u64(self.field(FIELD_POI_ID), poi_id);
        }
        if let Some(wof) = &poi.wof {
            doc.add_u64(self.field(FIELD_WOF_ID), wof.id);
            doc.add_text(self.field(FIELD_WOF), serde_json::to_string(wof)?);
        }
        if !poi.tags.is_empty() {
            doc.add_object(
                self.field(FIELD_TAGS),
                poi.tags
                    .iter()
                    .map(|(k, v)| (k.clone(), OwnedValue::Str(v.clone())))
                    .collect::<BTreeMap<String, OwnedValue>>(),
            );
        }
        let center = s2::cellid::CellID::from(s2::latlng::LatLng::from_degrees(poi.center.y(), poi.center.x()));
        doc.add_u64(self.field(FIELD_CENTER), center.0);
        doc.add_bytes(self.field(FIELD_GEOMETRY), &geometry::to_wkb(&poi.geometry)?);
        let mut ancestors = BTreeSet::new();
        for cell in geometry::covering(&poi.geometry) {
            doc.add_u64(self.field(FIELD_S2_COVER), cell.0);
            ancestors.extend(geometry::ancestors(cell).into_iter().map(|c| c.0));
        }
        for cell in ancestors {
            doc.add_u64(self.field(FIELD_S2_COVER_ANCESTORS), cell);
        }
        doc.add_f64(self.field(FIELD_IMPORTANCE), poi.importance);
        self.tantivy_writer.add_document(doc)?;
        Ok(())
    }

    /// Deletes every document from `source`, so loading it again replaces
    /// it rather than adding duplicates.
    pub fn delete_source(&mut self, source: &str) {
        self.tantivy_writer.delete_term(Term::from_field_text(self.field(FIELD_SOURCE), source));
    }

    pub fn commit(mut self) -> Result<()> {
        self.tantivy_writer.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn apostrophes_join_words() {
        assert_eq!(normalize_text("McDonald's"), "mcdonalds");
        assert_eq!(normalize_text("McDonald\u{2019}s"), "mcdonalds");
        // Bulgarian for Starbuck, which mustn't match the word "starb".
        assert_eq!(normalize_text("Старбък"), "старбък");
        assert_eq!(name_forms("Старбък").1, vec!["starbk"]);
    }

    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn test_indexed_tags() {
        let terms = indexed_tags(&tags(&[
            ("amenity", "restaurant"),
            ("building", "yes"),
            ("cuisine", "Vietnamese; pizza"),
            ("diet:vegan", "only"),
            ("name", "Pho Real"),
        ]));
        assert_eq!(
            terms.into_iter().collect::<Vec<_>>(),
            vec![
                "amenity=restaurant",
                "cuisine=asian",
                "cuisine=pizza",
                "cuisine=vietnamese",
                "diet:vegan=only",
                "diet:vegetarian=yes",
            ]
        );
    }

    fn admin_hit(id: u64, placetype: &str, name: &str, at: (f64, f64), ancestors: &[u64]) -> SearchHit {
        SearchHit {
            score: 1.0,
            kind: Kind::Admin,
            tags: Vec::new(),
            poi: None,
            osm: None,
            wof: Some(WofPlace {
                id,
                placetype: placetype.into(),
                population: None,
                default_name: name.into(),
                names: BTreeMap::new(),
                abbreviation: None,
                country: None,
            }),
            admin_ids: ancestors.to_vec(),
            center: Point::new(at.0, at.1),
            geometry: Geometry::Point(Point::new(at.0, at.1)),
            importance: 0.5,
        }
    }

    fn ids(hits: &[SearchHit]) -> Vec<u64> {
        hits.iter().map(|h| h.wof.as_ref().unwrap().id).collect()
    }

    #[test]
    fn a_city_goes_ahead_of_its_same_named_canton() {
        let mut hits = vec![
            admin_hit(1, "region", "Zürich", (8.65, 47.42), &[9]),
            admin_hit(2, "locality", "Winterthur", (8.72, 47.50), &[1, 9]),
            admin_hit(3, "locality", "Zürich", (8.54, 47.37), &[1, 9]),
        ];
        rerank_admins(&mut hits);
        assert_eq!(ids(&hits), vec![3, 1, 2]);
    }

    #[test]
    fn a_place_named_after_its_area_goes_below_it() {
        let named = |id: u64, name: &str, inside: &[u64], score: f32| {
            let mut hit = admin_hit(id, "", name, (-117.42, 47.66), inside);
            hit.kind = Kind::Poi;
            hit.wof = None;
            hit.tags = vec![("name".into(), name.into()), ("railway".into(), "station".into())];
            hit.score = score;
            hit
        };
        let mut city = admin_hit(10, "locality", "Spokane", (-117.42, 47.66), &[]);
        city.score = 2.0;
        let mut hits = vec![
            named(1, "Spokane", &[10], 3.0),
            named(2, "Spokane Falls", &[10], 2.5),
            city,
            named(3, "Spokane", &[99], 1.0),
        ];
        rank_areas_above_their_namesakes(&mut hits);
        let order: Vec<&str> = hits.iter().map(|h| h.tag("name").unwrap_or("city")).collect();
        assert_eq!(order, ["city", "Spokane Falls", "Spokane", "Spokane"]);
        assert!(hits[2].score <= hits[1].score);

        // A place named after its area, with more words, goes below it.
        let mut lake = named(5, "Lago di Lugano", &[], 1.5);
        lake.tags.retain(|(k, _)| k == "name");
        let mut hits = vec![lake, admin_hit(30, "locality", "Lugano", (-117.42, 47.66), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(hits[0].kind, Kind::Admin);
        assert!(!contains_words("starbucks center", "star"));

        // A landmark the area is named after stays, if it's better known.
        let mut market = named(4, "Pike Place Market", &[20], 1.5);
        market.tags.retain(|(k, _)| k == "name");
        market.importance = 0.6;
        let mut hits = vec![market.clone(), admin_hit(20, "neighbourhood", "Pike Place Market", (-122.34, 47.61), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(hits[0].kind, Kind::Poi);
        market.importance = 0.1;
        let mut hits = vec![market, admin_hit(20, "neighbourhood", "Pike Place Market", (-122.34, 47.61), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(hits[0].kind, Kind::Admin);

        // An island goes below the county that's named after it, and is it.
        let mut island = named(6, "Sardigna/Sardegna", &[40], 1.5);
        island.tags = vec![("name".into(), "Sardigna/Sardegna".into()), ("name:en".into(), "Sardinia".into()), ("place".into(), "island".into())];
        island.importance = 0.9;
        let mut hits = vec![island, admin_hit(40, "macroregion", "Sardinia", (9.0, 40.0), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(hits[0].kind, Kind::Admin);
        let mut maui = named(7, "Maui", &[50], 1.5);
        maui.tags = vec![("name".into(), "Maui".into()), ("place".into(), "island".into())];
        let mut hits = vec![maui, admin_hit(50, "county", "Maui County", (-156.3, 20.8), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(hits[0].kind, Kind::Admin);

        // An area named after one it's in goes below it.
        let original = admin_hit(61, "neighbourhood", "Original Daly City", (-122.46, 37.70), &[60]);
        let mut hits = vec![original, admin_hit(60, "locality", "Daly City", (-122.46, 37.69), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(ids(&hits), vec![60, 61]);
        // But not one it isn't in.
        let greater = admin_hit(70, "region", "Greater London", (-0.1, 51.5), &[]);
        let mut hits = vec![greater, admin_hit(71, "locality", "London", (-0.1, 51.5), &[70])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(ids(&hits), vec![70, 71]);

        // Not below an area far down the list.
        let mut far = named(9, "Spokane", &[10], 3.0);
        far.tags.push(("railway".into(), "station".into()));
        let mut hits = vec![far, admin_hit(10, "locality", "Spokane", (-117.42, 47.66), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(hits[0].kind, Kind::Poi);
        // An area isn't a part of one it's named after.
        let vatican_city = admin_hit(91, "locality", "Vatican City", (12.45, 41.9), &[90]);
        let mut hits = vec![vatican_city, admin_hit(90, "country", "Vatican", (12.45, 41.9), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(ids(&hits), vec![91, 90]);

        // Names are compared by their words.
        let river = named(8, "St. Marys River at Sault Ste. Marie", &[80], 1.5);
        let mut hits = vec![river, admin_hit(80, "locality", "Sault Ste Marie", (-84.3, 46.5), &[])];
        rank_areas_above_their_namesakes(&mut hits);
        assert_eq!(hits[0].kind, Kind::Admin);
    }

    #[test]
    fn pieces_of_a_street_are_dropped() {
        let street = |id: u64, name: &str, lon: f64| {
            let mut hit = admin_hit(id, "", name, (lon, 47.6), &[]);
            hit.kind = Kind::Street;
            hit.wof = None;
            hit.tags = vec![("name".into(), name.into())];
            hit
        };
        // Pieces touching in a row, ranked out of order, and a namesake
        // across the state.
        let piece = |id: u64, from: f64, to: f64| {
            let mut hit = street(id, "Broadway", from);
            hit.geometry = Geometry::LineString(geo::LineString::from(vec![(from, 47.6), (to, 47.6)]));
            hit
        };
        let mut hits = vec![piece(1, -122.32, -122.30), piece(3, -122.28, -122.26), piece(2, -122.30, -122.28), piece(4, -117.4, -117.3)];
        rerank_admins(&mut hits);
        assert_eq!(hits.iter().map(|h| h.center.x()).collect::<Vec<_>>(), vec![-122.32, -117.4]);
        assert!(matches!(&hits[0].geometry, Geometry::MultiLineString(m) if m.0.len() == 3));
    }

    #[test]
    fn duplicate_admins_are_dropped() {
        let mut hits = vec![
            admin_hit(1, "county", "King County", (-121.83, 47.49), &[]),
            admin_hit(2, "county", "King County", (-121.80, 47.50), &[]),
            admin_hit(3, "county", "King County", (-99.0, 33.6), &[]),
        ];
        rerank_admins(&mut hits);
        assert_eq!(ids(&hits), vec![1, 3]);
    }

    #[test]
    fn mutual_ancestors_dont_loop() {
        let mut hits = vec![admin_hit(1, "locality", "A", (0.0, 0.0), &[2]), admin_hit(2, "localadmin", "A", (0.0, 0.0), &[1])];
        rerank_admins(&mut hits);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn test_indexed_tags_keeps_explicit_vegetarian() {
        let terms = indexed_tags(&tags(&[("diet:vegan", "yes"), ("diet:vegetarian", "only")]));
        assert_eq!(terms.into_iter().collect::<Vec<_>>(), vec!["diet:vegan=yes", "diet:vegetarian=only"]);
    }

    #[test]
    fn focus_boost_fades_with_distance() {
        let at = |lon: f64, lat: f64| s2::cellid::CellID::from(s2::latlng::LatLng::from_degrees(lat, lon)).0;
        let seattle = s2::point::Point::from(s2::latlng::LatLng::from_degrees(47.6062, -122.3321));
        let here = focus_boost(&seattle, at(-122.3321, 47.6062), false);
        let bellevue = focus_boost(&seattle, at(-122.2015, 47.6101), false);
        let zurich = focus_boost(&seattle, at(8.54, 47.37), false);
        assert!((here - (1.0 + FOCUS_WEIGHT)).abs() < 1e-3);
        assert!(here > bellevue && bellevue > zurich);
        assert!((1.0..1.001).contains(&zurich));

        // Areas count nearness over hundreds of kilometres too: Leavenworth,
        // Washington, from Seattle, against Leavenworth, Kansas.
        let washington = focus_boost(&seattle, at(-120.6615, 47.5962), true);
        let kansas = focus_boost(&seattle, at(-94.9225, 39.3111), true);
        assert!(washington > 1.4 && kansas < 1.1, "{washington} {kansas}");
        assert!(focus_boost(&seattle, at(-120.6615, 47.5962), false) < 1.1, "not POIs");
    }

    #[test]
    fn focus_inside_an_admin_counts_as_at_its_center() {
        let focus = (-122.62, 45.65);
        let s2_focus = s2::point::Point::from(s2::latlng::LatLng::from_degrees(focus.1, focus.0));
        let station = |score: f32| {
            let mut hit = admin_hit(1, "locality", "Station", (-122.62, 45.65), &[]);
            hit.kind = Kind::Poi;
            hit.score = score;
            hit
        };
        let mut city = admin_hit(2, "locality", "Vancouver", (-122.55, 45.64), &[]);
        city.geometry = Geometry::Polygon(
            geo::Rect::new(geo::coord! { x: -122.77, y: 45.57 }, geo::coord! { x: -122.46, y: 45.70 }).to_polygon(),
        );
        city.score = 0.9;
        let mut hits = vec![station(1.0), city];
        focus_inside_admins(&mut hits, &(Point::new(focus.0, focus.1), s2_focus));
        assert_eq!(hits[0].kind, Kind::Admin);
        assert!(hits[0].score > 1.2);
    }
}
