use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use geo::Rect;
use itertools::Itertools;
use log::{trace, warn};
use s2::region::RegionCoverer;
use std::collections::{BTreeMap, BTreeSet};
use tantivy::schema::Value;
use tantivy::{
    TantivyDocument, Term,
    collector::TopDocs,
    directory::MmapDirectory,
    query::{BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, PhrasePrefixQuery, Query, TermQuery},
    schema::{
        IndexRecordOption, NumericOptions, OwnedValue, STORED, STRING, Schema, TextFieldIndexing,
        TextOptions,
    },
};
use tokio::task::spawn_blocking;
use unicode_segmentation::UnicodeSegmentation;

use crate::categories::{self, CategoryMode, QueryCategories};
use crate::cuisine;
use crate::error::AirmailError;
use crate::{
    poi::{AirmailPoi, SchemafiedPoi},
    query::all_subsequences,
};

// Field name keys.
pub const FIELD_CONTENT: &str = "content";
pub const FIELD_INDEXED_TAG: &str = "indexed_tag";
pub const FIELD_SOURCE: &str = "source";
pub const FIELD_S2CELL: &str = "s2cell";
pub const FIELD_S2CELL_PARENTS: &str = "s2cell_parents";
pub const FIELD_CATEGORY_JSON: &str = "category";
pub const FIELD_TAGS: &str = "tags";

#[derive(Clone)]
pub struct AirmailIndex {
    tantivy_index: Arc<tantivy::Index>,
}

impl AirmailIndex {
    fn schema() -> tantivy::schema::Schema {
        let mut schema_builder = Schema::builder();
        let text_options = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_fieldnorms(false)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
        let tag_options = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_fieldnorms(false)
                .set_tokenizer("raw")
                .set_index_option(IndexRecordOption::Basic),
        );
        let s2cell_parent_index_options = NumericOptions::default().set_indexed();
        let s2cell_index_options = NumericOptions::default()
            .set_indexed()
            .set_stored()
            .set_fast();
        assert!(!s2cell_parent_index_options.fieldnorms());
        assert!(!s2cell_index_options.fieldnorms());

