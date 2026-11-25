//! A trained model, for tagging.
//!
//! Two formats are read. The plain one (`airmtag3`, as training writes it,
//! and the older `airmtag2` and `airmtag1`) has, for each feature set, an
//! FST from feature to row number, each row's length, its one-byte columns
//! and 16-bit (bfloat16) weights: about 3 bytes a weight.
//!
//! The packed one (`airmtag4`, which `Model::packed` makes, for phones)
//! keeps each row's columns as a 16-bit bitmap where there are at most 16
//! columns (the labels), in place of a length and a byte a column, and its
//! weights as signed bytes times a power of two each row has (or bfloat16
//! still). Where each row's weights start is counted when it's
//! read, from every 64th row's. Its vocabulary keeps only whether each word
//! is rare, which is all the features ask of it.

use std::io::{self, Read, Write};

use fst::Streamer;
use fst::{Map, MapBuilder};

use crate::crf::{Label, Lattice, TokenFeatures, Weights};
use crate::features::{MIN_WORD_COUNT, RARE_WORD_COUNT};

/// A weight as bfloat16: the top half of an f32, rounded to nearest even.
/// Its 8 bits of mantissa keep weights to within 0.4%.
fn to_bf16(value: f32) -> u16 {
    let bits = value.to_bits();
    ((bits + 0x7FFF + ((bits >> 16) & 1)) >> 16) as u16
}

fn from_bf16(value: u16) -> f32 {
    f32::from_bits(u32::from(value) << 16)
}

/// A feature and its nonzero weights by column.
pub type Row = (String, Vec<(u32, f32)>);

/// Sparse rows of weights, by feature, as training writes them.
#[derive(Clone)]
struct Plain {
    /// Feature to row number.
    keys: Map<Vec<u8>>,
    /// Row `r` is `columns[starts[r]..starts[r + 1]]`.
    starts: Vec<u32>,
    columns: Vec<u8>,
    values: Vec<u16>,
}

impl Plain {
    fn new(mut rows: Vec<Row>) -> Self {
        sort_rows(&mut rows);
        let mut builder = MapBuilder::memory();
        let (mut starts, mut columns, mut values) = (vec![0], Vec::new(), Vec::new());
        for (row, (feature, weights)) in rows.iter().enumerate() {
            builder
                .insert(feature, row as u64)
                .expect("sorted, distinct features");
            for (column, value) in weights {
                columns.push(u8::try_from(*column).expect("columns fit in a byte"));
                values.push(to_bf16(*value));
            }
            starts.push(columns.len() as u32);
        }
        let keys = Map::new(builder.into_inner().expect("an FST in memory")).expect("a valid FST");
        Self {
            keys,
            starts,
            columns,
            values,
        }
    }

    fn write(&self, out: &mut impl Write) -> io::Result<()> {
        write_bytes(out, self.keys.as_fst().as_bytes())?;
        write_u32(out, self.starts.len() as u32 - 1)?;
        for row in self.starts.windows(2) {
            out.write_all(&[u8::try_from(row[1] - row[0]).expect("rows fit in a byte")])?;
        }
        out.write_all(&self.columns)?;
        for value in &self.values {
            out.write_all(&value.to_le_bytes())?;
        }
        Ok(())
    }

    fn read(input: &mut impl Read) -> io::Result<Self> {
        let keys = Map::new(read_bytes(input)?).map_err(invalid)?;
        let n = read_u32(input)? as usize;
        let mut lengths = vec![0; n];
        input.read_exact(&mut lengths)?;
        let mut starts = Vec::with_capacity(n + 1);
        starts.push(0);
        for length in lengths {
            starts.push(starts[starts.len() - 1] + u32::from(length));
        }
        let total = starts[n] as usize;
        let mut columns = vec![0; total];
        input.read_exact(&mut columns)?;
        let mut bytes = vec![0; total * 2];
        input.read_exact(&mut bytes)?;
        let values = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        Ok(Self {
            keys,
            starts,
            columns,
            values,
        })
    }

