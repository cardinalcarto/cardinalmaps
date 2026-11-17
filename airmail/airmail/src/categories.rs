//! Categorical query understanding: which POI categories, cuisines and diets a
//! search query asks for.
//!
//! The model is trained by `airmail/categorical` (fastText, three heads) and
//! exported to a single file. Inference here reproduces fastText's supervised
//! prediction exactly, apart from the 8-bit quantization of input embeddings.
//! [`CategoryModel::pruned`] makes smaller models for phones.

use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// The file's JSON header.
///
/// Version 1 stores every input row. Version 2 adds `kept_rows` to a head:
/// its data then starts with a bitmap of which of the `words + bucket` rows
/// are stored (`ceil(rows / 64)` little-endian u64s, bit `i % 64` of word
/// `i / 64`), followed by only those rows. A row that isn't stored is a zero
/// vector: its feature still counts in the average, but adds nothing.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    /// ISO 639-3 code of the language the model understands.
    language: String,
    head_order: Vec<String>,
    heads: BTreeMap<String, HeadHeader>,
    food_categories: Vec<String>,
    diets: Vec<String>,
    thresholds: Thresholds,
}

#[derive(Debug, Serialize, Deserialize)]
struct HeadHeader {
    dim: usize,
    minn: usize,
    maxn: usize,
    bucket: usize,
    word_ngrams: usize,
    loss: Loss,
    words: Vec<String>,
    labels: Vec<String>,
    /// Version 2: how many input rows are stored, when not all of them are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kept_rows: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Loss {
    Softmax,
    Ova,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Thresholds {
    categories: f32,
    cuisine: f32,
    diet: f32,
}

/// Which input rows a pruned head stores: a bitmap over all `words + bucket`
/// rows, with the number of set bits before each 64-bit word.
#[derive(Clone)]
struct RowMap {
    bits: Vec<u64>,
    ranks: Vec<u32>,
}

impl RowMap {
    fn new(bits: Vec<u64>) -> Self {
        let mut ranks = Vec::with_capacity(bits.len());
        let mut total = 0u32;
        for word in &bits {
            ranks.push(total);
            total += word.count_ones();
        }
        Self { bits, ranks }
    }

    fn from_kept(kept: &[bool]) -> Self {
        let mut bits = vec![0u64; kept.len().div_ceil(64)];
        for (i, _) in kept.iter().enumerate().filter(|(_, k)| **k) {
            bits[i / 64] |= 1 << (i % 64);
        }
        Self::new(bits)
    }

    fn len(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// The stored row index of feature row `id`, if it is stored.
    fn get(&self, id: usize) -> Option<usize> {
        let word = self.bits[id / 64];
        let bit = 1u64 << (id % 64);
        (word & bit != 0)
            .then(|| self.ranks[id / 64] as usize + (word & (bit - 1)).count_ones() as usize)
    }
}

/// One fastText supervised model.
#[derive(Clone)]
struct Head {
    dim: usize,
    minn: usize,
    maxn: usize,
    bucket: usize,
    word_ngrams: usize,
    loss: Loss,
    words: Vec<String>,
    word_ids: HashMap<String, usize>,
    labels: Vec<String>,
    /// The stored rows, if not all `words + bucket` of them are.
    kept: Option<RowMap>,
    /// `stored rows x dim`, each row scaled by `scales`.
    input: Vec<i8>,
    scales: Vec<f32>,
    /// `labels x dim`.
    output: Vec<f32>,
}

/// How to shrink one head.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PruneHead {
    /// Store rows as label scores when there are fewer labels than
    /// dimensions. Exact apart from re-quantizing.
    pub fold: bool,
    /// Fraction of all `words + bucket` rows to keep, those that move label
    /// scores most; the rest become zero vectors. 1 keeps every row.
    pub keep: f64,
}

/// How to shrink a model; see [`CategoryModel::pruned`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prune {
    pub categories: PruneHead,
    pub cuisine: PruneHead,
    pub diet: PruneHead,
}

