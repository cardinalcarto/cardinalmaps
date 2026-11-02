use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use geo::{Distance, Geometry, Point};
use log::{trace, warn};
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
    query::{
        BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, PhrasePrefixQuery, PhraseQuery, Query,
        TermQuery,
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
use crate::poi::{Kind, OsmRef, SchemafiedPoi, WofPlace};
use crate::query::all_subsequences;

/// The version of the index's schema and of what its fields hold. Readers
/// refuse indexes of other versions.
pub const SCHEMA_VERSION: u32 = 2;

/// Records `SCHEMA_VERSION` in the index directory.
const VERSION_FILE: &str = "airmail-index.json";

// Searchable text.
pub const FIELD_NAME: &str = "name";
pub const FIELD_NAME_EXACT: &str = "name_exact";
pub const FIELD_ADDRESS: &str = "address";
pub const FIELD_ADMIN: &str = "admin";
pub const FIELD_INDEXED_TAG: &str = "indexed_tag";
// Filters and identity.
pub const FIELD_KIND: &str = "kind";
pub const FIELD_SOURCE: &str = "source";
pub const FIELD_ADMIN_IDS: &str = "admin_ids";
pub const FIELD_WOF_ID: &str = "wof_id";
pub const FIELD_S2_COVER: &str = "s2_cover";
pub const FIELD_S2_COVER_ANCESTORS: &str = "s2_cover_ancestors";
// Stored for results.
pub const FIELD_OSM: &str = "osm";
pub const FIELD_WOF: &str = "wof";
pub const FIELD_TAGS: &str = "tags";
pub const FIELD_CENTER: &str = "center";
pub const FIELD_GEOMETRY: &str = "geometry";
pub const FIELD_IMPORTANCE: &str = "importance";

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
/// elsewhere, such as `seattle wa`. More than a phrase match, so `seattle
/// washington` is Seattle rather than something named with both words.
const EXACT_NAME_PREFIX_BOOST: f32 = 10.0;
/// For a query that's a name with typos, or its prefix.
const NEAR_EXACT_NAME_BOOST: f32 = 10.0;

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
    tantivy_index: Arc<tantivy::Index>,
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
        schema_builder.add_text_field(FIELD_ADDRESS, text(true));
        schema_builder.add_text_field(FIELD_ADMIN, text(false));
        schema_builder.add_text_field(FIELD_INDEXED_TAG, raw);
        schema_builder.add_text_field(FIELD_KIND, STRING | STORED);
        schema_builder.add_text_field(FIELD_SOURCE, STRING | STORED);
        schema_builder.add_u64_field(FIELD_ADMIN_IDS, indexed_u64.clone().set_stored());
        schema_builder.add_u64_field(FIELD_WOF_ID, indexed_u64.clone());
        schema_builder.add_u64_field(FIELD_S2_COVER, indexed_u64.clone());
        schema_builder.add_u64_field(FIELD_S2_COVER_ANCESTORS, indexed_u64);
        schema_builder.add_text_field(FIELD_OSM, STORED);
        schema_builder.add_text_field(FIELD_WOF, STORED);
        schema_builder.add_json_field(FIELD_TAGS, STORED);
        schema_builder.add_u64_field(FIELD_CENTER, STORED);
        schema_builder.add_bytes_field(FIELD_GEOMETRY, STORED);
        schema_builder.add_f64_field(FIELD_IMPORTANCE, FAST | STORED);
        schema_builder.build()
    }

    fn field(&self, name: &str) -> tantivy::schema::Field {
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
        Ok(Self { tantivy_index: Arc::new(tantivy_index) })
    }

    /// Opens an existing index.
    ///
    /// # Errors
    /// If there's no index of this schema version there.
    pub fn new(index_dir: &str) -> Result<Self> {
        check_version(Path::new(index_dir))?;
        let tantivy_index = tantivy::Index::open_in_dir(index_dir)?;
        Ok(Self { tantivy_index: Arc::new(tantivy_index) })
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
        Ok(Self { tantivy_index: Arc::new(tantivy_index) })
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
    fn tokens(&self, text: &str) -> Vec<String> {
        let mut analyzer = self.tantivy_index.tokenizer_for_field(self.field(FIELD_NAME)).expect("tokenizer");
        let mut tokens = Vec::new();
        let normalized = normalize_text(text);
        let mut stream = analyzer.token_stream(&normalized);
        stream.process(&mut |token| tokens.push(token.text.clone()));
        tokens
    }

    /// Matches one query word in one field.
    fn word_query(&self, field: &str, word: &str, prefix: bool, typos: bool) -> Box<dyn Query> {
        let term = Term::from_field_text(self.field(field), word);
        let distance = if typos { typo_distance(word) } else { 0 };
        // Without term frequencies, so a place with its name in fifty
        // languages scores no higher than one with a single name.
        let exact: Box<dyn Query> = Box::new(TermQuery::new(term.clone(), IndexRecordOption::Basic));
        if prefix {
            with_exact_score(Box::new(FuzzyTermQuery::new_prefix(term.clone(), distance, true)), term)
        } else if distance > 0 {
            with_exact_score(Box::new(FuzzyTermQuery::new(term.clone(), distance, true)), term)
        } else {
            exact
        }
    }

    /// Requires every word of the query, each in some text field, and ranks
    /// phrase and exact name matches higher. Also returns the per-word
    /// queries, for ranking category matches.
    fn text_query(&self, tokens: &[String], prefix: bool, typos: bool) -> TextQueries {
        let words_with = |admin_boost: f32| -> Vec<Box<dyn Query>> {
            tokens
                .iter()
                .enumerate()
                .map(|(i, word)| {
                    let last = i + 1 == tokens.len();
                    let fields = [(FIELD_NAME, NAME_BOOST), (FIELD_ADDRESS, ADDRESS_BOOST), (FIELD_ADMIN, admin_boost)];
                    let per_field = fields
                        .into_iter()
                        .map(|(field, boost)| -> Box<dyn Query> {
                            Box::new(BoostQuery::new(self.word_query(field, word, prefix && last, typos), boost))
                        })
                        .collect();
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
            for (field, field_boost) in [(FIELD_NAME, NAME_BOOST), (FIELD_ADDRESS, ADDRESS_BOOST)] {
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
            let boost = if end == tokens.len() { EXACT_NAME_BOOST } else { EXACT_NAME_PREFIX_BOOST };
            optional.push(Box::new(BoostQuery::new(Box::new(TermQuery::new(term.clone(), IndexRecordOption::Basic)), boost)));
            // The whole query as a name with typos, or a name's prefix.
            if end == tokens.len() && (prefix || typos) {
                let distance = if typos { typo_distance(&key) } else { 0 };
                let near: Box<dyn Query> = if prefix {
                    Box::new(FuzzyTermQuery::new_prefix(term, distance, true))
                } else {
                    Box::new(FuzzyTermQuery::new(term, distance, true))
                };
                optional.push(Box::new(BoostQuery::new(near, NEAR_EXACT_NAME_BOOST)));
            }
        }

        let any_text = words.iter().map(|q| q.box_clone()).collect();
        let all = Box::new(BooleanQuery::new(vec![
            (Occur::Must, Box::new(BooleanQuery::intersection(words)) as Box<dyn Query>),
            (Occur::Should, Box::new(BooleanQuery::union(optional))),
        ]));
        TextQueries { all, any_text, any_text_located: words_with(CATEGORY_ADMIN_BOOST) }
    }

    /// Matches documents whose covering intersects the S2 cells.
    fn covering_query(&self, cells: &[s2::cellid::CellID]) -> Box<dyn Query> {
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

    fn construct_query(&self, request: &SearchRequest, bbox_cells: Option<&[s2::cellid::CellID]>) -> Box<dyn Query> {
        let tokens = self.tokens(&request.text.replace("'s", "s"));
        let text = self.text_query(&tokens, request.prefix, request.typos);

        // A query matches either by its words or by the categories it asks
        // for. A POI with all the words is scored as a name match, so the
        // Spokane station doesn't outrank Spokane for being in Spokane.
        let mut query: Box<dyn Query> = match request.categories {
            Some((categories, mode)) => match self.category_query(categories, text.any_text) {
                Some(category_query) => {
                    let matched = Box::new(BooleanQuery::new(vec![
                        (Occur::Must, text.all.box_clone()),
                        (Occur::Should, category_query),
                    ]));
                    match (mode, self.category_query(categories, text.any_text_located)) {
                        (CategoryMode::Include, Some(located)) => Box::new(BooleanQuery::union(vec![
                            matched,
                            Box::new(BooleanQuery::new(vec![(Occur::Must, located), (Occur::MustNot, text.all)])),
                        ])),
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
        let query = self.construct_query(request, bbox_cells.as_deref());
        let limit = request.limit.max(1);
        // Coverings are larger than shapes, so some candidates in the box
        // are dropped below; duplicate admins are too.
        let candidates = if bbox_rects.is_some() { (limit * 4).min(200) } else { limit + 5 };

        let index = self.clone();
        let results = spawn_blocking(move || -> Result<SearchResults> {
            let searcher = index.tantivy_index.reader()?.searcher();
            let collector = TopDocs::with_limit(candidates).tweak_score(move |segment: &tantivy::SegmentReader| {
                let importance = segment.fast_fields().f64(FIELD_IMPORTANCE).ok();
                move |doc, score: f32| {
                    let importance = importance.as_ref().and_then(|column| column.first(doc)).unwrap_or(0.0);
                    score * (1.0 + IMPORTANCE_WEIGHT * importance as f32)
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
            rerank_admins(&mut hits);
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

    fn hit(&self, searcher: &Searcher, address: DocAddress, score: f32) -> Result<SearchHit> {
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
    fn admins(&self, searcher: &Searcher, ids: &BTreeSet<u64>) -> Result<HashMap<u64, WofPlace>> {
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

/// Two admins this close with the same name and placetype are the same
/// place recorded twice.
const DUPLICATE_ADMIN_KM: f64 = 25.0;

/// Drops admins that duplicate a better-ranked one, and puts a place ahead
/// of a same-named area containing it, since `zurich` usually means the city
/// rather than the canton.
fn rerank_admins(hits: &mut Vec<SearchHit>) {
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
            (i + 1..hits.len())
                .find(|&j| hits[j].admin_ids.contains(&area.id) && !names(&hits[j]).is_disjoint(&area_names))
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
fn typo_distance(word: &str) -> u8 {
    match word.chars().count() {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    }
}

/// Text as it's indexed: transliterated to ASCII. The tokenizer lowercases
/// and splits it into words.
#[must_use]
pub fn normalize_text(text: &str) -> String {
    deunicode::deunicode(text)
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
        normalize_text(name)
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn add_poi(&mut self, schemafied: SchemafiedPoi, source: &str) -> Result<()> {
        let poi = &schemafied.poi;
        let mut doc = TantivyDocument::default();
        let mut exact = BTreeSet::new();
        for name in &schemafied.names {
            doc.add_text(self.field(FIELD_NAME), normalize_text(name));
            exact.insert(self.exact_key(&name.replace("'s", "s")));
        }
        for key in exact {
            if !key.is_empty() {
                doc.add_text(self.field(FIELD_NAME_EXACT), key);
            }
        }
        for address in &schemafied.address {
            doc.add_text(self.field(FIELD_ADDRESS), normalize_text(address));
        }
        for admin in &schemafied.admin_names {
            doc.add_text(self.field(FIELD_ADMIN), normalize_text(admin));
        }
        for term in indexed_tags(&poi.tags) {
            doc.add_text(self.field(FIELD_INDEXED_TAG), &term);
        }
        doc.add_text(self.field(FIELD_KIND), poi.kind.as_str());
        doc.add_text(self.field(FIELD_SOURCE), source);
        for id in &poi.admin_ids {
            doc.add_u64(self.field(FIELD_ADMIN_IDS), *id);
        }
        if let Some(osm) = &poi.osm {
            doc.add_text(self.field(FIELD_OSM), osm.encode());
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
}
