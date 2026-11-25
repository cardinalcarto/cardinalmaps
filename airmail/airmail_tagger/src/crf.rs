//! A linear-chain conditional random field, scored as libpostal's is.
//!
//! A path of labels `y` through a sequence of `T` tokens scores
//!
//! ```text
//! state[0][y0] + Σ_{t ≥ 1} ( state[t][y_t] + state_trans[t][y_{t-1}][y_t] + trans[y_{t-1}][y_t] )
//! ```
//!
//! where `state[t][j]` sums the weights of token `t`'s state features for
//! label `j`, `state_trans[t][i][j]` sums the weights of its state-transition
//! features for the label pair `(i, j)`, and `trans` is a plain label-to-label
//! matrix. State-transition features let what's at a token change how likely
//! each transition into it is: a street suffix makes road → road likelier
//! than road → locality. There are no start or end transitions.

/// A label's index.
pub type Label = u16;

/// A token's features: state features, weighted per label, and
/// state-transition features, weighted per pair of the previous and current
/// labels.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenFeatures {
    pub state: Vec<String>,
    pub transition: Vec<String>,
}

/// Weights a model gives features.
pub trait Weights {
    /// How many labels there are.
    fn num_labels(&self) -> usize;
    /// Adds a state feature's weight for each label to `scores`.
    fn add_state(&self, feature: &str, scores: &mut [f64]);
    /// Adds a state-transition feature's weight for each pair `(i, j)` to
    /// `scores[i * L + j]`.
    fn add_state_trans(&self, feature: &str, scores: &mut [f64]);
    /// The transition weight from label `i` to `j`.
    fn trans(&self, i: usize, j: usize) -> f64;
}

/// The scores of one sequence's tokens under a model.
#[derive(Debug, Clone)]
pub struct Lattice {
    labels: usize,
    len: usize,
    /// `[t * L + j]`.
    state: Vec<f64>,
    /// `[(t * L + i) * L + j]`; nothing at `t = 0`.
    state_trans: Vec<f64>,
    /// `[i * L + j]`.
    trans: Vec<f64>,
}

impl Lattice {
    /// Scores `tokens` with `weights`.
    pub fn new(weights: &impl Weights, tokens: &[TokenFeatures]) -> Self {
        let labels = weights.num_labels();
        let len = tokens.len();
        let mut state = vec![0.0; len * labels];
        let mut state_trans = vec![0.0; len * labels * labels];
        for (t, token) in tokens.iter().enumerate() {
            let row = &mut state[t * labels..(t + 1) * labels];
            for feature in &token.state {
                weights.add_state(feature, row);
            }
            if t > 0 {
                let block = &mut state_trans[t * labels * labels..(t + 1) * labels * labels];
                for feature in &token.transition {
                    weights.add_state_trans(feature, block);
                }
            }
        }
        let trans = (0..labels * labels)
            .map(|k| weights.trans(k / labels, k % labels))
            .collect();
        Self {
            labels,
            len,
            state,
            state_trans,
            trans,
        }
    }

    /// A lattice from scores already summed: `state[t * L + j]`,
    /// `state_trans[(t * L + i) * L + j]` and `trans[i * L + j]`.
    #[must_use]
    pub fn from_scores(
        labels: usize,
        len: usize,
        state: Vec<f64>,
        state_trans: Vec<f64>,
        trans: Vec<f64>,
    ) -> Self {
        Self {
            labels,
            len,
            state,
            state_trans,
            trans,
        }
    }

    /// The lattice with every score multiplied by `scale`.
    #[must_use]
    pub fn scaled(&self, scale: f64) -> Self {
        let times = |v: &[f64]| v.iter().map(|x| x * scale).collect();
        Self {
            labels: self.labels,
            len: self.len,
            state: times(&self.state),
            state_trans: times(&self.state_trans),
            trans: times(&self.trans),
        }
    }