    /// Rows as written by the first format: `u32` columns and `f32` weights.
    fn read_v1(input: &mut impl Read) -> io::Result<Self> {
        let n = read_u32(input)?;
        let mut rows = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let feature = read_str(input)?;
            let len = read_u32(input)?;
            let mut weights = Vec::with_capacity(len as usize);
            for _ in 0..len {
                let column = read_u32(input)?;
                weights.push((column, f32::from_le_bytes(read_array(input)?)));
            }
            rows.push((feature, weights));
        }
        Ok(Plain::new(rows))
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.keys.len());
        let mut stream = self.keys.stream();
        while let Some((key, row)) = stream.next() {
            let (start, end) = (
                self.starts[row as usize] as usize,
                self.starts[row as usize + 1] as usize,
            );
            let weights = self.columns[start..end]
                .iter()
                .zip(&self.values[start..end])
                .map(|(c, v)| (u32::from(*c), from_bf16(*v)))
                .collect();
            rows.push((String::from_utf8_lossy(key).into_owned(), weights));
        }
        rows
    }

    fn add(&self, feature: &str, scale: f64, scores: &mut [f64]) {
        if let Some(row) = self.keys.get(feature) {
            let (start, end) = (
                self.starts[row as usize] as usize,
                self.starts[row as usize + 1] as usize,
            );
            for (column, value) in self.columns[start..end]
                .iter()
                .zip(&self.values[start..end])
            {
                scores[*column as usize] += scale * f64::from(from_bf16(*value));
            }
        }
    }
}

fn sort_rows(rows: &mut Vec<Row>) {
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.dedup_by(|a, b| a.0 == b.0);
}

/// How a packed model stores its weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bits {
    /// bfloat16, as the plain format.
    Sixteen,
    /// A signed byte, times a power of two each row has: within 1/127 of
    /// the row's largest weight. (A table of 256 values fitted to all the
    /// weights, and a logarithmic one, both lost a point of accuracy on
    /// held-out queries; this loses none.)
    Eight,
}

impl Bits {
    fn number(self) -> u8 {
        match self {
            Bits::Sixteen => 16,
            Bits::Eight => 8,
        }
    }

    /// From a number of bits: 16 or 8.
    #[must_use]
    pub fn of(bits: u8) -> Option<Self> {
        match bits {
            16 => Some(Bits::Sixteen),
            8 => Some(Bits::Eight),
            _ => None,
        }
    }
}

/// The most columns a row's columns are kept as a bitmap for.
const BITMAP_COLUMNS: usize = 16;

/// Rows between the weight offsets kept in memory: a lookup counts the
/// weights of at most this many rows before its own.
const SAMPLE_ROWS: usize = 64;

/// Sparse rows of weights, by feature, packed (see the module's docs).
#[derive(Clone)]
struct Packed {
    /// Feature to row number.
    keys: Map<Vec<u8>>,
    /// Each row's columns as a bitmap, two bytes little-endian a row, for at
    /// most 16 columns; else each row's length, a byte a row.
    bitmap: bool,
    shapes: Vec<u8>,
    /// Each weight's column, without `bitmap`.
    columns: Vec<u8>,
    bits: Bits,
    /// For `Eight`, each row's power of two, plus 128.
    scales: Vec<u8>,
    /// Each weight: a signed byte, or a bfloat16, little-endian.
    codes: Vec<u8>,
    /// Where the weights of every `SAMPLE_ROWS`th row start; not stored.
    samples: Vec<u32>,
}

impl Packed {
    fn new(mut rows: Vec<Row>, width: usize, bits: Bits) -> Self {
        sort_rows(&mut rows);
        let bitmap = width <= BITMAP_COLUMNS;
        let mut builder = MapBuilder::memory();
        let (mut shapes, mut columns, mut codes, mut scales) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (row, (feature, weights)) in rows.iter_mut().enumerate() {
            weights.sort_by_key(|(c, _)| *c);
            weights.dedup_by_key(|(c, _)| *c);
            builder
                .insert(&feature, row as u64)
                .expect("sorted, distinct features");
            if bitmap {
                let map = weights.iter().fold(0u16, |m, (c, _)| m | (1 << c));
                shapes.extend(map.to_le_bytes());
            } else {
                shapes.push(u8::try_from(weights.len()).expect("rows fit in a byte"));
                columns.extend(
                    weights
                        .iter()
                        .map(|(c, _)| u8::try_from(*c).expect("columns fit in a byte")),
                );
            }
            let exponent = if bits == Bits::Eight {
                let max = weights.iter().fold(0f32, |m, (_, w)| m.max(w.abs()));
                let e = if max > 0.0 {
                    (max / 127.0).log2().ceil().clamp(-128.0, 127.0) as i32
                } else {
                    0
                };
                scales.push(u8::try_from(e + 128).expect("an exponent in range"));
                e
            } else {
                0
            };
            for (_, w) in weights.iter() {
                match bits {
                    Bits::Eight => {
                        let q = (w / 2f32.powi(exponent)).round().clamp(-127.0, 127.0) as i8;
                        codes.push(q.to_le_bytes()[0]);
                    }
                    Bits::Sixteen => codes.extend(to_bf16(*w).to_le_bytes()),
                }
            }
        }
        let keys = Map::new(builder.into_inner().expect("an FST in memory")).expect("a valid FST");
        let mut packed = Self {
            keys,
            bitmap,
            shapes,
            columns,
            bits,
            scales,
            codes,
            samples: Vec::new(),
        };
        packed.samples = packed.sample().0;
        packed
    }

