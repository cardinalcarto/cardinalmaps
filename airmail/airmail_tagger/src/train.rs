//! Training by structured averaged perceptron, as libpostal does.
//!
//! Each example is decoded with the current weights. Where the best path is
//! wrong, the features at each wrong token move weight from the guessed
//! label to the true one, and the state-transition features and transitions
//! at each wrong step from the guessed label pair to the true one. The model
//! keeps the average of each weight over every example trained on, which
//! generalizes far better than the final weights. (libpostal averages over
//! mistaken tokens only, so once it stops making mistakes its final weights
//! hardly count; this averages as Collins's perceptron does.) Features
//! updated fewer than `min_updates` times are ignored, in training as in the
//! model, so rare noise neither scores nor takes up space.

use std::collections::HashMap;

use crate::crf::{Label, Lattice, TokenFeatures};
use crate::model::Model;

/// A weight and the running sum for its average.
#[derive(Debug, Clone, Copy, Default)]
struct Averaged {
    value: f64,
    /// Σ value over the clock ticks up to `last`.
    total: f64,
    last: u64,
}

impl Averaged {
    fn add(&mut self, delta: f64, clock: u64) {
        self.total += (clock - self.last) as f64 * self.value;
        self.last = clock;
        self.value += delta;
    }

    fn average(&self, clock: u64) -> f64 {
        (self.total + (clock - self.last) as f64 * self.value) / clock as f64
    }
}

/// One feature's weights, sparse over labels (or label pairs).
#[derive(Debug, Clone, Default)]
struct Row {
    updates: u32,
    weights: Vec<(u32, Averaged)>,
}

impl Row {
    fn add(&mut self, column: u32, delta: f64, clock: u64) {
        match self.weights.iter_mut().find(|(c, _)| *c == column) {
            Some((_, weight)) => weight.add(delta, clock),
            None => {
                let mut weight = Averaged {
                    last: clock,
                    ..Averaged::default()
                };
                weight.add(delta, clock);
                self.weights.push((column, weight));
            }
        }
    }
}

/// Feature strings as numbers, so training holds each string once.
#[derive(Debug, Default, Clone)]
pub struct Interner {
    ids: HashMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    /// The feature's number, numbering it if it's new.
    pub fn id(&mut self, feature: &str) -> u32 {
        if let Some(id) = self.ids.get(feature) {
            return *id;
        }
        let id = self.names.len() as u32;
        self.ids.insert(feature.to_string(), id);
        self.names.push(feature.to_string());
        id
    }

    /// Features' strings, by number.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Numbers a token's features.
    pub fn encode(&mut self, token: &TokenFeatures) -> Encoded {
        Encoded {
            state: token.state.iter().map(|f| self.id(f)).collect(),
            transition: token.transition.iter().map(|f| self.id(f)).collect(),
        }
    }

    /// Takes in another interner's features, returning each of its numbers'
    /// number here.
    pub fn absorb(&mut self, other: &Interner) -> Vec<u32> {
        other.names.iter().map(|name| self.id(name)).collect()
    }
}

/// A token's features as numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Encoded {
    pub state: Vec<u32>,
    pub transition: Vec<u32>,
}

impl Encoded {
    /// With its numbers renumbered by `map`.
    #[must_use]
    pub fn remap(&self, map: &[u32]) -> Self {
        Self {
            state: self.state.iter().map(|f| map[*f as usize]).collect(),
            transition: self.transition.iter().map(|f| map[*f as usize]).collect(),
        }
    }
}

/// Rows of weights by feature number.
#[derive(Debug, Default)]
struct Table {
    rows: Vec<Row>,
}

impl Table {
    fn row_mut(&mut self, feature: u32) -> &mut Row {
        let i = feature as usize;
        if i >= self.rows.len() {
            self.rows.resize_with(i + 1, Row::default);
        }
        &mut self.rows[i]
    }

    /// The row of a feature updated at least `min_updates` times.
    fn row(&self, feature: u32, min_updates: u32) -> Option<&Row> {
        self.rows
            .get(feature as usize)
            .filter(|row| row.updates >= min_updates)
    }
}