    /// The log of a path's probability.
    #[must_use]
    pub fn log_probability(&self, path: &[Label]) -> f64 {
        self.path_score(path) - self.log_partition()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn state(&self, t: usize, j: usize) -> f64 {
        self.state[t * self.labels + j]
    }

    /// The score of moving from label `i` at `t - 1` to `j` at `t`, apart
    /// from `j`'s state score.
    fn edge(&self, t: usize, i: usize, j: usize) -> f64 {
        let l = self.labels;
        self.state_trans[(t * l + i) * l + j] + self.trans[i * l + j]
    }

    /// The score of a path of labels.
    #[must_use]
    pub fn path_score(&self, path: &[Label]) -> f64 {
        path.iter().enumerate().fold(0.0, |score, (t, &j)| {
            let j = usize::from(j);
            let edge = if t > 0 {
                self.edge(t, usize::from(path[t - 1]), j)
            } else {
                0.0
            };
            score + edge + self.state(t, j)
        })
    }

    /// The best path and its score. Ties go to the lower label.
    #[must_use]
    pub fn viterbi(&self) -> (Vec<Label>, f64) {
        let l = self.labels;
        if self.len == 0 || l == 0 {
            return (Vec::new(), f64::NEG_INFINITY);
        }
        let mut best: Vec<f64> = (0..l).map(|j| self.state(0, j)).collect();
        let mut back = vec![0 as Label; self.len * l];
        for t in 1..self.len {
            let mut next = vec![f64::NEG_INFINITY; l];
            for j in 0..l {
                let (mut arg, mut max) = (0, f64::NEG_INFINITY);
                for (i, score) in best.iter().enumerate() {
                    let candidate = score + self.edge(t, i, j);
                    if candidate > max {
                        (arg, max) = (i, candidate);
                    }
                }
                next[j] = max + self.state(t, j);
                back[t * l + j] = arg as Label;
            }
            best = next;
        }
        let (mut last, score) =
            best.iter()
                .enumerate()
                .fold(
                    (0, f64::NEG_INFINITY),
                    |acc, (j, &s)| if s > acc.1 { (j, s) } else { acc },
                );
        let mut path = vec![0 as Label; self.len];
        for t in (0..self.len).rev() {
            path[t] = last as Label;
            last = usize::from(back[t * l + last]);
        }
        (path, score)
    }

    /// The `k` best paths, best first, each with its probability: how much of
    /// the distribution over every labelling it has. By Viterbi keeping the
    /// `k` best partial paths into each label.
    #[must_use]
    pub fn best_paths(&self, k: usize) -> Vec<(Vec<Label>, f64)> {
        let l = self.labels;
        if self.len == 0 || l == 0 || k == 0 {
            return Vec::new();
        }
        // For each position and label, the best partial paths ending there:
        // (score, previous label, rank of the previous partial path).
        let mut best: Vec<Vec<Vec<(f64, usize, usize)>>> = vec![vec![Vec::new(); l]; self.len];
        for j in 0..l {
            best[0][j].push((self.state(0, j), 0, 0));
        }
        for t in 1..self.len {
            for j in 0..l {
                let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
                for i in 0..l {
                    let edge = self.edge(t, i, j);
                    for (rank, (score, _, _)) in best[t - 1][i].iter().enumerate() {
                        candidates.push((score + edge, i, rank));
                    }
                }
                candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                candidates.truncate(k);
                let state = self.state(t, j);
                best[t][j] = candidates
                    .into_iter()
                    .map(|(s, i, r)| (s + state, i, r))
                    .collect();
            }
        }
        let mut ends: Vec<(f64, usize, usize)> = (0..l)
            .flat_map(|j| {
                best[self.len - 1][j]
                    .iter()
                    .enumerate()
                    .map(move |(r, (s, _, _))| (*s, j, r))
            })
            .collect();
        ends.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        ends.truncate(k);
        let log_z = self.log_partition();
        ends.into_iter()
            .map(|(score, mut j, mut rank)| {
                let mut path = vec![0 as Label; self.len];
                for t in (0..self.len).rev() {
                    path[t] = j as Label;
                    let (_, i, r) = best[t][j][rank];
                    (j, rank) = (i, r);
                }
                (path, (score - log_z).exp())
            })
            .collect()
    }

    /// The log of the sum of every path's exponentiated score.
    /// The log of the sum of every path's exponentiated score.
    #[must_use]
    pub fn log_partition(&self) -> f64 {
        let l = self.labels;
        let mut alpha: Vec<f64> = (0..l).map(|j| self.state(0, j)).collect();
        for t in 1..self.len {
            alpha = (0..l)
                .map(|j| {
                    log_sum_exp((0..l).map(|i| alpha[i] + self.edge(t, i, j))) + self.state(t, j)
                })
                .collect();
        }
        log_sum_exp(alpha.into_iter())
    }

    /// Each token's probability of each label, `[t][j]`, by forward-backward
    /// in log space.
    #[must_use]
    pub fn marginals(&self) -> Vec<Vec<f64>> {
        let l = self.labels;
        if self.len == 0 || l == 0 {
            return Vec::new();
        }
        let mut alpha = vec![0.0; self.len * l];
        for j in 0..l {
            alpha[j] = self.state(0, j);
        }
        for t in 1..self.len {
            for j in 0..l {
                let terms = (0..l).map(|i| alpha[(t - 1) * l + i] + self.edge(t, i, j));
                alpha[t * l + j] = log_sum_exp(terms) + self.state(t, j);
            }
        }
        let mut beta = vec![0.0; self.len * l];
        for t in (0..self.len - 1).rev() {
            for i in 0..l {
                let terms = (0..l)
                    .map(|j| self.edge(t + 1, i, j) + self.state(t + 1, j) + beta[(t + 1) * l + j]);
                beta[t * l + i] = log_sum_exp(terms);
            }
        }
        let log_z = log_sum_exp((0..l).map(|j| alpha[(self.len - 1) * l + j]));
        (0..self.len)
            .map(|t| {
                (0..l)
                    .map(|j| (alpha[t * l + j] + beta[t * l + j] - log_z).exp())
                    .collect()
            })
            .collect()
    }
}

fn log_sum_exp(terms: impl Iterator<Item = f64> + Clone) -> f64 {
    let max = terms.clone().fold(f64::NEG_INFINITY, f64::max);
    if max == f64::NEG_INFINITY {
        return max;
    }
    max + terms.map(|x| (x - max).exp()).sum::<f64>().ln()
}

/// The scale of scores that makes labelled sequences' labels most likely:
/// temperature scaling, which calibrates a model's probabilities without
/// changing which labelling is best. A perceptron learns only to rank the
/// right labels first, so its raw scores are far too confident.
///
/// The negative log-likelihood is convex in the scale, so a golden-section
/// search over its log finds the best between `1e-3` and `10`.
#[must_use]
pub fn fit_scale(labelled: &[(Lattice, Vec<Label>)]) -> f64 {
    let loss = |scale: f64| -> f64 {
        labelled
            .iter()
            .map(|(lattice, truth)| -lattice.scaled(scale).log_probability(truth))
            .sum()
    };
    let (mut lo, mut hi) = (1e-3f64.ln(), 10f64.ln());
    let ratio = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (hi - ratio * (hi - lo), lo + ratio * (hi - lo));
    let (mut fa, mut fb) = (loss(a.exp()), loss(b.exp()));
    while hi - lo > 1e-3 {
        if fa < fb {
            hi = b;
            (b, fb) = (a, fa);
            a = hi - ratio * (hi - lo);
            fa = loss(a.exp());
        } else {
            lo = a;
            (a, fa) = (b, fb);
            b = lo + ratio * (hi - lo);
            fb = loss(b.exp());
        }
    }
    f64::midpoint(lo, hi).exp()
}

#[cfg(test)]
mod test {
    use super::*;