    fn rows_len(&self) -> usize {
        if self.bitmap {
            self.shapes.len() / 2
        } else {
            self.shapes.len()
        }
    }

    /// How many weights row `r` has.
    fn len_of(&self, r: usize) -> usize {
        if self.bitmap {
            self.bitmap_of(r).count_ones() as usize
        } else {
            usize::from(self.shapes[r])
        }
    }

    fn bitmap_of(&self, r: usize) -> u16 {
        u16::from_le_bytes([self.shapes[2 * r], self.shapes[2 * r + 1]])
    }

    /// Every `SAMPLE_ROWS`th row's start, and how many weights there are.
    fn sample(&self) -> (Vec<u32>, usize) {
        let mut samples = Vec::with_capacity(self.rows_len() / SAMPLE_ROWS + 1);
        let mut at = 0usize;
        for r in 0..self.rows_len() {
            if r % SAMPLE_ROWS == 0 {
                samples.push(u32::try_from(at).expect("fewer than 2^32 weights"));
            }
            at += self.len_of(r);
        }
        (samples, at)
    }

    fn write(&self, out: &mut impl Write) -> io::Result<()> {
        out.write_all(&[u8::from(self.bitmap), self.bits.number()])?;
        write_bytes(out, self.keys.as_fst().as_bytes())?;
        write_bytes(out, &self.shapes)?;
        write_bytes(out, &self.columns)?;
        write_bytes(out, &self.scales)?;
        write_bytes(out, &self.codes)
    }

    /// Reads rows of `width` columns.
    fn read(input: &mut impl Read, width: usize) -> io::Result<Self> {
        let [bitmap, bits]: [u8; 2] = read_array(input)?;
        let bits = Bits::of(bits)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bits a weight"))?;
        let keys = Map::new(read_bytes(input)?).map_err(invalid)?;
        let shapes = read_bytes(input)?;
        let columns = read_bytes(input)?;
        let scales = read_bytes(input)?;
        let codes = read_bytes(input)?;
        let mut packed = Self {
            keys,
            bitmap: bitmap != 0,
            shapes,
            columns,
            bits,
            scales,
            codes,
            samples: Vec::new(),
        };
        packed.check(width)?;
        packed.samples = packed.sample().0;
        Ok(packed)
    }

