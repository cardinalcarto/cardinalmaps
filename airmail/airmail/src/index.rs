use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use geo::{Distance, Geometry, Point};
use log::trace;
use serde::{Deserialize, Serialize};
use tantivy::schema::{FAST, STORED, STRING, Value};
use tantivy::{
    DocAddress, Searcher, TantivyDocument, Term,
    collector::TopDocs,
    directory::{
        Directory, DirectoryLock, FileHandle, Lock, MmapDirectory, WatchCallback, WatchHandle,
        WritePtr,
        error::{DeleteError, LockError, OpenReadError, OpenWriteError},
    },
    query::{BooleanQuery, ConstScoreQuery, FuzzyTermQuery, Occur, Query, TermQuery},
    schema::{
        IndexRecordOption, NumericOptions, OwnedValue, Schema, TextFieldIndexing, TextOptions,
    },
};
#[cfg(feature = "tokio")]
use tokio::task::spawn_blocking;

use crate::categories::{self, CategoryModel, QueryCategories};
use crate::cuisine;
#[cfg(feature = "tokio")]
use crate::error::AirmailError;
use crate::geometry;
use crate::poi::{AdminBand, Kind, OsmRef, PlaceId, Pointer, SchemafiedPoi, TileId, ToIndexPoi, WofPlace};

/// The version of the index's schema and of what its fields hold. Readers
/// refuse indexes of other versions.
pub const SCHEMA_VERSION: u32 = 8;

/// The indexing memory of `AirmailIndex::writer`, in bytes.
pub const DEFAULT_WRITER_HEAP: usize = 2_000_000_000;

/// How much a writer may use.
#[derive(Debug, Clone, Copy)]
pub struct WriterOptions {
    /// Indexing memory in bytes, shared by the threads: segments are written
    /// out as it fills.
    pub heap_bytes: usize,
    /// Indexing threads, or `None` for tantivy's choice (one per core, up to
    /// eight).
    pub threads: Option<usize>,
    /// What documents store.
    pub storage: Storage,
}

impl Default for WriterOptions {
    fn default() -> Self {
        Self { heap_bytes: DEFAULT_WRITER_HEAP, threads: None, storage: Storage::Full }
    }
}

/// What documents store for results. Either way, they're indexed alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Storage {
    /// Everything results show: tags, shape, the OSM element, the Who's On
    /// First place. What the server's index stores, for clients that have
    /// nothing else.
    #[default]
    Full,
    /// For a document with a stable POI ID, only its kind, the ID and the
    /// tile it was read from (see `poi::Pointer`), its name tags (which
    /// ranking reads), its center, importance, source and areas' IDs; its
    /// other tags and its shape are in the tile. Others store everything.
    /// For a phone's index of the tiles it has.
    Pointers,
}

/// Fills in what a search can't rank pointer documents well without: their
/// tags, for their names (how much of a name a query is) and diets.
pub trait Hydrate {
    /// The tags of the feature a pointer points to, or `None` if it can't be
    /// read; ranking then goes without them.
    fn tags(&self, pointer: &Pointer) -> Option<Vec<(String, String)>>;
}

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
/// The OSM element as `type/id`, for lookups; or `local/<id>` for a document
/// with a [`LOCAL_ID_TAG`].
pub const FIELD_OSM_ID: &str = "osm_id";
/// The tag that gives a document with no other ID (no OSM element, POI ID,
/// Who's On First ID or postcode key) an ID within its own index, a decimal
/// number, for `AirmailIndex::lookup_local_blocking`. Only the device's index
/// sets it; the server's never does.
pub const LOCAL_ID_TAG: &str = "cardinal:local_id";

/// A local ID's term in `FIELD_OSM_ID`, apart from OSM's `node/`, `way/` and
/// `relation/`.
fn local_key(id: u64) -> String {
    format!("local/{id}")
}
/// A postcode's key, `<country>/<postcode>` (see `poi::postcode_key`).
pub const FIELD_POSTCODE_ID: &str = "postcode_id";
pub const FIELD_S2_COVER: &str = "s2_cover";
pub const FIELD_S2_COVER_ANCESTORS: &str = "s2_cover_ancestors";
// Stored for results.
pub const FIELD_OSM: &str = "osm";
pub const FIELD_WOF: &str = "wof";
pub const FIELD_TAGS: &str = "tags";
pub const FIELD_CENTER: &str = "center";
/// 1 if a document's shape is its center (a point, or a pointer document,
/// whose shape isn't stored), so that whether it's in a box can be told from
/// its `center` fast field without reading it; else 0.
pub const FIELD_IS_POINT: &str = "is_point";
pub const FIELD_GEOMETRY: &str = "geometry";
pub const FIELD_IMPORTANCE: &str = "importance";
/// A pointer document's stable POI ID and tile (`TileId::pack`); see
/// `Storage::Pointers`.
pub const FIELD_POINTER_POI_ID: &str = "pointer_poi_id";
pub const FIELD_POINTER_TILE: &str = "pointer_tile";

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

