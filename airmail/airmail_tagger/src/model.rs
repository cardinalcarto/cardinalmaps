//! A trained model, for tagging.
//!
//! On disk, and in memory, each feature set is an FST from feature to row,
//! and rows of one-byte columns and 16-bit (bfloat16) weights: about 3 bytes
//! a weight, read without building a hash table.

use std::io::{self, Read, Write};

use fst::Streamer;
use fst::{Map, MapBuilder};

use crate::crf::{Label, Lattice, TokenFeatures, Weights};

/// A weight as bfloat16: the top half of an f32, rounded to nearest even.
/// Its 8 bits of mantissa keep weights to within 0.4%.
fn to_bf16(value: f32) -> u16 {
    let bits = value.to_bits();
    ((bits + 0x7FFF + ((bits >> 16) & 1)) >> 16) as u16
}

fn from_bf16(value: u16) -> f32 {
    f32::from_bits(u32::from(value) << 16)
}

/// Sparse rows of weights, by feature.
#[derive(Clone)]
struct Rows {
    /// Feature to row number.
    keys: Map<Vec<u8>>,
    /// Row `r` is `columns[starts[r]..starts[r + 1]]`.
    starts: Vec<u32>,
    columns: Vec<u8>,
    values: Vec<u16>,
}

impl std::fmt::Debug for Rows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rows").field("features", &self.keys.len()).field("weights", &self.values.len()).finish()
    }
}

impl Rows {
    fn new(mut rows: Vec<(String, Vec<(u32, f32)>)>) -> Self {
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows.dedup_by(|a, b| a.0 == b.0);
        let mut builder = MapBuilder::memory();
        let (mut starts, mut columns, mut values) = (vec![0], Vec::new(), Vec::new());
        for (row, (feature, weights)) in rows.iter().enumerate() {
            builder.insert(feature, row as u64).expect("sorted, distinct features");
            for (column, value) in weights {
                columns.push(u8::try_from(*column).expect("columns fit in a byte"));
                values.push(to_bf16(*value));
            }
            starts.push(columns.len() as u32);
        }
        let keys = Map::new(builder.into_inner().expect("an FST in memory")).expect("a valid FST");
        Self { keys, starts, columns, values }
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
        let values = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        Ok(Self { keys, starts, columns, values })
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
        Ok(Rows::new(rows))
    }

    fn len(&self) -> usize {
        self.keys.len()
    }

    /// The rows, each feature with its weights whose magnitude is at least
    /// `min_weight`, without the rows left with none.
    fn pruned(&self, min_weight: f32) -> Vec<(String, Vec<(u32, f32)>)> {
        let mut rows = Vec::with_capacity(self.len());
        let mut stream = self.keys.stream();
        while let Some((key, row)) = stream.next() {
            let (start, end) = (self.starts[row as usize] as usize, self.starts[row as usize + 1] as usize);
            let weights: Vec<(u32, f32)> = self.columns[start..end]
                .iter()
                .zip(&self.values[start..end])
                .map(|(c, v)| (u32::from(*c), from_bf16(*v)))
                .filter(|(_, w)| w.abs() >= min_weight)
                .collect();
            if !weights.is_empty() {
                rows.push((String::from_utf8_lossy(key).into_owned(), weights));
            }
        }
        rows
    }

    /// How many weights there are of at least each magnitude.
    fn magnitudes(&self) -> Vec<f32> {
        self.values.iter().map(|v| from_bf16(*v).abs()).collect()
    }

