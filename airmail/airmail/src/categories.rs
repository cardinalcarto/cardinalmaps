//! Categorical query understanding: which POI categories, cuisines and diets a
//! search query asks for.
//!
//! The model is trained by `airmail/categorical` (fastText, three heads) and
//! exported to a single file. Inference here reproduces fastText's supervised
//! prediction exactly, apart from the 8-bit quantization of input embeddings.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::cuisine;

/// Keys whose tags are POI categories. Copied from `POI_KEYS` in
/// `poi_service/src/rules.rs`; must match `POI_KEYS` in
/// `airmail/categorical/src/airmail_categories/tags.py`.
pub const POI_KEYS: [&str; 36] = [
    "advertising",
    "allotments",
    "amenity",
    "attraction",
    "barrier",
    "boundary",
    "building",
    "building:part",
    "cemetery",
    "club",
    "craft",
    "education",
    "emergency",
    "entrance",
    "geological",
    "golf",
    "healthcare",
    "historic",
    "indoor",
    "landcover",
    "landuse",
    "leisure",
    "man_made",
    "military",
    "natural",
    "office",
    "piste:type",
    "place",
    "playground",
    "power",
    "public_transport",
    "roller_coaster",
    "shop",
    "telecom",
    "tourism",
    "waterway",
];

/// Tags that are POI categories though their key isn't. Copied from
/// `POI_TAGS` in `poi_service/src/rules.rs`.
pub const POI_TAGS: [(&str, &str); 2] = [("highway", "bus_stop"), ("railway", "subway_entrance")];

/// Values that don't say what a place is, such as `building=yes`. Must match
/// `UNINFORMATIVE_VALUES` in `tags.py`.
pub const UNINFORMATIVE_VALUES: [&str; 4] = ["yes", "no", "vacant", "disused"];

/// Diet values that mean a place serves that diet.
pub const DIET_POSITIVE_VALUES: [&str; 2] = ["yes", "only"];

/// True if `key=value` is a POI category tag the model can predict.
#[must_use]
pub fn is_category_tag(key: &str, value: &str) -> bool {
    (POI_KEYS.binary_search(&key).is_ok() || POI_TAGS.contains(&(key, value)))
        && !UNINFORMATIVE_VALUES.contains(&value)
}

/// The ISO 639-3 code for a language tag such as `de`, `de-CH`, `zh-Hans` or
/// `deu`, as Who's On First uses. Only the primary subtag counts.
#[must_use]
pub fn iso_639_3(tag: &str) -> Option<&'static str> {
    let primary = tag.split(['-', '_']).next()?.to_lowercase();
    let language = match primary.len() {
        2 => isolang::Language::from_639_1(&primary),
        3 => isolang::Language::from_639_3(&primary),
        _ => None,
    }?;
    Some(language.to_639_3())
}

/// Splits a `;`-separated OSM value into lowercase, trimmed parts.
#[must_use]
pub fn split_values(value: &str) -> Vec<String> {
    let mut parts: Vec<String> = value
        .split(';')
        .map(|part| part.trim().to_lowercase())
        .filter(|part| !part.is_empty())
        .collect();
    parts.sort();
    parts.dedup();
    parts
}

const MAGIC: &[u8; 8] = b"AMCATS01";
const EOS: &str = "</s>";

/// Lowercases and collapses whitespace. Must match `normalize` in
/// `airmail/categorical/src/airmail_categories/tags.py`.
#[must_use]
pub fn normalize(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Deserialize)]
struct Header {
    version: u32,
    /// ISO 639-3 code of the language the model understands.
    language: String,
    head_order: Vec<String>,
    heads: HashMap<String, HeadHeader>,
    food_categories: Vec<String>,
    diets: Vec<String>,
    thresholds: Thresholds,
}