        let _ = schema_builder.add_text_field(FIELD_CONTENT, text_options.clone());
        let _ = schema_builder.add_text_field(FIELD_INDEXED_TAG, tag_options);
        let _ = schema_builder.add_text_field(FIELD_SOURCE, STRING | STORED);
        let _ = schema_builder.add_u64_field(FIELD_S2CELL, s2cell_index_options);
        let _ = schema_builder.add_u64_field(FIELD_S2CELL_PARENTS, s2cell_parent_index_options);
        let _ = schema_builder.add_json_field(FIELD_TAGS, STORED);
        let _ = schema_builder.add_text_field(FIELD_CATEGORY_JSON, STORED);
        schema_builder.build()
    }

    fn field_content(&self) -> tantivy::schema::Field {
        self.tantivy_index
            .schema()
            .get_field(FIELD_CONTENT)
            .unwrap()
    }

    fn field_indexed_tag(&self) -> tantivy::schema::Field {
        self.tantivy_index
            .schema()
            .get_field(FIELD_INDEXED_TAG)
            .unwrap()
    }

    fn field_source(&self) -> tantivy::schema::Field {
        self.tantivy_index.schema().get_field(FIELD_SOURCE).unwrap()
    }

    fn field_s2cell(&self) -> tantivy::schema::Field {
        self.tantivy_index.schema().get_field(FIELD_S2CELL).unwrap()
    }

    fn field_s2cell_parents(&self) -> tantivy::schema::Field {
        self.tantivy_index
            .schema()
            .get_field(FIELD_S2CELL_PARENTS)
            .unwrap()
    }

    fn field_tags(&self) -> tantivy::schema::Field {
        self.tantivy_index.schema().get_field(FIELD_TAGS).unwrap()
    }

    pub fn create(index_dir: &Path) -> Result<Self> {
        if !index_dir.exists() {
            trace!("Creating index at {:?}", index_dir);
            std::fs::create_dir_all(index_dir)?;
        }

        trace!("Opening index at {:?}", index_dir);
        let schema = Self::schema();
        let tantivy_index =
            tantivy::Index::open_or_create(MmapDirectory::open(index_dir)?, schema)?;
        Ok(Self {
            tantivy_index: Arc::new(tantivy_index),
        })
    }

    pub fn new(index_dir: &str) -> Result<Self> {
        let tantivy_index = tantivy::Index::open_in_dir(index_dir)?;
        Ok(Self {
            tantivy_index: Arc::new(tantivy_index),
        })
    }

    pub fn writer(&mut self) -> Result<AirmailIndexWriter> {
        let tantivy_writer = self
            .tantivy_index
            .writer::<TantivyDocument>(2_000_000_000)?;
        let writer = AirmailIndexWriter {
            tantivy_writer,
            schema: self.tantivy_index.schema(),
        };
        Ok(writer)
    }

    pub async fn merge(&mut self) -> Result<()> {
        let ids = self.tantivy_index.searchable_segment_ids()?;
        self.tantivy_index
            .writer::<TantivyDocument>(2_000_000_000)?
            .merge(&ids)
            .await?;
        Ok(())
    }

    pub async fn num_docs(&self) -> Result<u64> {
        let index = self.tantivy_index.clone();
        let count = spawn_blocking(move || {
            if let Ok(tantivy_reader) = index.reader() {
                Some(tantivy_reader.searcher().num_docs())
            } else {
                None
            }
        });
        Ok(count.await?.ok_or(AirmailError::UnableToCount)?)
    }

    fn construct_query(
        &self,
        query: &str,
        tags: Option<Vec<String>>,
        categories: Option<(&QueryCategories, CategoryMode)>,
        bbox: Option<Rect<f64>>,
        _boost_regions: &[(f32, Rect<f64>)],
        lenient: bool,
    ) -> Box<dyn Query> {
        let mut queries: Vec<Box<dyn Query>> = Vec::new();
        let mut mandatory_queries: Vec<Box<dyn Query>> = Vec::new();

        let tokens: Vec<String> = query
            .split_word_bounds()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        for subsequence in all_subsequences(&tokens) {
            let possible_query = subsequence.join(" ");
            if possible_query
                .chars()
                .all(|c| c.is_whitespace() || c.is_ascii_punctuation())
            {
                continue;
            }

            let non_alphabetic = possible_query
                .chars()
                .filter(|c| c.is_numeric() || c.is_whitespace())
                .count();
            let total_chars = possible_query.chars().count();
            let term = Term::from_field_text(self.field_content(), &possible_query);
            let mut boost = 1.05f32.powf(possible_query.len() as f32);
            // Anecdotally, numbers in queries are usually important.
            if total_chars - non_alphabetic < 3 && non_alphabetic > 0 {
                boost *= 3.0;
            }
            if subsequence.len() > 1 {
                queries.push(Box::new(BoostQuery::new(
                    Box::new(PhrasePrefixQuery::new(
                        subsequence
                            .iter()
                            .map(|s| Term::from_field_text(self.field_content(), s))
                            .collect(),
                    )),
                    boost,
                )));
            } else if possible_query.len() >= 8 && lenient {
                let query = if tokens.ends_with(&[possible_query]) {
                    FuzzyTermQuery::new_prefix(term.clone(), 1, true)
                } else {
                    FuzzyTermQuery::new(term.clone(), 1, true)
                };
                mandatory_queries.push(Box::new(BoostQuery::new(
                    with_exact_score(Box::new(query), term),
                    boost,
                )));
            } else {
                let query: Box<dyn Query> = if !lenient || !tokens.ends_with(&[possible_query]) {
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                } else {
                    with_exact_score(
                        Box::new(FuzzyTermQuery::new_prefix(term.clone(), 0, false)),
                        term,
                    )
                };
                mandatory_queries.push(Box::new(BoostQuery::new(query, boost)));
            }
        }

        // Every query word, optional, for ranking category matches.
        let any_text: Vec<Box<dyn Query>> = queries
            .iter()
            .chain(&mandatory_queries)
            .map(|q| q.box_clone())
            .collect();
        let optional = BooleanQuery::union(queries);
        let required = BooleanQuery::intersection(mandatory_queries);
        let text_query: Box<dyn Query> = Box::new(BooleanQuery::new(vec![
            (Occur::Should, Box::new(optional)),
            (Occur::Must, Box::new(required)),
        ]));

        // A query matches either by its words or by the categories it asks for.
        let mut final_query: Box<dyn Query> = match categories
            .and_then(|(c, mode)| Some((self.category_query(c, any_text)?, mode)))
        {
            Some((category_query, CategoryMode::Include)) => {
                Box::new(BooleanQuery::union(vec![text_query, category_query]))
            }
            Some((category_query, CategoryMode::Boost)) => Box::new(BooleanQuery::new(vec![
                (Occur::Must, text_query),
                (Occur::Should, category_query),
            ])),
            None => text_query,
        };

        if let Some(tags) = tags {
            let mut clauses = vec![(Occur::Must, final_query)];
            for tag in &tags {
                let term = Term::from_field_text(self.field_indexed_tag(), tag);
                let query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                clauses.push((Occur::Must, query));
            }
            final_query = Box::new(BooleanQuery::new(clauses));
        }

        if let Some(bbox) = bbox {
            let region = s2::rect::Rect::from_degrees(
                bbox.min().y,
                bbox.min().x,
                bbox.max().y,
                bbox.max().x,
            );
            let covering_cells = {
                let coverer = RegionCoverer {
                    min_level: 0,
                    max_level: 16,
                    level_mod: 1,
                    max_cells: 64,
                };
                let mut cellunion = coverer.covering(&region);
                cellunion.normalize();
                cellunion.0.iter().map(|c| c.0).collect_vec()
            };
            let covering_disjunction_clauses = covering_cells
                .iter()
                .map(|c| {
                    let term = Term::from_field_u64(self.field_s2cell_parents(), *c);
                    let query: Box<dyn Query> =
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                    query
                })
                .collect_vec();
            let covering_query = BooleanQuery::union(covering_disjunction_clauses);
            return Box::new(BooleanQuery::intersection(vec![
                Box::new(covering_query),
                final_query,
            ]));
        }

        final_query
    }

    /// Matches POIs with the most specific confident signal in `understood`
    /// (cuisines, else categories, else diets), boosted by confidence. Diets
    /// that aren't the primary signal and the query's words only add to the
    /// score, since diet tags are sparse.
    fn category_query(
        &self,
        understood: &QueryCategories,
        any_text: Vec<Box<dyn Query>>,
    ) -> Option<Box<dyn Query>> {
        let tag_query = |tag: &str, boost: f32| -> Box<dyn Query> {
            let term = Term::from_field_text(self.field_indexed_tag(), tag);
            Box::new(BoostQuery::new(
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                boost,
            ))
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
            let cuisines = understood
                .cuisines
                .iter()
                .map(|(c, p)| tag_query(&format!("cuisine={c}"), *p))
                .collect();
            clauses.push((Occur::Must, Box::new(BooleanQuery::union(cuisines))));
            clauses.push((Occur::Should, Box::new(BooleanQuery::union(diet_queries()))));
        } else if !understood.categories.is_empty() {
            let categories = understood
                .categories
                .iter()
                .map(|(c, p)| tag_query(c, *p))
                .collect();
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

    /// This is public because I don't want one big mega-crate but its API should not be considered even remotely stable.
    pub async fn search(
        &self,
        query: &str,
        request_leniency: bool,
        tags: Option<Vec<String>>,
        categories: Option<(&QueryCategories, CategoryMode)>,
        bbox: Option<Rect<f64>>,
        boost_regions: &[(f32, Rect<f64>)],
    ) -> Result<Vec<(AirmailPoi, f32)>> {
        let tantivy_reader = self.tantivy_index.reader()?;
        let searcher = tantivy_reader.searcher();
        let query_string = query.trim().replace("'s", "s");

        let start = std::time::Instant::now();

        let query = self.construct_query(
            &query_string,
            tags,
            categories,
            bbox,
            boost_regions,
            request_leniency,
        );

        #[cfg(feature = "invasive_logging")]
        trace!("Search query: {:?}", &query);

        // Perform the search and then resolve the returned documents
        let top_docs: Result<Vec<(f32, TantivyDocument)>> = spawn_blocking(move || {
            let doc_addresses =
                searcher.search(&query, &TopDocs::with_limit(10).order_by_score())?;
            let mut docs = vec![];
            for (score, doc_address) in doc_addresses {
                if let Ok(doc) = searcher.doc::<TantivyDocument>(doc_address) {
                    docs.push((score, doc));
                }
            }

            Ok(docs)
        })
        .await?;

        let top_docs = top_docs.map_err(|e| {
            warn!("Search failed: {:?}", e);
            e
        })?;

        trace!(
            "Search took {:?} and yielded {} results",
            start.elapsed(),
            top_docs.len()
        );

        let results = top_docs
            .into_iter()
            .flat_map(|(score, doc)| {
                let source = doc
                    .get_first(self.field_source())
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .unwrap_or_default();
                let s2cell = doc.get_first(self.field_s2cell())?.as_u64()?;
                let cellid = s2::cellid::CellID(s2cell);
                let latlng = s2::latlng::LatLng::from(cellid);
                let tags: Vec<(String, String)> = doc
                    .get_first(self.field_tags())?
                    .as_object()?
                    .map(|(k, v)| (k.to_string(), v.as_str().unwrap_or_default().to_string()))
                    .collect();

                AirmailPoi::new(source, latlng.lat.deg(), latlng.lng.deg(), tags)
                    .ok()
                    .map(|poi| (poi, score))
            })
            .collect::<Vec<_>>();

        Ok(results)
    }
}

/// Matches what `matcher` matches, adding a BM25 score for an exact match of
/// `term`. Fuzzy and prefix queries score every match the same, so without
/// this a rare word like a POI's name ranks no higher than a common one, and
/// below POIs matching only the query's categories.
fn with_exact_score(matcher: Box<dyn Query>, term: Term) -> Box<dyn Query> {
    Box::new(BooleanQuery::new(vec![
        (Occur::Must, matcher),
        (
            Occur::Should,
            Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
        ),
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
                terms.extend(
                    cuisine::with_ancestors(&value)
                        .into_iter()
                        .map(|c| format!("cuisine={c}")),
                );
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
    fn process_field(&self, doc: &mut TantivyDocument, value: &str) {
        // Queries are transliterated to ASCII before searching, so content must be too.
        doc.add_text(
            self.schema.get_field(FIELD_CONTENT).unwrap(),
            deunicode::deunicode(value),
        );
    }

    pub fn add_poi(&mut self, poi: SchemafiedPoi, source: &str) -> Result<()> {
        let mut doc = TantivyDocument::default();
        for content in poi.content {
            self.process_field(&mut doc, &content);
        }
        doc.add_text(self.schema.get_field(FIELD_SOURCE)?, source);

        let indexed_tag = self.schema.get_field(FIELD_INDEXED_TAG)?;
        for term in indexed_tags(&poi.tags) {
            doc.add_text(indexed_tag, &term);
        }
        doc.add_object(
            self.schema.get_field(FIELD_TAGS)?,
            poi.tags
                .iter()
                .map(|(k, v)| (k.to_string(), OwnedValue::Str(v.to_string())))
                .collect::<BTreeMap<String, OwnedValue>>(),
        );

        doc.add_u64(self.schema.get_field(FIELD_S2CELL)?, poi.s2cell);
        for parent in poi.s2cell_parents {
            doc.add_u64(self.schema.get_field(FIELD_S2CELL_PARENTS)?, parent);
        }
        self.tantivy_writer.add_document(doc)?;

        Ok(())
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
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
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

    #[test]
    fn test_indexed_tags_keeps_explicit_vegetarian() {
        let terms = indexed_tags(&tags(&[("diet:vegan", "yes"), ("diet:vegetarian", "only")]));
        assert_eq!(
            terms.into_iter().collect::<Vec<_>>(),
            vec!["diet:vegan=yes", "diet:vegetarian=only"]
        );
    }
}