    fn add(&self, feature: &str, scale: f64, scores: &mut [f64]) {
        if let Some(row) = self.keys.get(feature) {
            let (start, end) = (self.starts[row as usize] as usize, self.starts[row as usize + 1] as usize);
            for (column, value) in self.columns[start..end].iter().zip(&self.values[start..end]) {
                scores[*column as usize] += scale * f64::from(from_bf16(*value));
            }
        }
    }
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
        state: Vec<(String, Vec<(u32, f32)>)>,
        state_trans: Vec<(String, Vec<(u32, f32)>)>,
        trans: Vec<f32>,
    ) -> Self {
        Self { labels, state: Rows::new(state), state_trans: Rows::new(state_trans), trans, scale: 1.0 }
    }

    /// The model without its weights smaller in magnitude than
    /// `min_weight`, nor the features left without weights: smaller, as
    /// nearly as accurate as the weights dropped are small.
    #[must_use]
    pub fn pruned(&self, min_weight: f32) -> Self {
        Self {
            labels: self.labels.clone(),
            state: Rows::new(self.state.pruned(min_weight)),
            state_trans: Rows::new(self.state_trans.pruned(min_weight)),
            trans: self.trans.clone(),
            scale: self.scale,
        }
    }

    /// The magnitude of every state and state-transition weight, unsorted.
    #[must_use]
    pub fn weight_magnitudes(&self) -> Vec<f32> {
        let mut all = self.state.magnitudes();
        all.extend(self.state_trans.magnitudes());
        all
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

    /// Writes the model, with `extra` (such as a vocabulary) after it.
    ///
    /// # Errors
    /// If writing fails.
    pub fn write(&self, out: &mut impl Write, extra: &[(String, u32)]) -> io::Result<()> {
        out.write_all(MAGIC)?;
        write_u32(out, self.labels.len() as u32)?;
        for label in &self.labels {
            write_str(out, label)?;
        }
        self.state.write(out)?;
        self.state_trans.write(out)?;
        for value in &self.trans {
            out.write_all(&value.to_le_bytes())?;
        }
        out.write_all(&self.scale.to_le_bytes())?;
        let mut extra: Vec<&(String, u32)> = extra.iter().collect();
        extra.sort();
        extra.dedup_by(|a, b| a.0 == b.0);
        let mut builder = MapBuilder::memory();
        for (key, value) in extra {
            builder.insert(key, u64::from(*value)).map_err(invalid)?;
        }
        write_bytes(out, &builder.into_inner().map_err(invalid)?)
    }

    /// Reads a model written by `write` (or by the first format), and its
    /// extra.
    ///
    /// # Errors
    /// If it isn't one.
    pub fn read(input: &mut impl Read) -> io::Result<(Self, Vec<(String, u32)>)> {
        let magic: [u8; 8] = read_array(input)?;
        let v1 = &magic == MAGIC_V1;
        let v2 = &magic == MAGIC_V2;
        if !v1 && !v2 && &magic != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a tagger model"));
        }
        let n = read_u32(input)? as usize;
        let labels = (0..n).map(|_| read_str(input)).collect::<io::Result<Vec<_>>>()?;
        let read_rows = if v1 { Rows::read_v1 } else { Rows::read };
        let state = read_rows(input)?;
        let state_trans = read_rows(input)?;
        let trans = (0..n * n).map(|_| Ok(f32::from_le_bytes(read_array(input)?))).collect::<io::Result<Vec<_>>>()?;
        let scale = if v1 || v2 { 1.0 } else { f64::from_le_bytes(read_array(input)?) };
        let extra = if v1 {
            let m = read_u32(input)?;
            (0..m).map(|_| Ok((read_str(input)?, read_u32(input)?))).collect::<io::Result<Vec<_>>>()?
        } else {
            let map = Map::new(read_bytes(input)?).map_err(invalid)?;
            let mut extra = Vec::with_capacity(map.len());
            let mut stream = fst::IntoStreamer::into_stream(&map);
            while let Some((key, value)) = fst::Streamer::next(&mut stream) {
                extra.push((String::from_utf8(key.to_vec()).map_err(invalid)?, value as u32));
            }
            extra
        };
        Ok((Self { labels, state, state_trans, trans, scale }, extra))
    }
}

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
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes)?;
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

    #[test]
    fn models_round_trip() {
        let model = Model::from_rows(
            vec!["a".into(), "b".into()],
            vec![("word|x".into(), vec![(0, 1.5), (1, -0.5)]), ("bias".into(), vec![(1, 0.25)])],
            vec![("word|y".into(), vec![(3, 2.0)])],
            vec![0.1, 0.2, 0.3, 0.4],
        );
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
            assert!((back - value).abs() <= value.abs() / 256.0, "{value} came back {back}");
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