    /// Weights from a seed, for checking the algorithms against brute force.
    struct Random {
        labels: usize,
        seed: u64,
    }

    impl Random {
        fn value(&self, key: &str, k: usize) -> f64 {
            // A small hash of the key, label and seed, in [-2, 2).
            let mut h = self.seed ^ 0x9e37_79b9_7f4a_7c15;
            for b in key.bytes().chain(k.to_le_bytes()) {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            (h % 4000) as f64 / 1000.0 - 2.0
        }
    }

    impl Weights for Random {
        fn num_labels(&self) -> usize {
            self.labels
        }
        fn add_state(&self, feature: &str, scores: &mut [f64]) {
            for (j, s) in scores.iter_mut().enumerate() {
                *s += self.value(feature, j);
            }
        }
        fn add_state_trans(&self, feature: &str, scores: &mut [f64]) {
            for (k, s) in scores.iter_mut().enumerate() {
                *s += self.value(feature, 100 + k);
            }
        }
        fn trans(&self, i: usize, j: usize) -> f64 {
            self.value("trans", i * self.labels + j)
        }
    }

    fn tokens(n: usize) -> Vec<TokenFeatures> {
        (0..n)
            .map(|t| TokenFeatures {
                state: vec![format!("w{t}"), "bias".into()],
                transition: vec![format!("t{t}")],
            })
            .collect()
    }