    /// Whether there's a shape for every row, a column and code for every
    /// weight, every column is one of `width` and every code has a value,
    /// so lookups can't go out of bounds.
    fn check(&self, width: usize) -> io::Result<()> {
        let bad = |what: &str| {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("packed rows: {what}"),
            ))
        };
        if self.bitmap != (width <= BITMAP_COLUMNS) {
            return bad("column layout");
        }
        let rows = self.keys.len();
        if self.shapes.len() != if self.bitmap { 2 * rows } else { rows } {
            return bad("shapes missing");
        }
        let mut stream = self.keys.stream();
        while let Some((_, row)) = stream.next() {
            if row as usize >= rows {
                return bad("rows out of range");
            }
        }
        if self.bitmap && (0..rows).any(|r| usize::from(self.bitmap_of(r)) >> width != 0) {
            return bad("columns out of range");
        }
        let (_, weights) = self.sample();
        let bytes = match self.bits {
            Bits::Sixteen => weights * 2,
            Bits::Eight => weights,
        };
        if self.scales.len() != if self.bits == Bits::Eight { rows } else { 0 } {
            return bad("scales missing");
        }
        if bytes != self.codes.len()
            || (!self.bitmap && weights != self.columns.len())
            || (self.bitmap && !self.columns.is_empty())
        {
            return bad("weights missing");
        }
        if self.columns.iter().any(|c| usize::from(*c) >= width) {
            return bad("columns out of range");
        }
        Ok(())
    }

    /// Where row `r`'s weights start.
    fn start_of(&self, r: usize) -> usize {
        let base = r / SAMPLE_ROWS * SAMPLE_ROWS;
        self.samples[r / SAMPLE_ROWS] as usize + (base..r).map(|q| self.len_of(q)).sum::<usize>()
    }

    /// Weight `i`, of row `r`.
    fn value(&self, i: usize, r: usize) -> f32 {
        match self.bits {
            Bits::Eight => {
                f32::from(i8::from_le_bytes([self.codes[i]]))
                    * 2f32.powi(i32::from(self.scales[r]) - 128)
            }
            Bits::Sixteen => from_bf16(u16::from_le_bytes([
                self.codes[2 * i],
                self.codes[2 * i + 1],
            ])),
        }
    }

    /// Row `r`'s columns and weights.
    fn each(&self, r: usize, mut f: impl FnMut(usize, f32)) {
        let start = self.start_of(r);
        if self.bitmap {
            let mut map = self.bitmap_of(r);
            let mut i = start;
            while map != 0 {
                f(map.trailing_zeros() as usize, self.value(i, r));
                map &= map - 1;
                i += 1;
            }
        } else {
            for i in start..start + self.len_of(r) {
                f(usize::from(self.columns[i]), self.value(i, r));
            }
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.keys.len());
        let mut stream = self.keys.stream();
        while let Some((key, r)) = stream.next() {
            let mut weights = Vec::new();
            self.each(r as usize, |c, w| weights.push((c as u32, w)));
            rows.push((String::from_utf8_lossy(key).into_owned(), weights));
        }
        rows
    }

    fn add(&self, feature: &str, scale: f64, scores: &mut [f64]) {
        if let Some(r) = self.keys.get(feature) {
            self.each(r as usize, |c, w| scores[c] += scale * f64::from(w));
        }
    }
}

#[derive(Clone)]
enum Rows {
    Plain(Plain),
    Packed(Packed),
}

impl std::fmt::Debug for Rows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rows")
            .field("features", &self.len())
            .finish()
    }
}

impl Rows {
    fn len(&self) -> usize {
        match self {
            Rows::Plain(rows) => rows.keys.len(),
            Rows::Packed(rows) => rows.keys.len(),
        }
    }

    fn rows(&self) -> Vec<Row> {
        match self {
            Rows::Plain(rows) => rows.rows(),
            Rows::Packed(rows) => rows.rows(),
        }
    }

    fn add(&self, feature: &str, scale: f64, scores: &mut [f64]) {
        match self {
            Rows::Plain(rows) => rows.add(feature, scale, scores),
            Rows::Packed(rows) => rows.add(feature, scale, scores),
        }
    }

    /// Bytes of feature strings (the FST), of columns and of weights.
    fn sizes(&self) -> (usize, usize, usize) {
        match self {
            Rows::Plain(rows) => (
                rows.keys.as_fst().as_bytes().len(),
                rows.starts.len() - 1 + rows.columns.len(),
                rows.values.len() * 2,
            ),
            Rows::Packed(rows) => (
                rows.keys.as_fst().as_bytes().len(),
                rows.shapes.len() + rows.columns.len(),
                rows.codes.len() + rows.scales.len(),
            ),
        }
    }
}

/// The rows of `rows` with their weights of magnitude at least `min_weight`,
/// without the rows left with none.
fn without_small(rows: Vec<Row>, min_weight: f32) -> Vec<Row> {
    rows.into_iter()
        .filter_map(|(feature, weights)| {
            let weights: Vec<(u32, f32)> = weights
                .into_iter()
                .filter(|(_, w)| w.abs() >= min_weight)
                .collect();
            (!weights.is_empty()).then_some((feature, weights))
        })
        .collect()
}

/// A linear-chain CRF's weights: see `crf`.
#[derive(Debug, Clone)]
pub struct Model {
    labels: Vec<String>,
    state: Rows,
    state_trans: Rows,
    /// `[i * L + j]`.
    trans: Vec<f32>,
    /// What every score is multiplied by, for calibrated probabilities (see
    /// `crf::fit_scale`).
    scale: f64,
}