/// A gazetteer key: `<band>/<words>`.
#[must_use]
pub fn area_key(band: AdminBand, words: &str) -> String {
    format!("{}/{words}", band.as_str())
}

// How much a query word counts in each field: a name match counts most, so a
// place named after a city outranks the places in it.
const TYPED_PREFIX_SCORE: f32 = 1.0;
pub(crate) const EARTH_RADIUS_KM: f64 = 6371.0088;

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
#[derive(Clone, Default)]
pub struct SearchRequest<'a> {
    pub text: &'a str,
    /// Match the last word as a prefix, unless a space follows it.
    pub prefix: bool,
    /// Allow typos if nothing is found without.
    pub typos: bool,
    /// Category tags results must all have, as `key=value`.
    pub tags: Vec<String>,
    /// `[min_lon, min_lat, max_lon, max_lat]` results' shapes must intersect.
    pub bbox: Option<[f64; 4]>,
    /// A Who's On First ID results must be inside.
    pub within: Option<u64>,
    /// Kinds to return; all if empty.
    pub kinds: Vec<Kind>,
    pub limit: usize,
    /// `[west, south, east, north]`: what the map shows. Results near it rank higher, the
    /// more so the closer it is.
    pub viewport: Option<[f64; 4]>,
    /// `[lon, lat]` to rank results near higher, without a viewport.
    pub focus: Option<[f64; 2]>,
    /// `[lon, lat]`: where the user is, for queries like `coffee near me`.
    pub location: Option<[f64; 2]>,
    /// Reads what kind of place words suggest.
    pub model: Option<&'a CategoryModel>,
    /// Reads pointer documents' tags, for ranking them as if they were
    /// stored (see `Storage::Pointers`).
    pub hydrate: Option<&'a dyn Hydrate>,
}

/// A search result.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub score: f32,
    pub kind: Kind,
    /// OSM tags, without Cardinal's `cardinal:*` tags, which are in `poi`.
    /// A pointer document's names, or all its tags if the search was given
    /// `Hydrate`.
    pub tags: Vec<(String, String)>,
    /// Cardinal's stable identifier and the OSM version it was indexed at,
    /// from its tags.
    pub poi: Option<(u64, u32)>,
    /// For a pointer document, where the rest of it is: its stable POI ID
    /// and its tile. It has no OSM element or Who's On First place, and its
    /// shape is its center.
    pub pointer: Option<Pointer>,
    pub osm: Option<OsmRef>,
    pub wof: Option<WofPlace>,
    /// Its ID within its index, from its [`LOCAL_ID_TAG`] (only on the device).
    pub local_id: Option<u64>,
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

    /// Sets its tags: Cardinal's `cardinal:*` tags go to `poi` (if there's
    /// a version) and `local_id`, and the rest to `tags`.
    pub(crate) fn set_tags(&mut self, mut tags: Vec<(String, String)>) {
        let tag = |key: &str| tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let poi = tag("cardinal:poi_id").and_then(|id| id.parse().ok()).zip(tag("cardinal:version").and_then(|v| v.parse().ok()));
        let local_id = tag(LOCAL_ID_TAG).and_then(|id| id.parse().ok());
        tags.retain(|(k, _)| !k.starts_with("cardinal:"));
        self.poi = poi;
        self.local_id = local_id;
        self.tags = tags;
    }
}