#[derive(Debug, Deserialize)]
struct HeadHeader {
    dim: usize,
    minn: usize,
    maxn: usize,
    bucket: usize,
    word_ngrams: usize,
    loss: Loss,
    words: Vec<String>,
    labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Loss {
    Softmax,
    Ova,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Thresholds {
    categories: f32,
    cuisine: f32,
    diet: f32,
}

/// One fastText supervised model.
struct Head {
    dim: usize,
    minn: usize,
    maxn: usize,
    bucket: usize,
    word_ngrams: usize,
    loss: Loss,
    word_ids: HashMap<String, usize>,
    labels: Vec<String>,
    /// `(words + bucket) x dim`, each row scaled by `scales`.
    input: Vec<i8>,
    scales: Vec<f32>,
    /// `labels x dim`.
    output: Vec<f32>,
}

/// fastText's FNV-1a variant, which sign-extends each byte.
fn fasttext_hash(text: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for &byte in text.as_bytes() {
        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
        let extended = i32::from(byte as i8) as u32;
        h ^= extended;
        h = h.wrapping_mul(16_777_619);
    }
    h
}

impl Head {
    fn nwords(&self) -> usize {
        self.word_ids.len()
    }

    /// Bucket rows for the character n-grams of `<token>`, as fastText's
    /// `computeSubwords`.
    fn push_subwords(&self, token: &str, ids: &mut Vec<usize>) {
        if self.maxn == 0 || token == EOS {
            return;
        }
        let word = format!("<{token}>");
        let bytes = word.as_bytes();
        let is_continuation = |b: u8| b & 0xC0 == 0x80;
        for i in 0..bytes.len() {
            if is_continuation(bytes[i]) {
                continue;
            }
            let mut j = i;
            let mut n = 1;
            while j < bytes.len() && n <= self.maxn {
                j += 1;
                while j < bytes.len() && is_continuation(bytes[j]) {
                    j += 1;
                }
                if n >= self.minn && !(n == 1 && (i == 0 || j == bytes.len())) {
                    let ngram = &word[i..j];
                    ids.push(self.nwords() + fasttext_hash(ngram) as usize % self.bucket);
                }
                n += 1;
            }
        }
    }

    /// Input rows for `text`, as fastText's `Dictionary::getLine`.
    fn features(&self, text: &str) -> Vec<usize> {
        let mut ids = Vec::new();
        let mut hashes: Vec<i32> = Vec::new();
        for token in text.split(' ').filter(|t| !t.is_empty()).chain([EOS]) {
            let h = fasttext_hash(token);
            if let Some(&id) = self.word_ids.get(token) {
                ids.push(id);
            }
            self.push_subwords(token, &mut ids);
            #[allow(clippy::cast_possible_wrap)]
            hashes.push(h as i32);
        }
        for i in 0..hashes.len() {
            #[allow(clippy::cast_sign_loss)]
            let mut h = i64::from(hashes[i]) as u64;
            for &next in hashes
                .iter()
                .skip(i + 1)
                .take(self.word_ngrams.saturating_sub(1))
            {
                #[allow(clippy::cast_sign_loss)]
                let next = i64::from(next) as u64;
                h = h.wrapping_mul(116_049_371).wrapping_add(next);
                #[allow(clippy::cast_possible_truncation)]
                ids.push(self.nwords() + (h % self.bucket as u64) as usize);
            }
        }
        ids
    }

    /// Probability of each label for normalized `text`.
    fn predict(&self, text: &str) -> Vec<f32> {
        let ids = self.features(text);
        let mut hidden = vec![0f32; self.dim];
        for &id in &ids {
            let row = &self.input[id * self.dim..(id + 1) * self.dim];
            let scale = self.scales[id];
            for (h, &q) in hidden.iter_mut().zip(row) {
                *h += f32::from(q) * scale;
            }
        }
        if !ids.is_empty() {
            #[allow(clippy::cast_precision_loss)]
            let n = ids.len() as f32;
            hidden.iter_mut().for_each(|h| *h /= n);
        }
        let scores: Vec<f32> = self
            .output
            .chunks_exact(self.dim)
            .map(|row| row.iter().zip(&hidden).map(|(w, h)| w * h).sum())
            .collect();
        match self.loss {
            Loss::Ova => scores.iter().map(|s| 1.0 / (1.0 + (-s).exp())).collect(),
            Loss::Softmax => {
                let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let exps: Vec<f32> = scores.iter().map(|s| (s - max).exp()).collect();
                let total: f32 = exps.iter().sum();
                exps.iter().map(|e| e / total).collect()
            }
        }
    }

    fn labelled(&self, text: &str) -> Vec<(String, f32)> {
        let mut out: Vec<_> = self
            .labels
            .iter()
            .cloned()
            .zip(self.predict(text))
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }
}

/// Categorical models by language. Meaning depends on language, so each language
/// has its own model and categorical search needs the query's language.
#[derive(Default)]
pub struct CategoryModels {
    by_language: HashMap<String, CategoryModel>,
}

impl CategoryModels {
    /// Loads `path`: a model file, or a directory of `.amcats` files.
    pub fn open(path: &Path) -> Result<Self> {
        let mut models = Self::default();
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let file = entry?.path();
                if file.extension().is_some_and(|e| e == "amcats") {
                    models.insert(CategoryModel::open(&file)?);
                }
            }
        } else {
            models.insert(CategoryModel::open(path)?);
        }
        Ok(models)
    }