impl Model {
    /// A model from each feature's nonzero weights by column: label for
    /// state features, `prev * L + cur` for state-transition features.
    #[must_use]
    pub fn from_rows(
        labels: Vec<String>,
        state: Vec<Row>,
        state_trans: Vec<Row>,
        trans: Vec<f32>,
    ) -> Self {
        Self {
            labels,
            state: Rows::Plain(Plain::new(state)),
            state_trans: Rows::Plain(Plain::new(state_trans)),
            trans,
            scale: 1.0,
        }
    }

    /// The model without its weights smaller in magnitude than
    /// `min_weight`, nor the features left without weights: smaller, as
    /// nearly as accurate as the weights dropped are small.
    #[must_use]
    pub fn pruned(&self, min_weight: f32) -> Self {
        let (state, state_trans) = self.rows();
        self.with_rows(
            without_small(state, min_weight),
            without_small(state_trans, min_weight),
        )
    }

    /// The model with other rows of weights, in the same format.
    #[must_use]
    pub fn with_rows(&self, state: Vec<Row>, state_trans: Vec<Row>) -> Self {
        let (state, state_trans) = match (&self.state, &self.state_trans) {
            (Rows::Packed(s), Rows::Packed(t)) => (
                Rows::Packed(Packed::new(state, self.labels.len(), s.bits)),
                Rows::Packed(Packed::new(state_trans, self.labels.len().pow(2), t.bits)),
            ),
            _ => (
                Rows::Plain(Plain::new(state)),
                Rows::Plain(Plain::new(state_trans)),
            ),
        };
        Self {
            labels: self.labels.clone(),
            state,
            state_trans,
            trans: self.trans.clone(),
            scale: self.scale,
        }
    }

    /// The model in the packed format, its state weights stored in `state`
    /// bits and its state-transition weights in `transition` bits.
    #[must_use]
    pub fn packed(&self, state: Bits, transition: Bits) -> Self {
        let (state_rows, transition_rows) = self.rows();
        let l = self.labels.len();
        Self {
            labels: self.labels.clone(),
            state: Rows::Packed(Packed::new(state_rows, l, state)),
            state_trans: Rows::Packed(Packed::new(transition_rows, l * l, transition)),
            trans: self.trans.clone(),
            scale: self.scale,
        }
    }

    /// Whether it's in the packed format.
    #[must_use]
    pub fn is_packed(&self) -> bool {
        matches!(self.state, Rows::Packed(_))
    }

    /// Every state and state-transition feature with its weights, by
    /// feature.
    #[must_use]
    pub fn rows(&self) -> (Vec<Row>, Vec<Row>) {
        (self.state.rows(), self.state_trans.rows())
    }

    /// The bytes of the state and state-transition features' strings,
    /// columns and weights, as stored.
    #[must_use]
    pub fn sizes(&self) -> [(usize, usize, usize); 2] {
        [self.state.sizes(), self.state_trans.sizes()]
    }

    /// The magnitude of every state and state-transition weight, unsorted.
    #[must_use]
    pub fn weight_magnitudes(&self) -> Vec<f32> {
        let (state, state_trans) = self.rows();
        state
            .iter()
            .chain(&state_trans)
            .flat_map(|(_, w)| w.iter().map(|(_, v)| v.abs()))
            .collect()
    }