/// Results, with the administrative areas they're in.
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub admins: HashMap<u64, WofPlace>,
    /// What the likeliest reading's words suggest.
    pub categories: Option<QueryCategories>,
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
        schema_builder.add_u64_field(FIELD_IS_POINT, FAST);
        schema_builder.add_bytes_field(FIELD_GEOMETRY, STORED);
        schema_builder.add_f64_field(FIELD_IMPORTANCE, FAST | STORED);
        schema_builder.add_u64_field(FIELD_POINTER_POI_ID, STORED);
        schema_builder.add_u64_field(FIELD_POINTER_TILE, STORED);
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

    /// A writer with `DEFAULT_WRITER_HEAP` bytes of indexing memory and
    /// tantivy's choice of threads, as the indexer uses.
    pub fn writer(&mut self) -> Result<AirmailIndexWriter> {
        self.writer_with(WriterOptions::default())
    }

    /// A writer with `options`' memory, threads and storage, for building
    /// small indexes in little memory, as on a phone.
    ///
    /// # Errors
    /// If another writer has the index, or the memory is too little for the
    /// threads (tantivy needs at least 15 MB for each).
    pub fn writer_with(&mut self, options: WriterOptions) -> Result<AirmailIndexWriter> {
        let tantivy_writer = match options.threads {
            Some(threads) => self.tantivy_index.writer_with_num_threads::<TantivyDocument>(threads, options.heap_bytes)?,
            None => self.tantivy_index.writer::<TantivyDocument>(options.heap_bytes)?,
        };
        Ok(AirmailIndexWriter { tantivy_writer, schema: self.tantivy_index.schema(), storage: options.storage })
    }

    /// Merges every segment into one.
    pub async fn merge(&mut self) -> Result<()> {
        let ids = self.tantivy_index.searchable_segment_ids()?;
        self.tantivy_index.writer::<TantivyDocument>(DEFAULT_WRITER_HEAP)?.merge(&ids).await?;
        Ok(())
    }

    /// Merges every segment into one, on this thread, with a writer of
    /// `options`.
    ///
    /// # Errors
    /// If another writer has the index or merging fails.
    pub fn merge_blocking(&mut self, options: WriterOptions) -> Result<()> {
        let ids = self.tantivy_index.searchable_segment_ids()?;
        if ids.len() < 2 {
            return Ok(());
        }
        let mut writer = self.writer_with(options)?.tantivy_writer;
        writer.merge(&ids).wait()?;
        writer.wait_merging_threads()?;
        Ok(())
    }

    /// How many documents the index has.
    ///
    /// # Errors
    /// If the index can't be read.
    pub fn num_docs_blocking(&self) -> Result<u64> {
        Ok(self.searcher()?.num_docs())
    }

    #[cfg(feature = "tokio")]
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

    /// The query's words, and the tagger's likeliest readings of them if
    /// there's a tagger, until together they're at least `coverage` likely or
    /// there are `max` of them.
    pub(crate) fn tagged(&self, text: &str, prefix: bool, focus: Option<[f64; 2]>, max: usize, coverage: f64) -> (Vec<String>, Option<Vec<airmail_tagger::Interpretation>>) {
        let words = self.tokens(&text.replace("'s", "s").replace(',', " "));
        let readings = self.tagger.as_ref().and_then(|tagger| {
            let prefix = prefix && !text.ends_with(char::is_whitespace);
            let mut query = airmail_tagger::tagger::query(text, prefix, focus);
            if query.words.len() != words.len() {
                return None;
            }
            query.words.clone_from(&words);
            let areas = airmail_tagger::features::Gazetteer::areas(&crate::gazetteer::IndexGazetteer::new(self).ok()?, &query);
            Some(tagger.interpret(&query, &areas, max, coverage))
        });
        (words, readings)
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

    /// The place with this ID, with the administrative areas it's in, or no
    /// hits if there's none. Its score is 0. Runs on tokio's blocking pool.
    ///
    /// # Errors
    /// If the search fails or the stored document can't be decoded.
    #[cfg(feature = "tokio")]
    pub async fn lookup(&self, id: PlaceId) -> Result<SearchResults> {
        let index = self.clone();
        spawn_blocking(move || index.lookup_blocking(&id)).await?
    }

    /// `lookup`, on this thread.
    ///
    /// # Errors
    /// If the search fails or the stored document can't be decoded.
    pub fn lookup_blocking(&self, id: &PlaceId) -> Result<SearchResults> {
        let (field, term) = match id {
            PlaceId::Poi(id) => (FIELD_POI_ID, Term::from_field_u64(self.field(FIELD_POI_ID), *id)),
            PlaceId::Wof(id) => (FIELD_WOF_ID, Term::from_field_u64(self.field(FIELD_WOF_ID), *id)),
            PlaceId::Osm(osm_type, id) => {
                (FIELD_OSM_ID, Term::from_field_text(self.field(FIELD_OSM_ID), &format!("{}/{id}", osm_type.as_str())))
            }
            PlaceId::Postcode(key) => (FIELD_POSTCODE_ID, Term::from_field_text(self.field(FIELD_POSTCODE_ID), key)),
        };
        trace!("Looking up {id} by {field}");
        self.lookup_term(term)
    }

    /// The document whose [`LOCAL_ID_TAG`] is `id` (only in the device's
    /// index), as `lookup_blocking` finds a place.
    ///
    /// # Errors
    /// If the search fails or the stored document can't be decoded.
    pub fn lookup_local_blocking(&self, id: u64) -> Result<SearchResults> {
        self.lookup_term(Term::from_field_text(self.field(FIELD_OSM_ID), &local_key(id)))
    }

    fn lookup_term(&self, term: Term) -> Result<SearchResults> {
        let searcher = self.searcher()?;
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        // An element is indexed once, but a POI ID could in principle be
        // on more than one; the first is as good as any.
        let Some((_, address)) = searcher.search(&query, &TopDocs::with_limit(1).order_by_score())?.into_iter().next() else {
            return Ok(SearchResults::default());
        };
        let hit = self.hit(&searcher, address, 0.0)?;
        let admins = self.admins(&searcher, &hit.admin_ids.iter().copied().collect())?;
        Ok(SearchResults { hits: vec![hit], admins, categories: None })
    }

    pub(crate) fn hit(&self, searcher: &Searcher, address: DocAddress, score: f32) -> Result<SearchHit> {
        let doc: TantivyDocument = searcher.doc(address)?;
        let text = |field: &str| doc.get_first(self.field(field)).and_then(|v| v.as_str()).map(str::to_string);
        let number = |field: &str| doc.get_first(self.field(field)).and_then(|v| v.as_u64());
        let kind = text(FIELD_KIND).as_deref().and_then(Kind::parse).context("document without a kind")?;
        let tags: Vec<(String, String)> = doc
            .get_first(self.field(FIELD_TAGS))
            .and_then(|v| v.as_object())
            .map(|object| object.map(|(k, v)| (k.to_string(), v.as_str().unwrap_or_default().to_string())).collect())
            .unwrap_or_default();
        let osm = text(FIELD_OSM).map(|s| OsmRef::decode(&s)).transpose()?;
        let wof = text(FIELD_WOF).map(|s| serde_json::from_str::<WofPlace>(&s)).transpose()?;
        let admin_ids = doc.get_all(self.field(FIELD_ADMIN_IDS)).filter_map(|v| v.as_u64()).collect();
        let center = number(FIELD_CENTER).context("document without a center")?;
        let center = s2::latlng::LatLng::from(s2::cellid::CellID(center));
        let center = Point::new(center.lng.deg(), center.lat.deg());
        let pointer = number(FIELD_POINTER_POI_ID)
            .zip(number(FIELD_POINTER_TILE))
            .map(|(poi_id, tile)| Pointer { poi_id, tile: TileId::unpack(tile) });
        let geometry = match doc.get_first(self.field(FIELD_GEOMETRY)).and_then(|v| v.as_bytes()) {
            Some(wkb) => geometry::from_wkb(wkb)?,
            None if pointer.is_some() => Geometry::Point(center),
            None => bail!("document without a geometry"),
        };
        let mut hit = SearchHit {
            score,
            kind,
            tags: Vec::new(),
            poi: None,
            pointer,
            osm,
            wof,
            local_id: None,
            admin_ids,
            center,
            geometry,
            importance: doc.get_first(self.field(FIELD_IMPORTANCE)).and_then(|v| v.as_f64()).unwrap_or(0.0),
        };
        hit.set_tags(tags);
        Ok(hit)
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

/// The words of a name, lowercased and transliterated, joined by spaces.
fn name_words(name: &str) -> String {
    name_key(name)
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

/// A name's forms, as [`name_forms`] gives them, with the `name_exact` key of
/// each exact one.
pub struct NameForms {
    exact: Vec<String>,
    other: Vec<String>,
    exact_keys: Vec<String>,
}

impl NameForms {
    fn of(name: &str) -> Self {
        let (exact, other) = name_forms(name);
        let exact_keys = exact.iter().map(|form| name_key(form)).collect();
        Self { exact, other, exact_keys }
    }
}

/// The forms of a document's roads (see `SchemafiedPoi`): every form of each,
/// and the `name_exact` keys of their exact forms, which name a street.
pub struct RoadForms {
    forms: BTreeSet<String>,
    exact_keys: BTreeSet<String>,
}

impl RoadForms {
    fn of(roads: &[String], forms: &mut dyn FnMut(&str) -> Arc<NameForms>) -> Self {
        let mut all = BTreeSet::new();
        let mut exact_keys = BTreeSet::new();
        for road in roads {
            let road_forms = forms(road);
            exact_keys.extend(road_forms.exact_keys.iter().cloned());
            all.extend(road_forms.exact.iter().chain(&road_forms.other).cloned());
        }
        Self { forms: all, exact_keys }
    }
}

/// The most names, or roads, a [`FormsCache`] holds; it starts over when full.
const FORMS_CACHE_ENTRIES: usize = 1 << 10;

/// The forms of names and roads that many documents share, for
/// [`DocumentBuilder::document_cached`]: a region's addresses have a few
/// thousand streets and areas between them.
#[derive(Default)]
pub struct FormsCache {
    names: HashMap<String, Arc<NameForms>>,
    roads: HashMap<(String, Vec<crate::language::Language>), Arc<RoadForms>>,
}

impl FormsCache {
    fn name(&mut self, name: &str) -> Arc<NameForms> {
        if let Some(forms) = self.names.get(name) {
            return forms.clone();
        }
        if self.names.len() >= FORMS_CACHE_ENTRIES {
            self.names.clear();
        }
        let forms = Arc::new(NameForms::of(name));
        self.names.insert(name.to_string(), forms.clone());
        forms
    }

    /// The forms of a POI's road, in its languages (see `SchemafiedPoi`).
    fn roads(&mut self, poi: &ToIndexPoi) -> Arc<RoadForms> {
        let Some(road) = &poi.road else {
            return Arc::new(RoadForms { forms: BTreeSet::new(), exact_keys: BTreeSet::new() });
        };
        let key = (road.clone(), poi.languages.clone());
        if let Some(forms) = self.roads.get(&key) {
            return forms.clone();
        }
        if self.roads.len() >= FORMS_CACHE_ENTRIES {
            self.roads.clear();
        }
        let roads = crate::poi::road_forms(road, &poi.languages);
        let forms = Arc::new(RoadForms::of(&roads, &mut |name| self.name(name)));
        self.roads.insert(key, forms.clone());
        forms
    }
}

/// A document of the index, as `DocumentBuilder` makes it.
pub type Document = TantivyDocument;

/// Builds an index's documents. Building one is most of the work of adding
/// it, and this can do it on any thread, while the writer adds them from one.
#[derive(Clone)]
pub struct DocumentBuilder {
    schema: Schema,
    storage: Storage,
}

impl DocumentBuilder {
    fn field(&self, name: &str) -> tantivy::schema::Field {
        self.schema.get_field(name).expect("schema field")
    }

    /// The words of a name, joined by spaces, as `name_exact` holds it.
    fn exact_key(&self, name: &str) -> String {
        name_key(name)
    }

    /// What this builder's documents store.
    #[must_use]
    pub fn storage(&self) -> Storage {
        self.storage
    }

    /// The document for a POI.
    ///
    /// # Errors
    /// If its shape can't be encoded, or it's to be stored as a pointer (it
    /// has a stable POI ID and the storage is `Storage::Pointers`) but has no
    /// tile.
    pub fn document(&self, schemafied: SchemafiedPoi, source: &str) -> Result<TantivyDocument> {
        let forms = &mut |name: &str| Arc::new(NameForms::of(name));
        let roads = RoadForms::of(&schemafied.roads, forms);
        self.build(&schemafied.poi, &schemafied.names, &roads, forms, source)
    }

    /// The same document as `document`'s, with the forms of names and roads
    /// that documents share (streets, areas) worked out once for many of them.
    ///
    /// # Errors
    /// As `document`.
    pub fn document_cached(&self, poi: ToIndexPoi, source: &str, cache: &mut FormsCache) -> Result<TantivyDocument> {
        let roads = cache.roads(&poi);
        self.build(&poi, &poi.names, &roads, &mut |name| cache.name(name), source)
    }

    fn build(
        &self,
        poi: &ToIndexPoi,
        names: &[String],
        roads: &RoadForms,
        forms: &mut dyn FnMut(&str) -> Arc<NameForms>,
        source: &str,
    ) -> Result<TantivyDocument> {
        let pointer = match (self.storage, poi.poi_id()) {
            (Storage::Pointers, Some(poi_id)) => {
                let tile = poi.tile.with_context(|| format!("POI {poi_id} is stored as a pointer, but has no tile"))?;
                Some(Pointer { poi_id, tile })
            }
            _ => None,
        };
        let mut doc = TantivyDocument::default();
        let mut exact = BTreeSet::new();
        // Every form a name is in, for the gazetteer.
        let mut all_forms = BTreeSet::new();
        for name in names {
            let name_forms = forms(name);
            for (form, key) in name_forms.exact.iter().zip(&name_forms.exact_keys) {
                exact.insert(key.clone());
                all_forms.insert(form.clone());
                doc.add_text(self.field(FIELD_NAME), form);
            }
            for form in &name_forms.other {
                all_forms.insert(form.clone());
                doc.add_text(self.field(FIELD_NAME), form);
            }
        }
        for name in &poi.variant_names {
            let name_forms = forms(name);
            for form in name_forms.exact.iter().chain(&name_forms.other) {
                doc.add_text(self.field(FIELD_NAME), form);
            }
        }
        // A street is named by its road's forms too: `pike st` is exactly
        // Pike Street.
        if poi.kind == Kind::Street {
            exact.extend(roads.exact_keys.iter().cloned());
        }
        for form in &roads.forms {
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
            let name_forms = forms(name);
            admin_forms.extend(name_forms.exact.iter().chain(&name_forms.other).map(|form| (*band, form.clone())));
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
        // A pointer stores only what search needs; the rest is in its tile.
        let stored = pointer.is_none();
        if let Some(osm) = &poi.osm {
            if stored {
                doc.add_text(self.field(FIELD_OSM), osm.encode());
            }
            doc.add_text(self.field(FIELD_OSM_ID), format!("{}/{}", osm.osm_type.as_str(), osm.id));
        }
        if let Some(local) = poi.tag(LOCAL_ID_TAG).and_then(|v| v.parse::<u64>().ok()) {
            doc.add_text(self.field(FIELD_OSM_ID), local_key(local));
        }
        if let Some(poi_id) = poi.poi_id() {
            doc.add_u64(self.field(FIELD_POI_ID), poi_id);
        }
        if let Some(pointer) = pointer {
            doc.add_u64(self.field(FIELD_POINTER_POI_ID), pointer.poi_id);
            doc.add_u64(self.field(FIELD_POINTER_TILE), pointer.tile.pack());
        }
        if let Some(wof) = &poi.wof {
            doc.add_u64(self.field(FIELD_WOF_ID), wof.id);
            if stored {
                doc.add_text(self.field(FIELD_WOF), serde_json::to_string(wof)?);
            }
        }
        // A pointer keeps its names, which ranking reads (how much of a name a
        // query is); its other tags are in its tile.
        let kept: BTreeMap<String, OwnedValue> = poi
            .tags
            .iter()
            .filter(|(k, _)| stored || crate::osm::is_name_key(k))
            .map(|(k, v)| (k.clone(), OwnedValue::Str(v.clone())))
            .collect();
        if !kept.is_empty() {
            doc.add_object(self.field(FIELD_TAGS), kept);
        }
        let center = s2::cellid::CellID::from(s2::latlng::LatLng::from_degrees(poi.center.y(), poi.center.x()));
        doc.add_u64(self.field(FIELD_CENTER), center.0);
        doc.add_u64(self.field(FIELD_IS_POINT), u64::from(!stored || matches!(poi.geometry, Geometry::Point(_))));
        if stored {
            doc.add_bytes(self.field(FIELD_GEOMETRY), &geometry::to_wkb(&poi.geometry)?);
        }
        let mut ancestors = BTreeSet::new();
        for cell in geometry::covering(&poi.geometry) {
            doc.add_u64(self.field(FIELD_S2_COVER), cell.0);
            ancestors.extend(geometry::ancestors(cell).into_iter().map(|c| c.0));
        }
        for cell in ancestors {
            doc.add_u64(self.field(FIELD_S2_COVER_ANCESTORS), cell);
        }
        doc.add_f64(self.field(FIELD_IMPORTANCE), poi.importance);
        Ok(doc)
    }
}

pub struct AirmailIndexWriter {
    tantivy_writer: tantivy::IndexWriter,
    schema: Schema,
    storage: Storage,
}

impl AirmailIndexWriter {
    fn field(&self, name: &str) -> tantivy::schema::Field {
        self.schema.get_field(name).expect("schema field")
    }

    /// Builds documents for this writer, on any thread, stored as the
    /// writer's options say.
    #[must_use]
    pub fn document_builder(&self) -> DocumentBuilder {
        DocumentBuilder { schema: self.schema.clone(), storage: self.storage }
    }

    /// Adds a document from `document_builder`.
    pub fn add_document(&self, doc: TantivyDocument) -> Result<()> {
        self.tantivy_writer.add_document(doc)?;
        Ok(())
    }

    pub fn add_poi(&mut self, schemafied: SchemafiedPoi, source: &str) -> Result<()> {
        let doc = self.document_builder().document(schemafied, source)?;
        self.add_document(doc)
    }

    /// Deletes every document from `source`, so loading it again replaces
    /// it rather than adding duplicates.
    pub fn delete_source(&mut self, source: &str) {
        self.tantivy_writer.delete_term(Term::from_field_text(self.field(FIELD_SOURCE), source));
    }

    /// Commits, then waits for the merges the commit set off. Dropping the
    /// writer would cancel them, leaving the index in many small segments.
    pub fn commit(mut self) -> Result<()> {
        self.tantivy_writer.commit()?;
        self.tantivy_writer.wait_merging_threads()?;
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
            pointer: None,
            osm: None,
            local_id: None,
            wof: Some(WofPlace {
                id,
                placetype: placetype.into(),
                population: None,
                default_name: name.into(),
                names: BTreeMap::new(),
                abbreviation: None,
                country: None,
                size: 0.0,
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
        merge_duplicates(&mut hits);
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
        merge_duplicates(&mut hits);
        assert_eq!(ids(&hits), vec![1, 3]);
    }

    #[test]
    fn test_indexed_tags_keeps_explicit_vegetarian() {
        let terms = indexed_tags(&tags(&[("diet:vegan", "yes"), ("diet:vegetarian", "only")]));
        assert_eq!(terms.into_iter().collect::<Vec<_>>(), vec!["diet:vegan=yes", "diet:vegetarian=only"]);
    }

    #[test]
    fn cached_documents_are_the_same() {
        use crate::language::Language;
        let builder = DocumentBuilder { schema: AirmailIndex::schema(), storage: Storage::Full };
        let at = Geometry::Point(Point::new(-122.3405, 47.6097));
        let mut docs = Vec::new();
        for (street, number, kind) in [
            ("Fremont Ave N", "3401", Kind::Address),
            ("Fremont Ave N", "3405", Kind::Address),
            ("Fremont Ave N", "", Kind::Street),
            ("Ackermannstraße", "1", Kind::Address),
            ("Rue de l'Arsenal", "40", Kind::Address),
            ("Ленинский проспект", "2", Kind::Address),
        ] {
            let names = if kind == Kind::Street { vec![street.to_string()] } else { Vec::new() };
            let mut doc = ToIndexPoi::new(kind, names, at.clone(), Vec::new()).unwrap();
            doc.road = Some(street.to_string());
            doc.house_number = (!number.is_empty()).then(|| number.to_string());
            doc.languages = vec![Language::English, Language::French];
            doc.admin_names =
                vec![(AdminBand::Locality, "Seattle".into()), (AdminBand::Neighbourhood, "Fremont".into())];
            doc.variant_names = vec!["Müllerstraße".into()];
            doc.country_code = Some("US".into());
            docs.push(doc);
        }
        // Twice, for the cached forms.
        docs.extend(docs.clone());
        let mut cache = FormsCache::default();
        for doc in docs {
            let plain = builder.document(SchemafiedPoi::from(doc.clone()), "test").unwrap();
            let cached = builder.document_cached(doc, "test", &mut cache).unwrap();
            assert_eq!(format!("{plain:?}"), format!("{cached:?}"));
        }
    }
}