    pub fn insert(&mut self, model: CategoryModel) {
        self.by_language.insert(model.language.clone(), model);
    }

    /// The model for a language tag such as `de`, `de-CH` or `deu`.
    #[must_use]
    pub fn get(&self, language: &str) -> Option<&CategoryModel> {
        self.by_language.get(iso_639_3(language)?)
    }

    /// ISO 639-3 codes of the loaded models, sorted.
    #[must_use]
    pub fn languages(&self) -> Vec<&str> {
        let mut languages: Vec<&str> = self.by_language.keys().map(String::as_str).collect();
        languages.sort_unstable();
        languages
    }
}

/// What a query asks for, each with the model's confidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct QueryCategories {
    /// POI category tags as `key=value`, most likely first.
    pub categories: Vec<(String, f32)>,
    /// Probability the query is for a place that serves food.
    pub food: f32,
    /// Cuisine values, keeping only the most specific confident ones.
    pub cuisines: Vec<(String, f32)>,
    /// Diet keys such as `vegan`.
    pub diets: Vec<(String, f32)>,
}

impl QueryCategories {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.categories.is_empty() && self.cuisines.is_empty() && self.diets.is_empty()
    }

    /// Confidence in the signal search matches on: cuisines if any, else
    /// categories, else diets (see `AirmailIndex::category_query`).
    #[must_use]
    pub fn primary_confidence(&self) -> f32 {
        let max = |signal: &[(String, f32)]| signal.iter().map(|(_, p)| *p).fold(0.0, f32::max);
        if !self.cuisines.is_empty() {
            max(&self.cuisines)
        } else if !self.categories.is_empty() {
            self.categories.iter().map(|(_, p)| *p).sum::<f32>().min(1.0)
        } else {
            max(&self.diets)
        }
    }

    /// Whether search should also return POIs matching only by category, or
    /// just rerank POIs the query's words match. Including is right for
    /// category queries ("pizza", "vegan food") and costs name queries little,
    /// since name matches still rank first, so it only switches to boosting
    /// when the model is unsure.
    #[must_use]
    pub fn mode(&self, include_threshold: f32) -> CategoryMode {
        if self.primary_confidence() >= include_threshold {
            CategoryMode::Include
        } else {
            CategoryMode::Boost
        }
    }
}

/// Default for [`QueryCategories::mode`], chosen from a small set of name and
/// category searches in Zürich: every category query there had a primary
/// confidence of at least 0.46.
pub const DEFAULT_INCLUDE_THRESHOLD: f32 = 0.4;

/// How a query's categories affect results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryMode {
    /// Also return POIs that match only by category.
    Include,
    /// Only rerank POIs the query's words match.
    Boost,
}

pub struct CategoryModel {
    language: String,
    categories: Head,
    cuisine: Head,
    diet: Head,
    food_categories: BTreeSet<String>,
    diets: Vec<String>,
    thresholds: Thresholds,
}