/// Trains a model on labelled sequences of numbered features (see
/// `Interner`).
#[derive(Debug)]
pub struct Trainer {
    labels: Vec<String>,
    min_updates: u32,
    state: Table,
    state_trans: Table,
    /// `[i * L + j]`.
    trans: Vec<Averaged>,
    /// Examples so far: the clock weights are averaged over.
    clock: u64,
}

/// How many updates a feature needs to count, as in libpostal.
pub const DEFAULT_MIN_UPDATES: u32 = 5;

impl Trainer {
    #[must_use]
    pub fn new(labels: Vec<String>, min_updates: u32) -> Self {
        let n = labels.len();
        Self {
            labels,
            min_updates,
            state: Table::default(),
            state_trans: Table::default(),
            trans: vec![Averaged::default(); n * n],
            clock: 0,
        }
    }

    #[must_use]
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// The lattice of a sequence under the current weights.
    fn lattice(&self, tokens: &[Encoded]) -> Lattice {
        let l = self.labels.len();
        let len = tokens.len();
        let mut state = vec![0.0; len * l];
        let mut state_trans = vec![0.0; len * l * l];
        for (t, token) in tokens.iter().enumerate() {
            for feature in &token.state {
                if let Some(row) = self.state.row(*feature, self.min_updates) {
                    for (c, w) in &row.weights {
                        state[t * l + *c as usize] += w.value;
                    }
                }
            }
            if t > 0 {
                for feature in &token.transition {
                    if let Some(row) = self.state_trans.row(*feature, self.min_updates) {
                        for (c, w) in &row.weights {
                            state_trans[t * l * l + *c as usize] += w.value;
                        }
                    }
                }
            }
        }
        let trans = self.trans.iter().map(|w| w.value).collect();
        Lattice::from_scores(l, len, state, state_trans, trans)
    }

    /// Learns from one example; returns whether the current weights already
    /// labelled it right.
    ///
    /// # Panics
    /// If `truth` isn't as long as `tokens`.
    pub fn train(&mut self, tokens: &[Encoded], truth: &[Label]) -> bool {
        assert_eq!(tokens.len(), truth.len(), "one label per token");
        self.clock += 1;
        let (guess, _) = self.lattice(tokens).viterbi();
        if guess == truth {
            return true;
        }
        let l = self.labels.len() as u32;
        for (t, token) in tokens.iter().enumerate() {
            if guess[t] == truth[t] {
                continue;
            }
            for feature in &token.state {
                let row = self.state.row_mut(*feature);
                row.updates += 1;
                row.add(u32::from(guess[t]), -1.0, self.clock);
                row.add(u32::from(truth[t]), 1.0, self.clock);
            }
        }
        for t in 1..tokens.len() {
            if guess[t] == truth[t] && guess[t - 1] == truth[t - 1] {
                continue;
            }
            let guessed = u32::from(guess[t - 1]) * l + u32::from(guess[t]);
            let right = u32::from(truth[t - 1]) * l + u32::from(truth[t]);
            for feature in &tokens[t].transition {
                let row = self.state_trans.row_mut(*feature);
                row.updates += 1;
                row.add(guessed, -1.0, self.clock);
                row.add(right, 1.0, self.clock);
            }
            self.trans[guessed as usize].add(-1.0, self.clock);
            self.trans[right as usize].add(1.0, self.clock);
        }
        false
    }