    /// What every score is multiplied by.
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
    }

    #[must_use]
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// How many state and state-transition features it has.
    #[must_use]
    pub fn num_features(&self) -> (usize, usize) {
        (self.state.len(), self.state_trans.len())
    }

    /// The best labels for the tokens, and their score.
    #[must_use]
    pub fn tag(&self, tokens: &[TokenFeatures]) -> (Vec<Label>, f64) {
        Lattice::new(self, tokens).viterbi()
    }

    /// The `k` most likely labellings, with their probabilities.
    #[must_use]
    pub fn best(&self, tokens: &[TokenFeatures], k: usize) -> Vec<(Vec<Label>, f64)> {
        Lattice::new(self, tokens).best_paths(k)
    }

    /// Each token's probability of each label.
    #[must_use]
    pub fn marginals(&self, tokens: &[TokenFeatures]) -> Vec<Vec<f64>> {
        Lattice::new(self, tokens).marginals()
    }

    /// Writes the model, with `extra` (such as a vocabulary) after it. A
    /// packed model keeps of each extra value only whether it's rare (see
    /// `Model::read`).
    ///
    /// # Errors
    /// If writing fails.
    pub fn write(&self, out: &mut impl Write, extra: &[(String, u32)]) -> io::Result<()> {
        let packed = self.is_packed();
        out.write_all(if packed { MAGIC_V4 } else { MAGIC })?;
        write_u32(out, self.labels.len() as u32)?;
        for label in &self.labels {
            write_str(out, label)?;
        }
        for rows in [&self.state, &self.state_trans] {
            match rows {
                Rows::Plain(rows) if !packed => rows.write(out)?,
                Rows::Packed(rows) if packed => rows.write(out)?,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "feature sets in different formats",
                    ));
                }
            }
        }
        for value in &self.trans {
            out.write_all(&value.to_le_bytes())?;
        }
        out.write_all(&self.scale.to_le_bytes())?;
        let mut extra: Vec<&(String, u32)> = extra.iter().collect();
        extra.sort();
        extra.dedup_by(|a, b| a.0 == b.0);
        let mut builder = MapBuilder::memory();
        for (key, value) in extra {
            let value = if packed {
                u64::from(*value > RARE_WORD_COUNT)
            } else {
                u64::from(*value)
            };
            builder.insert(key, value).map_err(invalid)?;
        }
        write_bytes(out, &builder.into_inner().map_err(invalid)?)
    }

    /// Reads a model written by `write` (or by an older format), and its
    /// extra. A packed model's extra values are `MIN_WORD_COUNT` for words
    /// that were rare and one more than `RARE_WORD_COUNT` for those that
    /// weren't.
    ///
    /// # Errors
    /// If it isn't one.
    pub fn read(input: &mut impl Read) -> io::Result<(Self, Vec<(String, u32)>)> {
        let magic: [u8; 8] = read_array(input)?;
        let v1 = &magic == MAGIC_V1;
        let v2 = &magic == MAGIC_V2;
        let v4 = &magic == MAGIC_V4;
        if !v1 && !v2 && !v4 && &magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a tagger model",
            ));
        }
        let n = read_u32(input)? as usize;
        let labels = (0..n)
            .map(|_| read_str(input))
            .collect::<io::Result<Vec<_>>>()?;
        let (state, state_trans) = if v4 {
            (
                Rows::Packed(Packed::read(input, n)?),
                Rows::Packed(Packed::read(input, n * n)?),
            )
        } else {
            let read_rows = if v1 { Plain::read_v1 } else { Plain::read };
            (
                Rows::Plain(read_rows(input)?),
                Rows::Plain(read_rows(input)?),
            )
        };
        let trans = (0..n * n)
            .map(|_| Ok(f32::from_le_bytes(read_array(input)?)))
            .collect::<io::Result<Vec<_>>>()?;
        let scale = if v1 || v2 {
            1.0
        } else {
            f64::from_le_bytes(read_array(input)?)
        };
        let extra = if v1 {
            let m = read_u32(input)?;
            (0..m)
                .map(|_| Ok((read_str(input)?, read_u32(input)?)))
                .collect::<io::Result<Vec<_>>>()?
        } else {
            let map = Map::new(read_bytes(input)?).map_err(invalid)?;
            let mut extra = Vec::with_capacity(map.len());
            let mut stream = fst::IntoStreamer::into_stream(&map);
            while let Some((key, value)) = fst::Streamer::next(&mut stream) {
                let value = if v4 {
                    if value == 0 {
                        MIN_WORD_COUNT
                    } else {
                        RARE_WORD_COUNT + 1
                    }
                } else {
                    value as u32
                };
                extra.push((String::from_utf8(key.to_vec()).map_err(invalid)?, value));
            }
            extra
        };
        Ok((
            Self {
                labels,
                state,
                state_trans,
                trans,
                scale,
            },
            extra,
        ))
    }
}

const MAGIC_V4: &[u8; 8] = b"airmtag4";
const MAGIC: &[u8; 8] = b"airmtag3";
const MAGIC_V2: &[u8; 8] = b"airmtag2";
const MAGIC_V1: &[u8; 8] = b"airmtag1";

fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn write_u32(out: &mut impl Write, value: u32) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

fn read_array<const N: usize>(input: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    Ok(u32::from_le_bytes(read_array(input)?))
}

fn write_bytes(out: &mut impl Write, value: &[u8]) -> io::Result<()> {
    out.write_all(&(value.len() as u64).to_le_bytes())?;
    out.write_all(value)
}