    fn all_paths(len: usize, labels: usize) -> Vec<Vec<Label>> {
        let mut paths = vec![Vec::new()];
        for _ in 0..len {
            paths = paths
                .into_iter()
                .flat_map(|p| (0..labels).map(move |j| [p.clone(), vec![j as Label]].concat()))
                .collect();
        }
        paths
    }

    #[test]
    fn viterbi_finds_the_best_path() {
        for seed in 0..20 {
            let weights = Random { labels: 3, seed };
            let lattice = Lattice::new(&weights, &tokens(4));
            let (path, score) = lattice.viterbi();
            let best = all_paths(4, 3)
                .into_iter()
                .map(|p| lattice.path_score(&p))
                .fold(f64::NEG_INFINITY, f64::max);
            assert!((score - best).abs() < 1e-9, "seed {seed}");
            assert!((lattice.path_score(&path) - score).abs() < 1e-9);
        }
    }

    #[test]
    fn marginals_are_path_probabilities() {
        for seed in 0..10 {
            let weights = Random { labels: 3, seed };
            let lattice = Lattice::new(&weights, &tokens(4));
            let paths = all_paths(4, 3);
            let z: f64 = paths.iter().map(|p| lattice.path_score(p).exp()).sum();
            let marginals = lattice.marginals();
            for t in 0..4 {
                let total: f64 = marginals[t].iter().sum();
                assert!((total - 1.0).abs() < 1e-9);
                for j in 0..3 {
                    let expected: f64 = paths
                        .iter()
                        .filter(|p| usize::from(p[t]) == j)
                        .map(|p| lattice.path_score(p).exp())
                        .sum::<f64>()
                        / z;
                    assert!(
                        (marginals[t][j] - expected).abs() < 1e-9,
                        "seed {seed} t {t} j {j}"
                    );
                }
            }
        }
    }

    #[test]
    fn best_paths_are_the_k_best_with_their_probabilities() {
        for seed in 0..10 {
            let weights = Random { labels: 3, seed };
            let lattice = Lattice::new(&weights, &tokens(4));
            let paths = all_paths(4, 3);
            let z: f64 = paths.iter().map(|p| lattice.path_score(p).exp()).sum();
            let mut scored: Vec<(Vec<Label>, f64)> = paths
                .into_iter()
                .map(|p| {
                    let s = lattice.path_score(&p);
                    (p, s)
                })
                .collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            let best = lattice.best_paths(5);
            assert_eq!(best.len(), 5);
            for ((path, probability), (want, score)) in best.iter().zip(&scored) {
                assert!(
                    (lattice.path_score(path) - score).abs() < 1e-9,
                    "seed {seed}: {path:?} vs {want:?}"
                );
                assert!((probability - score.exp() / z).abs() < 1e-9);
            }
            assert_eq!(best[0].0, lattice.viterbi().0);
        }
    }

    #[test]
    fn empty_sequences() {
        let lattice = Lattice::new(&Random { labels: 3, seed: 0 }, &[]);
        assert!(lattice.viterbi().0.is_empty());
        assert!(lattice.marginals().is_empty());
    }

    #[test]
    fn scale_fits_how_often_labels_are_right() {
        // One token whose second label scores 10 more, right 70% of the time:
        // the scale that's most likely gives it sigmoid(10 s) = 0.7.
        let lattice = Lattice::from_scores(2, 1, vec![0.0, 10.0], vec![0.0; 4], vec![0.0; 4]);
        let labelled: Vec<(Lattice, Vec<Label>)> = (0..10)
            .map(|i| (lattice.clone(), vec![Label::from(i < 7)]))
            .collect();
        let scale = fit_scale(&labelled);
        let expected = (0.7f64 / 0.3).ln() / 10.0;
        assert!((scale - expected).abs() < 1e-3, "{scale} vs {expected}");
    }
}