    /// The model of the averaged weights, with features' strings from
    /// `interner`, without features updated fewer than `min_updates` times.
    #[must_use]
    pub fn finish(self, interner: &Interner) -> Model {
        let clock = self.clock.max(1);
        let rows = |table: Table, min_updates: u32| -> Vec<(String, Vec<(u32, f32)>)> {
            let mut kept: Vec<(String, Vec<(u32, f32)>)> = table
                .rows
                .into_iter()
                .enumerate()
                .filter(|(_, row)| row.updates >= min_updates)
                .map(|(id, row)| {
                    let mut weights: Vec<(u32, f32)> = row
                        .weights
                        .iter()
                        .map(|(c, w)| (*c, w.average(clock) as f32))
                        .filter(|(_, w)| *w != 0.0)
                        .collect();
                    weights.sort_by_key(|(c, _)| *c);
                    (interner.names()[id].clone(), weights)
                })
                .collect();
            kept.sort_by(|a, b| a.0.cmp(&b.0));
            kept
        };
        let trans = self.trans.iter().map(|w| w.average(clock) as f32).collect();
        Model::from_rows(
            self.labels,
            rows(self.state, self.min_updates),
            rows(self.state_trans, self.min_updates),
            trans,
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn token(state: &[&str], transition: &[&str]) -> TokenFeatures {
        TokenFeatures {
            state: state.iter().map(|s| s.to_string()).collect(),
            transition: transition.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Words `a` and `b` are labels 0 and 1; `same` takes the previous
    /// token's label, which only a state-transition feature can learn.
    fn example(words: &[&str]) -> (Vec<TokenFeatures>, Vec<Label>) {
        let mut labels = Vec::new();
        for word in words {
            labels.push(match *word {
                "a" => 0,
                "b" => 1,
                _ => *labels.last().unwrap_or(&0),
            });
        }
        let tokens = words
            .iter()
            .map(|w| token(&[&format!("word|{w}"), "bias"], &[&format!("word|{w}")]))
            .collect();
        (tokens, labels)
    }

    #[test]
    fn learns_words_and_transitions_that_depend_on_them() {
        let data: Vec<_> = [
            &["a", "same", "b"][..],
            &["b", "same", "a"],
            &["a", "b", "same"],
            &["b", "a", "same"],
            &["a", "same", "same", "b", "same"],
            &["b", "same", "a", "same", "same"],
        ]
        .iter()
        .map(|w| example(w))
        .collect();
        let mut interner = Interner::default();
        let encoded: Vec<(Vec<Encoded>, Vec<Label>)> = data
            .iter()
            .map(|(t, l)| (t.iter().map(|t| interner.encode(t)).collect(), l.clone()))
            .collect();
        let mut trainer = Trainer::new(vec!["A".into(), "B".into()], 1);
        for _ in 0..20 {
            for (tokens, labels) in &encoded {
                trainer.train(tokens, labels);
            }
        }
        let model = trainer.finish(&interner);
        for (tokens, labels) in &data {
            assert_eq!(&model.tag(tokens).0, labels);
        }
        // `same` copies whichever label comes before it.
        for words in [&["a", "same"][..], &["b", "same"], &["b", "same", "same"]] {
            let (tokens, labels) = example(words);
            assert_eq!(model.tag(&tokens).0, labels, "{words:?}");
        }
    }

    #[test]
    fn rarely_updated_features_are_dropped() {
        let (tokens, labels) = example(&["a", "b"]);
        let mut interner = Interner::default();
        let tokens: Vec<Encoded> = tokens.iter().map(|t| interner.encode(t)).collect();
        let mut trainer = Trainer::new(vec!["A".into(), "B".into()], 5);
        // Never right until features count, so every pass updates; but only
        // four passes, so none reaches five updates.
        for _ in 0..4 {
            trainer.train(&tokens, &labels);
        }
        assert_eq!(trainer.finish(&interner).num_features(), (0, 0));
    }

    #[test]
    fn interners_merge() {
        let mut a = Interner::default();
        let mut b = Interner::default();
        assert_eq!((a.id("x"), a.id("y")), (0, 1));
        assert_eq!((b.id("y"), b.id("z")), (0, 1));
        let map = a.absorb(&b);
        assert_eq!(map, vec![1, 2]);
        assert_eq!(
            Encoded {
                state: vec![0, 1],
                transition: vec![1]
            }
            .remap(&map),
            Encoded {
                state: vec![1, 2],
                transition: vec![2]
            }
        );
    }
}