fn read_bytes(input: &mut impl Read) -> io::Result<Vec<u8>> {
    let len = u64::from_le_bytes(read_array(input)?) as usize;
    let mut bytes = Vec::new();
    // Not `vec![0; len]`: a bad length shouldn't allocate before failing.
    input.take(len as u64).read_to_end(&mut bytes)?;
    if bytes.len() != len {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated"));
    }
    Ok(bytes)
}

fn write_str(out: &mut impl Write, value: &str) -> io::Result<()> {
    write_u32(out, value.len() as u32)?;
    out.write_all(value.as_bytes())
}

fn read_str(input: &mut impl Read) -> io::Result<String> {
    let len = read_u32(input)? as usize;
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(invalid)
}

#[cfg(test)]
mod test {
    use super::*;

    fn model() -> Model {
        Model::from_rows(
            vec!["a".into(), "b".into()],
            vec![
                ("word|x".into(), vec![(0, 1.5), (1, -0.5)]),
                ("bias".into(), vec![(1, 0.25)]),
            ],
            vec![("word|y".into(), vec![(3, 2.0)])],
            vec![0.1, 0.2, 0.3, 0.4],
        )
    }

    #[test]
    fn models_round_trip() {
        let model = model();
        let mut bytes = Vec::new();
        model.write(&mut bytes, &[("seattle".into(), 12)]).unwrap();
        let (mut read, extra) = Model::read(&mut bytes.as_slice()).unwrap();
        assert_eq!(extra, vec![("seattle".to_string(), 12)]);
        let mut scores = [0.0; 2];
        read.add_state("word|x", &mut scores);
        assert_eq!(scores, [1.5, -0.5]);
        let mut pair = [0.0; 4];
        read.add_state_trans("word|y", &mut pair);
        assert_eq!(pair, [0.0, 0.0, 0.0, 2.0]);
        assert_eq!(read.trans(1, 0), f64::from(0.3f32));
        read.set_scale(0.5);
        let mut bytes = Vec::new();
        read.write(&mut bytes, &[]).unwrap();
        let (read, _) = Model::read(&mut bytes.as_slice()).unwrap();
        let mut scores = [0.0; 2];
        read.add_state("word|x", &mut scores);
        assert_eq!(scores, [0.75, -0.25]);
    }

    #[test]
    fn weights_keep_three_significant_digits() {
        for value in [0.0f32, 1.0, -0.5, 0.123_456, 3.141_59, -203.15, 1e-3] {
            let back = from_bf16(to_bf16(value));
            assert!(
                (back - value).abs() <= value.abs() / 256.0,
                "{value} came back {back}"
            );
        }
    }

    #[test]
    fn packed_models_round_trip_with_the_same_weights() {
        let mut model = model();
        model.set_scale(0.5);
        let extra = [
            ("seattle".to_string(), 12),
            ("main".to_string(), 51),
            ("rare".to_string(), 50),
        ];
        for bits in [Bits::Sixteen, Bits::Eight] {
            let packed = model.packed(bits, bits);
            let mut bytes = Vec::new();
            packed.write(&mut bytes, &extra).unwrap();
            assert_eq!(&bytes[..8], MAGIC_V4);
            let (read, extra) = Model::read(&mut bytes.as_slice()).unwrap();
            assert!(read.is_packed());
            // Only whether a word is rare is kept.
            assert_eq!(
                extra,
                vec![
                    ("main".to_string(), RARE_WORD_COUNT + 1),
                    ("rare".to_string(), MIN_WORD_COUNT),
                    ("seattle".to_string(), MIN_WORD_COUNT)
                ]
            );
            // These weights are exact in a byte times a power of two.
            assert_eq!(read.rows(), model.rows(), "{bits:?}");
            let mut scores = [0.0; 2];
            read.add_state("word|x", &mut scores);
            assert_eq!(scores, [0.75, -0.25]);
            let mut pair = [0.0; 4];
            read.add_state_trans("word|y", &mut pair);
            assert_eq!(pair, [0.0, 0.0, 0.0, 1.0]);
            read.add_state_trans("word|nothing", &mut pair);
            assert_eq!(pair, [0.0, 0.0, 0.0, 1.0]);
            assert_eq!(read.trans(1, 0), 0.5 * f64::from(0.3f32));
            // Pruning keeps the format.
            assert!(read.pruned(1.0).is_packed());
            assert_eq!(read.pruned(1.0).num_features(), (1, 1));
        }
    }