/// Symmetric 8-bit quantization of one row, as `categories-train` exports.
fn quantize_row(row: &[f32]) -> (Vec<i8>, f32) {
    let scale = row.iter().fold(0f32, |m, x| m.max(x.abs())) / 127.0;
    #[allow(clippy::cast_possible_truncation)]
    let q = row
        .iter()
        .map(|x| {
            if scale > 0.0 {
                (x / scale).round().clamp(-127.0, 127.0) as i8
            } else {
                0
            }
        })
        .collect();
    (q, scale)
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
        self.words.len()
    }

    /// All feature rows, stored or not.
    fn rows(&self) -> usize {
        self.words.len() + self.bucket
    }

    fn stored_row(&self, id: usize) -> Option<usize> {
        match &self.kept {
            None => Some(id),
            Some(kept) => kept.get(id),
        }
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
            let Some(id) = self.stored_row(id) else {
                continue;
            };
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

    /// Stored row `i`, dequantized.
    fn row(&self, i: usize) -> impl Iterator<Item = f32> + '_ {
        let scale = self.scales[i];
        self.input[i * self.dim..(i + 1) * self.dim]
            .iter()
            .map(move |&q| f32::from(q) * scale)
    }

    /// What stored row `i` adds to the label scores, `output · row`, centred
    /// for softmax (where adding the same amount to every score changes
    /// nothing).
    fn contribution(&self, i: usize) -> Vec<f32> {
        let row: Vec<f32> = self.row(i).collect();
        let mut scores: Vec<f32> = self
            .output
            .chunks_exact(self.dim)
            .map(|w| w.iter().zip(&row).map(|(a, b)| a * b).sum())
            .collect();
        if self.loss == Loss::Softmax {
            #[allow(clippy::cast_precision_loss)]
            let mean = scores.iter().sum::<f32>() / scores.len() as f32;
            scores.iter_mut().for_each(|s| *s -= mean);
        }
        scores
    }

    fn pruned(&self, prune: &PruneHead) -> Self {
        let mut head = self.clone();
        let stored = self.input.len() / self.dim;
        if prune.fold && self.labels.len() < self.dim {
            // Scores are linear in the hidden vector, so rows can live in
            // score space: `output · row` with an identity output matrix.
            let labels = self.labels.len();
            let mut input = Vec::with_capacity(stored * labels);
            let mut scales = Vec::with_capacity(stored);
            for i in 0..stored {
                let row: Vec<f32> = self.row(i).collect();
                let folded: Vec<f32> = self
                    .output
                    .chunks_exact(self.dim)
                    .map(|w| w.iter().zip(&row).map(|(a, b)| a * b).sum())
                    .collect();
                let (q, scale) = quantize_row(&folded);
                input.extend(q);
                scales.push(scale);
            }
            head.dim = labels;
            head.input = input;
            head.scales = scales;
            head.output = (0..labels * labels)
                .map(|i| if i / labels == i % labels { 1.0 } else { 0.0 })
                .collect();
        }
        if prune.keep >= 1.0 {
            return head;
        }

        let stored_ids: Vec<usize> = (0..self.rows())
            .filter(|&id| self.stored_row(id).is_some())
            .collect();
        // A row matters if it moves label scores and if queries use it. How
        // often is estimated from the model alone, so it isn't tied to any
        // one region: fastText sorts the dictionary by training count, so
        // word `r` gets a Zipf weight 1/(r + 1), shared by every row that
        // word alone looks up (itself, its n-grams, its bigram with `</s>`).
        // Rows no dictionary word uses (most word pairs) get the weight of
        // the rarest word.
        let mut usage = vec![0f64; self.rows()];
        for (rank, word) in self.words.iter().enumerate().skip(1) {
            #[allow(clippy::cast_precision_loss)]
            let weight = 1.0 / (rank + 1) as f64;
            for id in self.features(word) {
                usage[id] += weight;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let floor = 1.0 / self.words.len().max(1) as f64;
        let importance: Vec<f64> = (0..stored)
            .map(|i| {
                let norm = head
                    .contribution(i)
                    .iter()
                    .map(|c| f64::from(*c).powi(2))
                    .sum::<f64>()
                    .sqrt();
                (usage[stored_ids[i]] + floor).sqrt() * norm
            })
            .collect();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let k = ((prune.keep * self.rows() as f64).round() as usize).min(stored);
        let mut order: Vec<usize> = (0..stored).collect();
        order.sort_by(|&a, &b| importance[b].total_cmp(&importance[a]).then(a.cmp(&b)));
        let mut keep_stored = vec![false; stored];
        order[..k].iter().for_each(|&i| keep_stored[i] = true);

        let mut kept = vec![false; self.rows()];
        let (mut input, mut scales) = (Vec::new(), Vec::new());
        for (i, &id) in stored_ids.iter().enumerate() {
            if keep_stored[i] {
                kept[id] = true;
                input.extend_from_slice(&head.input[i * head.dim..(i + 1) * head.dim]);
                scales.push(head.scales[i]);
            }
        }
        head.kept = Some(RowMap::from_kept(&kept));
        head.input = input;
        head.scales = scales;
        head
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
        let mut header: Header = serde_json::from_slice(
            bytes
                .get(12..12 + header_len)
                .context("truncated model header")?,
        )?;
        ensure!(
            matches!(header.version, 1 | 2),
            "unsupported model version {}",
            header.version
        );

        let mut rest = &bytes[12 + header_len..];
        let mut heads = HashMap::new();
        for name in &header.head_order {
            let h = header
                .heads
                .remove(name)
                .with_context(|| format!("missing head {name}"))?;
            let rows = h.words.len() + h.bucket;
            let take = |rest: &mut &[u8], n: usize| -> Result<Vec<u8>> {
                ensure!(rest.len() >= n, "truncated model data");
                let (head, tail) = rest.split_at(n);
                *rest = tail;
                Ok(head.to_vec())
            };
            let kept = match h.kept_rows {
                None => None,
                Some(n) => {
                    ensure!(header.version >= 2, "kept_rows in a version 1 model");
                    let bits: Vec<u64> = take(&mut rest, rows.div_ceil(64) * 8)?
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|c| u64::from_le_bytes(*c))
                        .collect();
                    let kept = RowMap::new(bits);
                    ensure!(
                        kept.len() == n,
                        "{name}: bitmap has {} rows, header {n}",
                        kept.len()
                    );
                    Some(kept)
                }
            };
            let stored = h.kept_rows.unwrap_or(rows);
            #[allow(clippy::cast_possible_wrap)]
            let input = take(&mut rest, stored * h.dim)?
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
            let scales = floats(take(&mut rest, stored * 4)?);
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
                    words: h.words,
                    labels: h.labels,
                    kept,
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

    fn heads(&self) -> [(&'static str, &Head); 3] {
        [
            ("categories", &self.categories),
            ("cuisine", &self.cuisine),
            ("diet", &self.diet),
        ]
    }

    /// The model as a file `from_bytes` reads: version 1 if every head stores
    /// all its rows, so older readers can still load it, else version 2.
    ///
    /// # Panics
    ///
    /// If the header is over 4 GB.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let pruned = self.heads().iter().any(|(_, h)| h.kept.is_some());
        let header = Header {
            version: if pruned { 2 } else { 1 },
            language: self.language.clone(),
            head_order: self.heads().iter().map(|(n, _)| (*n).to_string()).collect(),
            heads: self
                .heads()
                .iter()
                .map(|(name, h)| {
                    let header = HeadHeader {
                        dim: h.dim,
                        minn: h.minn,
                        maxn: h.maxn,
                        bucket: h.bucket,
                        word_ngrams: h.word_ngrams,
                        loss: h.loss,
                        words: h.words.clone(),
                        labels: h.labels.clone(),
                        kept_rows: h.kept.as_ref().map(RowMap::len),
                    };
                    ((*name).to_string(), header)
                })
                .collect(),
            food_categories: self.food_categories.iter().cloned().collect(),
            diets: self.diets.clone(),
            thresholds: self.thresholds,
        };
        let json = serde_json::to_vec(&header).expect("header serializes");
        let mut out = MAGIC.to_vec();
        out.extend(
            u32::try_from(json.len())
                .expect("header fits")
                .to_le_bytes(),
        );
        out.extend(json);
        for (_, h) in self.heads() {
            if let Some(kept) = &h.kept {
                kept.bits.iter().for_each(|w| out.extend(w.to_le_bytes()));
            }
            #[allow(clippy::cast_sign_loss)]
            out.extend(h.input.iter().map(|&q| q as u8));
            h.scales.iter().for_each(|s| out.extend(s.to_le_bytes()));
            h.output.iter().for_each(|w| out.extend(w.to_le_bytes()));
        }
        out
    }

    /// A smaller model for devices; see [`Prune`].
    #[must_use]
    pub fn pruned(&self, prune: &Prune) -> Self {
        Self {
            language: self.language.clone(),
            categories: self.categories.pruned(&prune.categories),
            cuisine: self.cuisine.pruned(&prune.cuisine),
            diet: self.diet.pruned(&prune.diet),
            food_categories: self.food_categories.clone(),
            diets: self.diets.clone(),
            thresholds: self.thresholds,
        }
    }

    /// Sizes of each head: (name, stored rows, all rows, dim).
    #[must_use]
    pub fn head_sizes(&self) -> Vec<(&'static str, usize, usize, usize)> {
        self.heads()
            .iter()
            .map(|(n, h)| (*n, h.input.len() / h.dim, h.rows(), h.dim))
            .collect()
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
                // What a building is built as isn't what's in it.
                .filter(|(c, p)| *p >= t.categories && !c.starts_with("building"))
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

    fn prune(fold: bool, keep: f64) -> Prune {
        let head = PruneHead { fold, keep };
        Prune {
            categories: head,
            cuisine: head,
            diet: head,
        }
    }

    #[test]
    fn test_unpruned_round_trip_is_version_1() {
        let model = CategoryModel::from_bytes(MODEL).unwrap();
        let bytes = model.pruned(&prune(false, 1.0)).to_bytes();
        assert!(String::from_utf8_lossy(&bytes).contains("\"version\":1"));
        let again = CategoryModel::from_bytes(&bytes).unwrap();
        for query in ["restaurant", "vegan pizza", "xyz"] {
            assert_eq!(
                model.head_probabilities(query),
                again.head_probabilities(query)
            );
        }
    }

    #[test]
    fn test_folding_keeps_predictions() {
        let model = CategoryModel::from_bytes(MODEL).unwrap();
        let folded =
            CategoryModel::from_bytes(&model.pruned(&prune(true, 1.0)).to_bytes()).unwrap();
        // The tiny model's diet head has fewer labels than dimensions.
        assert_eq!(folded.diet.dim, folded.diet.labels.len());
        assert_matches_fixtures(&folded, FIXTURES);
    }

    #[test]
    fn test_pruned_round_trip() {
        let model = CategoryModel::from_bytes(MODEL).unwrap();
        let pruned = model.pruned(&prune(true, 0.5));
        let bytes = pruned.to_bytes();
        let again = CategoryModel::from_bytes(&bytes).unwrap();
        assert_eq!(again.to_bytes(), bytes);
        for (_, stored, rows, _) in again.head_sizes() {
            assert_eq!(stored, (rows as f64 * 0.5).round() as usize);
        }
        for query in ["restaurant", "vegan pizza", "Bäckerei"] {
            assert_eq!(
                pruned.head_probabilities(query),
                again.head_probabilities(query)
            );
        }
        // Pruning a pruned model keeps a subset of its rows.
        let smaller = again.pruned(&prune(true, 0.25));
        for (_, stored, rows, _) in smaller.head_sizes() {
            assert_eq!(stored, (rows as f64 * 0.25).round() as usize);
        }
    }

    #[test]
    fn test_row_map() {
        let kept: Vec<bool> = (0..200).map(|i| i % 3 == 0).collect();
        let map = RowMap::from_kept(&kept);
        assert_eq!(map.len(), 67);
        for (i, k) in kept.iter().enumerate() {
            assert_eq!(map.get(i), k.then_some(i / 3));
        }
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