impl CategoryModel {
    /// Load a model file exported by `categories-train`.
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 12 && &bytes[..8] == MAGIC,
            "not a categories model"
        );
        let header_len = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
        let header: Header = serde_json::from_slice(
            bytes
                .get(12..12 + header_len)
                .context("truncated model header")?,
        )?;
        ensure!(
            header.version == 1,
            "unsupported model version {}",
            header.version
        );

        let mut rest = &bytes[12 + header_len..];
        let mut heads = HashMap::new();
        for name in &header.head_order {
            let h = header
                .heads
                .get(name)
                .with_context(|| format!("missing head {name}"))?;
            let rows = h.words.len() + h.bucket;
            let take = |rest: &mut &[u8], n: usize| -> Result<Vec<u8>> {
                ensure!(rest.len() >= n, "truncated model data");
                let (head, tail) = rest.split_at(n);
                *rest = tail;
                Ok(head.to_vec())
            };
            #[allow(clippy::cast_possible_wrap)]
            let input = take(&mut rest, rows * h.dim)?
                .into_iter()
                .map(|b| b as i8)
                .collect();
            let floats = |raw: Vec<u8>| -> Vec<f32> {
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect()
            };
            let scales = floats(take(&mut rest, rows * 4)?);
            let output = floats(take(&mut rest, h.labels.len() * h.dim * 4)?);
            heads.insert(
                name.clone(),
                Head {
                    dim: h.dim,
                    minn: h.minn,
                    maxn: h.maxn,
                    bucket: h.bucket,
                    word_ngrams: h.word_ngrams,
                    loss: h.loss,
                    word_ids: h
                        .words
                        .iter()
                        .enumerate()
                        .map(|(i, w)| (w.clone(), i))
                        .collect(),
                    labels: h.labels.clone(),
                    input,
                    scales,
                    output,
                },
            );
        }
        ensure!(rest.is_empty(), "trailing data after model");
        let mut take_head = |name: &str| {
            heads
                .remove(name)
                .with_context(|| format!("model has no {name} head"))
        };
        let (categories, cuisine, diet) = (
            take_head("categories")?,
            take_head("cuisine")?,
            take_head("diet")?,
        );
        if heads.is_empty() {
            Ok(Self {
                language: header.language,
                categories,
                cuisine,
                diet,
                food_categories: header.food_categories.into_iter().collect(),
                diets: header.diets,
                thresholds: header.thresholds,
            })
        } else {
            bail!("unexpected heads: {:?}", heads.keys().collect::<Vec<_>>())
        }
    }

    /// ISO 639-3 code of the language this model understands.
    #[must_use]
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Raw per-head label probabilities for `query`, for parity tests and debugging.
    #[must_use]
    pub fn head_probabilities(&self, query: &str) -> HashMap<&'static str, Vec<(String, f32)>> {
        let text = normalize(query);
        HashMap::from([
            ("categories", self.categories.labelled(&text)),
            ("cuisine", self.cuisine.labelled(&text)),
            ("diet", self.diet.labelled(&text)),
        ])
    }

    /// What `query` asks for. Must match `understand` in
    /// `airmail/categorical/src/airmail_categories/evaluate.py`.
    #[must_use]
    pub fn understand(&self, query: &str) -> QueryCategories {
        let text = normalize(query);
        let t = self.thresholds;

        let categories = self.categories.labelled(&text);
        let food = categories
            .iter()
            .filter(|(c, _)| self.food_categories.contains(c))
            .map(|(_, p)| p)
            .sum::<f32>()
            .min(1.0);

        // Cuisines are only meaningful for food queries, and a confident
        // `chinese` makes its ancestor `asian` redundant.
        let cuisines: Vec<(String, f32)> = self
            .cuisine
            .labelled(&text)
            .into_iter()
            .map(|(c, p)| (c, p * food))
            .filter(|(_, p)| *p >= t.cuisine)
            .collect();
        let implied: BTreeSet<String> = cuisines
            .iter()
            .flat_map(|(c, _)| {
                let mut ancestors = cuisine::with_ancestors(c);
                ancestors.remove(c);
                ancestors
            })
            .collect();
        let cuisines = cuisines
            .into_iter()
            .filter(|(c, _)| !implied.contains(c))
            .collect();

        let diets = self
            .diet
            .labelled(&text)
            .into_iter()
            .filter(|(d, p)| self.diets.contains(d) && *p >= t.diet)
            .collect();

        QueryCategories {
            categories: categories
                .into_iter()
                .filter(|(_, p)| *p >= t.categories)
                .collect(),
            food,
            cuisines,
            diets,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const MODEL: &[u8] = include_bytes!("../tests/fixtures/categories_tiny.amcats");
    const FIXTURES: &str = include_str!("../tests/fixtures/categories_tiny_fixtures.json");

    #[test]
    fn test_poi_keys_sorted() {
        assert!(POI_KEYS.is_sorted());
        assert!(is_category_tag("natural", "peak"));
        assert!(is_category_tag("highway", "bus_stop"));
        assert!(!is_category_tag("highway", "residential"));
        assert!(!is_category_tag("building", "yes"));
    }

    #[test]
    fn test_mode_uses_most_specific_signal() {
        let understood = QueryCategories {
            categories: vec![("amenity=restaurant".into(), 0.9)],
            food: 1.0,
            cuisines: vec![("asian".into(), 0.5)],
            diets: vec![("vegan".into(), 1.0)],
        };
        assert!((understood.primary_confidence() - 0.5).abs() < f32::EPSILON);
        assert_eq!(understood.mode(0.4), CategoryMode::Include);
        assert_eq!(understood.mode(0.6), CategoryMode::Boost);
        assert_eq!(QueryCategories::default().mode(0.4), CategoryMode::Boost);
    }

    #[test]
    fn test_models_by_language_tag() {
        let mut models = CategoryModels::default();
        models.insert(CategoryModel::from_bytes(MODEL).unwrap());
        assert_eq!(models.languages(), vec!["deu"]);
        assert!(models.get("de-CH").is_some());
        assert!(models.get("deu").is_some());
        assert!(models.get("fr").is_none());
        assert!(models.get("not a language").is_none());
    }

    #[test]
    fn test_iso_639_3() {
        assert_eq!(iso_639_3("de"), Some("deu"));
        assert_eq!(iso_639_3("de-CH"), Some("deu"));
        assert_eq!(iso_639_3("zh-Hans"), Some("zho"));
        assert_eq!(iso_639_3("el"), Some("ell"));
        assert_eq!(iso_639_3("nb"), Some("nob"));
        assert_eq!(iso_639_3("vie"), Some("vie"));
        assert_eq!(iso_639_3("xx"), None);
        assert_eq!(iso_639_3(""), None);
    }

    #[test]
    fn test_split_values() {
        assert_eq!(
            split_values("Pizza; italian;;pizza"),
            vec!["italian", "pizza"]
        );
    }

    #[test]
    fn test_normalize() {
        assert_eq!(normalize("  Thai\tFOOD \n"), "thai food");
        assert_eq!(normalize("Café SPRÜNGLI"), "café sprüngli");
    }

    #[test]
    fn test_hash_matches_fasttext() {
        // fastText sign-extends bytes, so non-ASCII text hashes differently
        // from plain FNV-1a.
        assert_eq!(fasttext_hash(""), 2_166_136_261);
        assert_ne!(fasttext_hash("é"), {
            let mut h: u32 = 2_166_136_261;
            for &b in "é".as_bytes() {
                h ^= u32::from(b);
                h = h.wrapping_mul(16_777_619);
            }
            h
        });
    }

    /// Probabilities match fastText's own predictions on the same model, up to
    /// the error from 8-bit embeddings (and fastText's +1e-5 log offset).
    #[test]
    fn test_matches_fasttext_predictions() {
        assert_matches_fixtures(&CategoryModel::from_bytes(MODEL).unwrap(), FIXTURES);
    }

    /// The same check for a full-size model, e.g. after training one with
    /// `categories-train --fixtures`.
    #[test]
    #[ignore = "set AIRMAIL_TEST_CATEGORIES_MODEL and AIRMAIL_TEST_CATEGORIES_FIXTURES"]
    fn test_full_model_matches_fasttext_predictions() {
        let model = std::env::var("AIRMAIL_TEST_CATEGORIES_MODEL").unwrap();
        let fixtures = std::env::var("AIRMAIL_TEST_CATEGORIES_FIXTURES").unwrap();
        assert_matches_fixtures(
            &CategoryModel::open(Path::new(&model)).unwrap(),
            &std::fs::read_to_string(fixtures).unwrap(),
        );
    }

    fn assert_matches_fixtures(model: &CategoryModel, fixtures: &str) {
        let fixtures: Vec<serde_json::Value> = serde_json::from_str(fixtures).unwrap();
        for fixture in &fixtures {
            let query = fixture["query"].as_str().unwrap();
            let ours = model.head_probabilities(query);
            for head in ["categories", "cuisine", "diet"] {
                let ours: HashMap<&str, f32> =
                    ours[head].iter().map(|(l, p)| (l.as_str(), *p)).collect();
                for (label, expected) in fixture[head].as_object().unwrap() {
                    #[allow(clippy::cast_possible_truncation)]
                    let expected = expected.as_f64().unwrap() as f32;
                    let got = ours[label.as_str()];
                    assert!(
                        (got - expected).abs() < 0.02,
                        "{query:?} {head} {label}: got {got}, fastText {expected}"
                    );
                }
            }
        }
    }
}