    #[test]
    fn packed_transition_rows_keep_their_columns() {
        // More than 16 labels, so pairs, as columns are bytes.
        let labels: Vec<String> = (0..12).map(|i| i.to_string()).collect();
        let pairs: Vec<(u32, f32)> = (0..144)
            .step_by(7)
            .map(|c| (c, c as f32 / 10.0 - 7.0))
            .collect();
        let model = Model::from_rows(
            labels,
            vec![("bias".into(), vec![(11, 1.0)])],
            vec![("word|y".into(), pairs)],
            vec![0.0; 144],
        );
        let packed = model.packed(Bits::Sixteen, Bits::Sixteen);
        let mut bytes = Vec::new();
        packed.write(&mut bytes, &[]).unwrap();
        let (read, _) = Model::read(&mut bytes.as_slice()).unwrap();
        assert_eq!(read.rows(), model.rows());
        let eight = model.packed(Bits::Eight, Bits::Eight);
        // The same columns, the weights within 1/127 of the largest.
        let (got, want) = (eight.rows().1, model.rows().1);
        for (g, w) in got[0].1.iter().zip(&want[0].1) {
            assert_eq!(g.0, w.0);
            assert!((g.1 - w.1).abs() <= 7.0 / 127.0, "{w:?} came back {g:?}");
        }
    }

    #[test]
    fn packed_rows_are_found_past_the_sampled_starts() {
        // Rows of 1 to 12 weights, more than SAMPLE_ROWS of them.
        let labels: Vec<String> = (0..12).map(|i| i.to_string()).collect();
        let state: Vec<Row> = (0..300u32)
            .map(|r| {
                (
                    format!("f{r:03}"),
                    (0..=r % 12).map(|c| (c, (r * 12 + c) as f32)).collect(),
                )
            })
            .collect();
        let transition: Vec<Row> = (0..300u32)
            .map(|r| {
                (
                    format!("t{r:03}"),
                    (0..r % 5 + 1)
                        .map(|c| (c * 30 + r % 7, -(r as f32)))
                        .collect(),
                )
            })
            .collect();
        let model = Model::from_rows(labels, state, transition, vec![0.0; 144]);
        let packed = model.packed(Bits::Sixteen, Bits::Sixteen);
        let mut bytes = Vec::new();
        packed.write(&mut bytes, &[]).unwrap();
        let (read, _) = Model::read(&mut bytes.as_slice()).unwrap();
        assert_eq!(read.rows(), model.rows());
        let mut scores = [0.0; 12];
        read.add_state("f250", &mut scores);
        let want: Vec<f64> = (0..12)
            .map(|c| {
                if c <= 250 % 12 {
                    f64::from(to_bf16_f32((250 * 12 + c) as f32))
                } else {
                    0.0
                }
            })
            .collect();
        assert_eq!(scores.to_vec(), want);
    }

    fn to_bf16_f32(value: f32) -> f32 {
        from_bf16(to_bf16(value))
    }

    #[test]
    fn eight_bit_weights_are_near_their_rows_largest() {
        let weights = vec![(0, 300.0), (1, -2.5), (2, 0.01), (3, 77.7)];
        let model = Model::from_rows(
            vec!["a".into(); 4],
            vec![("f".into(), weights)],
            vec![],
            vec![0.0; 16],
        );
        let (got, want) = (
            model.packed(Bits::Eight, Bits::Eight).rows().0,
            model.rows().0,
        );
        for ((_, g), (_, w)) in got[0].1.iter().zip(&want[0].1) {
            assert!((g - w).abs() <= 300.0 / 127.0, "{w} came back {g}");
        }
    }

    #[test]
    fn corrupt_packed_models_dont_load() {
        let packed = model().packed(Bits::Eight, Bits::Eight);
        let mut bytes = Vec::new();
        packed.write(&mut bytes, &[]).unwrap();
        for cut in [10, bytes.len() / 2, bytes.len() - 1] {
            assert!(Model::read(&mut &bytes[..cut]).is_err(), "cut at {cut}");
        }
    }
}

impl Weights for Model {
    fn num_labels(&self) -> usize {
        self.labels.len()
    }

    fn add_state(&self, feature: &str, scores: &mut [f64]) {
        self.state.add(feature, self.scale, scores);
    }

    fn add_state_trans(&self, feature: &str, scores: &mut [f64]) {
        self.state_trans.add(feature, self.scale, scores);
    }

    fn trans(&self, i: usize, j: usize) -> f64 {
        self.scale * f64::from(self.trans[i * self.labels.len() + j])
    }
}
