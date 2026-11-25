//! Trains and tries the query tagger.
//!
//! ```text
//! tagger train --index <dir> --out <model> [--stride N] [--epochs N] [--copies N]
//! tagger tag --index <dir> --model <model> [--typing] [--focus lon,lat] [query]
//! tagger eval --index <dir> --model <model> <tsv>
//! tagger convert <model> <out> [--min-weight W]
//! tagger calibrate --index <dir> --model <model> --out <model> [--stride N] [--copies N] [--seed N]
//! tagger score --index <dir> --model <model>... [--tsv <tsv>] [--json <out>]
//! tagger usage --index <dir> --model <model> --out <tsv> [--every N]
//! ```
//!
//! `convert` rewrites a model in the current format.
//!
//! `eval` scores a model on hand-labelled queries, such as `eval/tagger.tsv`.
//!
//! Training makes labelled queries from every `stride`th document of the
//! index (see `airmail::tagger_data`), holds a tenth of the documents out,
//! and reports how well the trained model labels their queries.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

use airmail::gazetteer::IndexGazetteer;
use airmail::index::{AirmailIndex, SearchHit};
use airmail::poi::WofPlace;
use airmail::tagger_data::{Example, Generator, sample};
use airmail_tagger::crf::{Lattice, fit_scale};
use airmail_tagger::features::{Extractor, Gazetteer, Query, Vocabulary};
use airmail_tagger::labels::Tag;
use airmail_tagger::lexicon::Lexicon;
use airmail_tagger::text::{fold, words};
use airmail_tagger::{Encoded, Interner, Label, Model, TokenFeatures, Trainer};
use anyhow::Result;
use clap::{Parser, Subcommand};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

#[path = "tagger/score.rs"]
mod score;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Train {
        /// Indexes to make examples from, each its own gazetteer; more than
        /// one for places in more countries.
        #[arg(long, required = true)]
        index: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        /// Use every this-many-th document.
        #[arg(long, default_value_t = 20)]
        stride: u32,
        #[arg(long, default_value_t = 5)]
        epochs: usize,
        /// Examples made from each document.
        #[arg(long, default_value_t = 2)]
        copies: usize,
        #[arg(long, default_value_t = airmail_tagger::train::DEFAULT_MIN_UPDATES)]
        min_updates: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Rewrites a model in the current format, pruned of its weights smaller
    /// than `--min-weight` (see `Model::pruned`) if given.
    Convert {
        model: PathBuf,
        out: PathBuf,
        #[arg(long)]
        min_weight: Option<f32>,
    },
    /// Prints training queries made from an index, with their tags.
    Examples {
        #[arg(long)]
        index: PathBuf,
        #[arg(long, default_value_t = 2000)]
        stride: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Fits a model's scale to the queries its training held out: give the
    /// indexes, stride, copies and seed it was trained with.
    Calibrate {
        #[arg(long, required = true)]
        index: Vec<PathBuf>,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 20)]
        stride: u32,
        #[arg(long, default_value_t = 2)]
        copies: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Scores models on the queries held out of training on the indexes
    /// (give the stride, copies and seed of training), and on hand-labelled
    /// queries; the first model is the one the others are compared with.
    Score {
        #[arg(long, required = true)]
        index: Vec<PathBuf>,
        #[arg(long, required = true)]
        model: Vec<PathBuf>,
        #[arg(long, default_value_t = 20)]
        stride: u32,
        #[arg(long, default_value_t = 2)]
        copies: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// The most held-out queries of each index.
        #[arg(long, default_value_t = MAX_HELD_OUT)]
        max: usize,
        #[arg(long)]
        tsv: Option<PathBuf>,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Counts how often a model's features fire on every `every`th
    /// training query, for `tagger_prune --usage`.
    Usage {
        #[arg(long, required = true)]
        index: Vec<PathBuf>,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 20)]
        stride: u32,
        #[arg(long, default_value_t = 2)]
        copies: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 16)]
        every: usize,
    },
    Eval {
        #[arg(long)]
        index: PathBuf,
        #[arg(long)]
        model: PathBuf,
        tsv: PathBuf,
    },
    Tag {
        #[arg(long)]
        index: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        typing: bool,
        /// `lon,lat`.
        #[arg(long)]
        focus: Option<String>,
        /// Without one, reads queries a line at a time (see `repl`).
        query: Option<String>,
    },
}

/// A query's words from text, as the tagger reads it.
fn query_of(text: &str, typing: bool, focus: Option<[f64; 2]>) -> Query {
    let mut query = Query {
        typing,
        focus,
        ..Query::default()
    };
    for (i, section) in text.split(',').enumerate() {
        let folded = fold(section);
        for (j, (_, word)) in words(&folded).into_iter().enumerate() {
            query.section_starts.push(i > 0 && j == 0);
            query.words.push(word.to_string());
        }
    }
    query
}

/// Features of examples, computed in parallel.
fn featurize(
    index: &AirmailIndex,
    lexicon: &Lexicon,
    vocabulary: &Vocabulary,
    examples: &[Example],
) -> Vec<(Vec<TokenFeatures>, Vec<Label>)> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = examples.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = examples
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let gazetteer = IndexGazetteer::new(index).expect("searcher");
                    let extractor = Extractor {
                        lexicon,
                        vocabulary,
                    };
                    chunk
                        .iter()
                        .enumerate()
                        .map(|(i, example)| {
                            if i % 20_000 == 0 && i > 0 {
                                eprintln!("  a thread has done {i} of {} examples", chunk.len());
                            }
                            let areas = gazetteer.areas(&example.query);
                            let features = extractor.features(&example.query, &areas);
                            (features, example.tags.iter().map(|t| t.label()).collect())
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("featurize"))
            .collect()
    })
}

/// The most held-out examples to measure on.
const MAX_HELD_OUT: usize = 100_000;

/// An index and the documents sampled from it to make examples of.
struct Source {
    index: AirmailIndex,
    docs: Vec<SearchHit>,
    admins: HashMap<u64, WofPlace>,
}

fn sources(indexes: &[PathBuf], stride: u32, seed: u64, start: Instant) -> Result<Vec<Source>> {
    indexes
        .iter()
        .map(|path| {
            let index = AirmailIndex::open_read_only(path.to_str().expect("path"))?;
            let (docs, admins) = sample(&index, stride, seed as u32)?;
            eprintln!(
                "Sampled {} documents of {} in {:?}",
                docs.len(),
                path.display(),
                start.elapsed()
            );
            Ok(Source {
                index,
                docs,
                admins,
            })
        })
        .collect()
}

/// How many ranges each index's documents are split into. Each range's
/// examples are made with their own random choices, so the examples are
/// the same however many threads make them.
const RANGES_PER_SOURCE: usize = 1024;

/// Some of a source's documents, examples are made from together.
struct Range {
    source: usize,
    docs: std::ops::Range<usize>,
    number: usize,
}

fn ranges(sources: &[Source]) -> Vec<Range> {
    let mut ranges = Vec::new();
    for (source, s) in sources.iter().enumerate() {
        let n = s.docs.len();
        let per = n.div_ceil(RANGES_PER_SOURCE).max(1);
        for start in (0..n).step_by(per) {
            ranges.push(Range {
                source,
                docs: start..(start + per).min(n),
                number: ranges.len(),
            });
        }
    }
    ranges
}

/// Calls `each(example, held_out)` for every example made from a range's
/// documents, the same every time for the same arguments. A tenth of the
/// documents are held out.
fn range_examples(
    generator: &mut Generator,
    sources: &[Source],
    range: &Range,
    copies: usize,
    seed: u64,
    mut each: impl FnMut(Example, bool),
) {
    generator.reseed(seed ^ (range.number as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let source = &sources[range.source];
    let docs = &source.docs;
    for i in range.docs.clone() {
        let hit = &docs[i];
        // Somewhere else, for a focus far away.
        let elsewhere = docs[(i * 7919 + 13) % docs.len()].center;
        for _ in 0..copies * airmail::tagger_data::copies(hit.kind) {
            // A name that folds to nothing makes a query of no words.
            for example in generator.examples(hit, &source.admins, [elsewhere.x(), elsewhere.y()]) {
                if example.query.words.is_empty() {
                    continue;
                }
                each(example, i % 10 == 0);
            }
        }
    }
}

/// `run(state, range)` for every range, on a thread per core, each thread
/// with its own `init()` state: the states, and the results in range order.
fn by_range<S: Send, R: Send>(
    ranges: &[Range],
    init: impl Fn() -> S + Sync,
    run: impl Fn(&mut S, &Range) -> R + Sync,
) -> (Vec<S>, Vec<R>) {
    let threads = std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(ranges.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (states, mut done): (Vec<S>, Vec<Vec<(usize, R)>>) = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let (mut state, mut done) = (init(), Vec::new());
                    loop {
                        let job = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(range) = ranges.get(job) else {
                            return (state, done);
                        };
                        done.push((job, run(&mut state, range)));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a range panicked"))
            .unzip()
    });
    let mut done: Vec<(usize, R)> = done.drain(..).flatten().collect();
    done.sort_by_key(|(job, _)| *job);
    (states, done.into_iter().map(|(_, r)| r).collect())
}

/// What the first pass learns: the training words, how many examples, and
/// what they're like.
#[derive(Default)]
struct Counts {
    vocabulary: Vocabulary,
    description: Description,
    training: usize,
}

/// The first pass over the examples: their counts, and each range's
/// held-out count.
fn count(sources: &[Source], ranges: &[Range], copies: usize, seed: u64) -> (Counts, Vec<usize>) {
    let (states, held) = by_range(
        ranges,
        || (Generator::new(seed), Counts::default()),
        |(generator, counts), range| {
            let mut held = 0;
            range_examples(
                generator,
                sources,
                range,
                copies,
                seed,
                |example, held_out| {
                    if held_out {
                        held += 1;
                    } else {
                        counts.training += 1;
                        for word in &example.query.words {
                            counts.vocabulary.add(word);
                        }
                        counts.description.add(&example);
                    }
                },
            );
            held
        },
    );
    let mut all = Counts::default();
    for (_, counts) in states {
        all.vocabulary.merge(counts.vocabulary);
        all.description.merge(counts.description);
        all.training += counts.training;
    }
    (all, held)
}

/// The most held-out examples to keep from each source: enough to measure
/// by, without holding every one's features as strings.
fn held_out_steps(sources: &[Source], ranges: &[Range], held: &[usize]) -> Vec<usize> {
    let keep = MAX_HELD_OUT / sources.len().max(1);
    let mut per_source = vec![0; sources.len()];
    for (range, n) in ranges.iter().zip(held) {
        per_source[range.source] += n;
    }
    per_source.iter().map(|n| n.div_ceil(keep).max(1)).collect()
}

/// Each source's kept held-out examples.
fn held_out(sources: &[Source], copies: usize, seed: u64) -> Vec<Vec<Example>> {
    let ranges = ranges(sources);
    let (_, held) = count(sources, &ranges, copies, seed);
    let steps = held_out_steps(sources, &ranges, &held);
    let (_, kept) = by_range(
        &ranges,
        || Generator::new(seed),
        |generator, range| {
            let (mut seen, mut kept) = (0, Vec::new());
            range_examples(
                generator,
                sources,
                range,
                copies,
                seed,
                |example, held_out| {
                    if held_out {
                        if seen % steps[range.source] == 0 {
                            kept.push(example);
                        }
                        seen += 1;
                    }
                },
            );
            kept
        },
    );
    let mut per_source = vec![Vec::new(); sources.len()];
    for (range, kept) in ranges.iter().zip(kept) {
        per_source[range.source].extend(kept);
    }
    per_source
}

/// Training examples encoded on disk, in buckets each example was put in at
/// random: an epoch reads the buckets in a random order and trains on each
/// one's examples shuffled, which shuffles them all nearly as well as
/// holding them in memory would.
struct Buckets {
    paths: Vec<PathBuf>,
    writers: Vec<BufWriter<File>>,
    rng: StdRng,
}

/// About how many examples a bucket holds, so one fits in memory easily.
const BUCKET_EXAMPLES: usize = 1_000_000;

impl Buckets {
    fn create(dir: &std::path::Path, examples: usize, seed: u64) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let n = examples.div_ceil(BUCKET_EXAMPLES).max(1);
        let paths: Vec<PathBuf> = (0..n).map(|i| dir.join(format!("bucket-{i}"))).collect();
        let writers = paths
            .iter()
            .map(|p| Ok(BufWriter::with_capacity(1 << 20, File::create(p)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            paths,
            writers,
            rng: StdRng::seed_from_u64(seed),
        })
    }

    /// Writes an example to a bucket at random: its token count, labels,
    /// and each token's feature counts and numbers, little-endian.
    fn add(&mut self, tokens: &[Encoded], labels: &[Label]) -> Result<()> {
        let bucket = self.rng.random_range(0..self.writers.len());
        let w = &mut self.writers[bucket];
        w.write_all(&(tokens.len() as u16).to_le_bytes())?;
        for label in labels {
            w.write_all(&label.to_le_bytes())?;
        }
        for token in tokens {
            w.write_all(&(token.state.len() as u16).to_le_bytes())?;
            w.write_all(&(token.transition.len() as u16).to_le_bytes())?;
            for f in token.state.iter().chain(&token.transition) {
                w.write_all(&f.to_le_bytes())?;
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Vec<PathBuf>> {
        for mut w in self.writers {
            w.flush()?;
        }
        Ok(self.paths)
    }
}

/// A bucket's examples, as `Buckets::add` wrote them.
fn read_bucket(path: &std::path::Path) -> Result<Vec<(Vec<Encoded>, Vec<Label>)>> {
    let bytes = std::fs::read(path)?;
    let mut at = 0;
    let u16_at = |at: &mut usize| {
        let v = u16::from_le_bytes([bytes[*at], bytes[*at + 1]]);
        *at += 2;
        v
    };
    let mut examples = Vec::new();
    while at < bytes.len() {
        let n = u16_at(&mut at) as usize;
        let labels: Vec<Label> = (0..n).map(|_| u16_at(&mut at)).collect();
        let mut tokens = Vec::with_capacity(n);
        for _ in 0..n {
            let (states, transitions) = (u16_at(&mut at) as usize, u16_at(&mut at) as usize);
            let mut ids = (0..states + transitions).map(|i| {
                let i = at + 4 * i;
                u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
            });
            let state = ids.by_ref().take(states).collect();
            let transition = ids.collect();
            at += 4 * (states + transitions);
            tokens.push(Encoded { state, transition });
        }
        examples.push((tokens, labels));
    }
    Ok(examples)
}

/// Examples a thread featurizes before sending them to be written.
const BATCH_EXAMPLES: usize = 2_000;

/// The most cached gazetteer lookups a featurizing thread keeps.
const MOST_CACHED: usize = 2_000_000;

/// Featurized examples, numbered by the thread's own interner.
struct Batch {
    interner: Interner,
    examples: Vec<(Vec<Encoded>, Vec<Label>)>,
}

/// The second pass: makes the examples again, featurizes and numbers the
/// training ones on a thread per core, and writes them to the buckets on
/// this one. Returns each source's kept held-out examples.
#[allow(clippy::too_many_arguments)]
fn encode_all(
    sources: &[Source],
    ranges: &[Range],
    steps: &[usize],
    copies: usize,
    seed: u64,
    extractor: &Extractor,
    interner: &mut Interner,
    buckets: &mut Buckets,
    training: usize,
    start: Instant,
) -> Result<Vec<Vec<Example>>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Batch>(256);
    let kept = std::thread::scope(|scope| -> Result<Vec<Vec<Example>>> {
        let workers = scope.spawn(move || {
            let init = || {
                (
                    Generator::new(seed),
                    None::<(usize, IndexGazetteer)>,
                    sender.clone(),
                )
            };
            let (_, kept) = by_range(ranges, init, |(generator, gazetteer, sender), range| {
                if gazetteer.as_ref().is_none_or(|(s, _)| *s != range.source) {
                    *gazetteer = Some((
                        range.source,
                        IndexGazetteer::new(&sources[range.source].index).expect("searcher"),
                    ));
                }
                let gazetteer = &gazetteer.as_ref().expect("a gazetteer").1;
                let (mut seen, mut kept) = (0, Vec::new());
                let mut batch = Batch {
                    interner: Interner::default(),
                    examples: Vec::new(),
                };
                range_examples(
                    generator,
                    sources,
                    range,
                    copies,
                    seed,
                    |example, held_out| {
                        if held_out {
                            if seen % steps[range.source] == 0 {
                                kept.push(example);
                            }
                            seen += 1;
                            return;
                        }
                        let areas = gazetteer.areas(&example.query);
                        let tokens = extractor
                            .features(&example.query, &areas)
                            .iter()
                            .map(|f| batch.interner.encode(f))
                            .collect();
                        batch
                            .examples
                            .push((tokens, example.tags.iter().map(|t| t.label()).collect()));
                        if batch.examples.len() >= BATCH_EXAMPLES {
                            let full = std::mem::replace(
                                &mut batch,
                                Batch {
                                    interner: Interner::default(),
                                    examples: Vec::new(),
                                },
                            );
                            // Only fails if the writer has stopped, which it says why.
                            let _ = sender.send(full);
                            gazetteer.forget_if_over(MOST_CACHED);
                        }
                    },
                );
                let _ = sender.send(batch);
                kept
            });
            kept
        });
        let mut done = 0usize;
        for batch in receiver {
            let map = interner.absorb(&batch.interner);
            for (tokens, labels) in batch.examples {
                let tokens: Vec<Encoded> = tokens.iter().map(|t| t.remap(&map)).collect();
                buckets.add(&tokens, &labels)?;
                done += 1;
                if done % 1_000_000 == 0 {
                    eprintln!(
                        "  featurized {done} of {training} examples, {} features, {:?}",
                        interner.len(),
                        start.elapsed()
                    );
                }
            }
        }
        let kept = workers.join().expect("featurizing panicked");
        let mut per_source = vec![Vec::new(); sources.len()];
        for (range, kept) in ranges.iter().zip(kept) {
            per_source[range.source].extend(kept);
        }
        Ok(per_source)
    })?;
    Ok(kept)
}

/// The most held-out examples the scale is fitted on.
const MAX_CALIBRATION: usize = 20_000;

/// Fits the model's scale to the held-out examples (see `crf::fit_scale`),
/// and says how well calibrated it is before and after.
fn calibrate(model: &mut Model, test_set: &[(Vec<TokenFeatures>, Vec<Label>)]) {
    model.set_scale(1.0);
    let every = test_set.len().div_ceil(MAX_CALIBRATION).max(1);
    let labelled: Vec<(Lattice, Vec<Label>)> = test_set
        .iter()
        .filter(|(features, _)| !features.is_empty())
        .step_by(every)
        .map(|(features, truth)| (Lattice::new(&*model, features), truth.clone()))
        .collect();
    let summary = |scale: f64| -> (f64, f64, f64) {
        let (mut loss, mut confidence, mut right) = (0.0, 0.0, 0usize);
        for (lattice, truth) in &labelled {
            let lattice = lattice.scaled(scale);
            loss -= lattice.log_probability(truth);
            let (best, score) = lattice.viterbi();
            confidence += (score - lattice.log_partition()).exp();
            right += usize::from(&best == truth);
        }
        let n = labelled.len().max(1) as f64;
        (loss / n, confidence / n, right as f64 / n)
    };
    let scale = fit_scale(&labelled);
    for (name, s) in [("unscaled", 1.0), ("scaled", scale)] {
        let (loss, confidence, accuracy) = summary(s);
        eprintln!(
            "Calibration, {name}: {:.3} nats a query; best labelling {:.1}% likely on average, right {:.1}% of the time",
            loss,
            100.0 * confidence,
            100.0 * accuracy
        );
    }
    eprintln!(
        "Scale: {scale:.4}, fitted on {} held-out queries",
        labelled.len()
    );
    model.set_scale(scale);
}

/// Trains a model on examples made from the indexes, streaming them: one
/// pass makes them to count their words, another makes them again and
/// writes their features as numbers to buckets on disk (under `work`,
/// removed after), which each epoch reads back a bucket at a time. Both
/// passes make examples on every core.
fn train(
    indexes: Vec<PathBuf>,
    out: PathBuf,
    stride: u32,
    epochs: usize,
    copies: usize,
    min_updates: u32,
    seed: u64,
) -> Result<()> {
    let start = Instant::now();
    let sources = sources(&indexes, stride, seed, start)?;
    let ranges = ranges(&sources);

    let (counts, held) = count(&sources, &ranges, copies, seed);
    let unaligned = airmail::tagger_data::unaligned();
    eprintln!("{unaligned} formatted examples left out, their words not all traced to their parts");
    eprintln!(
        "{} training examples, {} held out, in {:?}",
        counts.training,
        held.iter().sum::<usize>(),
        start.elapsed()
    );
    counts.description.print();
    let vocabulary = counts.vocabulary;

    let work = out.with_extension("examples");
    let mut buckets = Buckets::create(&work, counts.training, seed)?;
    let lexicon = Lexicon::new();
    let extractor = Extractor {
        lexicon: &lexicon,
        vocabulary: &vocabulary,
    };
    let steps = held_out_steps(&sources, &ranges, &held);
    let mut interner = Interner::default();
    let held_out = encode_all(
        &sources,
        &ranges,
        &steps,
        copies,
        seed,
        &extractor,
        &mut interner,
        &mut buckets,
        counts.training,
        start,
    )?;
    let paths = buckets.finish()?;
    eprintln!("{} distinct features", interner.len());
    eprintln!("Features in {:?}", start.elapsed());

    let mut test_set = Vec::new();
    for (source, held) in sources.iter().zip(&held_out) {
        test_set.extend(featurize(&source.index, &lexicon, &vocabulary, held));
    }
    let held_out: Vec<Example> = held_out.into_iter().flatten().collect();
    drop(sources);

    let mut trainer = Trainer::new(Tag::names(), min_updates);
    let mut rng = StdRng::seed_from_u64(seed);
    let mut order: Vec<usize> = (0..paths.len()).collect();
    for epoch in 0..epochs {
        order.shuffle(&mut rng);
        let (mut right, mut seen) = (0, 0);
        for &b in &order {
            let mut examples = read_bucket(&paths[b])?;
            examples.shuffle(&mut rng);
            seen += examples.len();
            right += examples
                .iter()
                .filter(|(tokens, labels)| trainer.train(tokens, labels))
                .count();
        }
        eprintln!(
            "Epoch {epoch}: {:.1}% of training examples right before updating, {:?}",
            100.0 * right as f64 / seen.max(1) as f64,
            start.elapsed()
        );
    }
    std::fs::remove_dir_all(&work)?;
    let mut model = trainer.finish(&interner);
    eprintln!(
        "Model: {:?} state and state-transition features",
        model.num_features()
    );
    // Written before calibrating too, so nothing after training can lose it.
    let known: Vec<(String, u32)> = vocabulary
        .known()
        .map(|(w, c)| (w.to_string(), c))
        .collect();
    model.write(&mut BufWriter::new(File::create(&out)?), &known)?;
    calibrate(&mut model, &test_set);
    report(&model, &test_set, &held_out);
    model.write(&mut BufWriter::new(File::create(&out)?), &known)?;
    eprintln!("Wrote {} in {:?}", out.display(), start.elapsed());
    Ok(())
}

/// What the examples are like: their shapes and what they're of.
#[derive(Default)]
struct Description {
    shapes: HashMap<&'static str, usize>,
    subjects: HashMap<&'static str, usize>,
    examples: usize,
}

impl Description {
    fn add(&mut self, example: &Example) {
        self.examples += 1;
        let has_area = example.tags.iter().any(|t| t.band().is_some());
        let subject = example
            .tags
            .iter()
            .find(|t| t.band().is_none() && **t != Tag::Near && **t != Tag::Other);
        let shape = match (subject.is_some(), has_area) {
            (true, false) => "bare",
            (true, true) => "with areas",
            (false, _) => "areas only",
        };
        *self.shapes.entry(shape).or_default() += 1;
        *self
            .subjects
            .entry(subject.map_or("area", |t| t.as_str()))
            .or_default() += 1;
        if example.query.typing {
            *self.shapes.entry("being typed").or_default() += 1;
        }
        if example.query.section_starts.iter().any(|s| *s) {
            *self.shapes.entry("with commas").or_default() += 1;
        }
    }

    fn merge(&mut self, other: Description) {
        self.examples += other.examples;
        for (k, n) in other.shapes {
            *self.shapes.entry(k).or_default() += n;
        }
        for (k, n) in other.subjects {
            *self.subjects.entry(k).or_default() += n;
        }
    }

    fn print(&self) {
        let share = |n: usize| 100.0 * n as f64 / self.examples.max(1) as f64;
        let mut shapes: Vec<_> = self.shapes.iter().collect();
        shapes.sort();
        for (shape, n) in shapes {
            eprintln!("  {shape:<12} {:5.1}%", share(*n));
        }
        let mut subjects: Vec<_> = self.subjects.iter().collect();
        subjects.sort_by(|a, b| b.1.cmp(a.1));
        eprintln!(
            "  by subject: {}",
            subjects
                .iter()
                .map(|(s, n)| format!("{s} {:.1}%", share(**n)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Held-out accuracy: by word and whole query, and by tag.
fn report(model: &Model, test_set: &[(Vec<TokenFeatures>, Vec<Label>)], examples: &[Example]) {
    let (mut words, mut right_words, mut right_queries) = (0usize, 0usize, 0usize);
    // Per tag: (true positives, guessed, actual).
    let mut counts: HashMap<Tag, (usize, usize, usize)> = HashMap::new();
    let mut wrong = Vec::new();
    for ((features, truth), example) in test_set.iter().zip(examples) {
        let (guess, _) = model.tag(features);
        words += truth.len();
        let right = guess.iter().zip(truth).filter(|(g, t)| g == t).count();
        right_words += right;
        if right == truth.len() {
            right_queries += 1;
        } else if wrong.len() < 25 {
            wrong.push((example.query.words.join(" "), guess.clone(), truth.clone()));
        }
        for (g, t) in guess.iter().zip(truth) {
            let (gt, tt) = (Tag::of_label(*g), Tag::of_label(*t));
            counts.entry(gt).or_default().1 += 1;
            counts.entry(tt).or_default().2 += 1;
            if g == t {
                counts.entry(tt).or_default().0 += 1;
            }
        }
    }
    eprintln!(
        "Held out: {:.2}% of words, {:.2}% of queries right",
        100.0 * right_words as f64 / words.max(1) as f64,
        100.0 * right_queries as f64 / test_set.len().max(1) as f64
    );
    for tag in Tag::ALL {
        let (tp, guessed, actual) = counts.get(&tag).copied().unwrap_or_default();
        if actual == 0 && guessed == 0 {
            continue;
        }
        let precision = tp as f64 / guessed.max(1) as f64;
        let recall = tp as f64 / actual.max(1) as f64;
        eprintln!(
            "  {:<14} precision {:.3} recall {:.3} ({actual} words)",
            tag.as_str(),
            precision,
            recall
        );
    }
    for (query, guess, truth) in wrong {
        let show = |labels: &[Label]| {
            labels
                .iter()
                .map(|l| Tag::of_label(*l).as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        eprintln!(
            "  wrong: {query:?}\n    got  {}\n    want {}",
            show(&guess),
            show(&truth)
        );
    }
}

fn parse_focus(focus: &str) -> Option<[f64; 2]> {
    let (lon, lat) = focus.split_once(',')?;
    Some([lon.trim().parse().ok()?, lat.trim().parse().ok()?])
}

fn tag(
    index: PathBuf,
    model: PathBuf,
    typing: bool,
    focus: Option<String>,
    text: Option<String>,
) -> Result<()> {
    let index = AirmailIndex::open_read_only(index.to_str().expect("path"))?;
    let (model, known) = Model::read(&mut BufReader::new(File::open(model)?))?;
    let vocabulary = Vocabulary::from_counts(known.into_iter().collect());
    let lexicon = Lexicon::new();
    let mut focus = focus.map(|f| parse_focus(&f).expect("--focus lon,lat"));
    let tag_one =
        |text: &str, typing: bool, focus: Option<[f64; 2]>, areas_too: bool| -> Result<()> {
            let started = Instant::now();
            let query = query_of(text, typing, focus);
            let gazetteer = IndexGazetteer::new(&index)?;
            let areas = gazetteer.areas(&query);
            let features = Extractor {
                lexicon: &lexicon,
                vocabulary: &vocabulary,
            }
            .features(&query, &areas);
            let (labels, _) = model.tag(&features);
            let marginals = model.marginals(&features);
            let best = model.best(&features, 3);
            let elapsed = started.elapsed();
            for ((word, label), probabilities) in query.words.iter().zip(labels).zip(marginals) {
                let p = probabilities[usize::from(label)];
                println!("  {word:<20} {:<14} {p:.3}", Tag::of_label(label).as_str());
            }
            let lattice = airmail_tagger::crf::Lattice::new(&model, &features);
            for (path, probability) in best {
                let tags: Vec<&str> = path.iter().map(|l| Tag::of_label(*l).as_str()).collect();
                println!(
                    "  {probability:.3}  (score {:.1})  {}",
                    lattice.path_score(&path),
                    tags.join(" ")
                );
            }
            if areas_too {
                for area in &areas {
                    let mut km = area
                        .distance_km
                        .map_or(String::new(), |km| format!(", {km:.0} km away"));
                    if !area.inside.is_empty() {
                        let parents: Vec<String> = area
                            .inside
                            .iter()
                            .map(|(s, e, band)| {
                                format!("{band} \"{}\"", query.words[*s..*e].join(" "))
                            })
                            .collect();
                        km.push_str(&format!(", in the {}", parents.join(" and the ")));
                    }
                    println!(
                        "  gazetteer: \"{}\" {} ({:?}, importance {:.2}{km})",
                        query.words[area.start..area.end].join(" "),
                        area.band,
                        area.quality,
                        area.importance
                    );
                }
                println!("  ({elapsed:.1?})");
            }
            Ok(())
        };
    if let Some(text) = text {
        return tag_one(&text, typing, focus, false);
    }
    repl(typing, &mut focus, tag_one)
}

/// Tags queries a line at a time. `:typing` turns treating the last word as
/// unfinished on and off, `:focus lon,lat` sets the focus and `:focus`
/// clears it.
fn repl(
    mut typing: bool,
    focus: &mut Option<[f64; 2]>,
    tag_one: impl Fn(&str, bool, Option<[f64; 2]>, bool) -> Result<()>,
) -> Result<()> {
    eprintln!(
        "Type a query; :typing toggles the last word being unfinished, :focus lon,lat sets a focus (:focus clears it), Ctrl-D quits."
    );
    let mut editor = rustyline::DefaultEditor::new()?;
    loop {
        let mode = match (typing, *focus) {
            (true, Some([lon, lat])) => format!("typing, near {lon},{lat}"),
            (true, None) => "typing".to_string(),
            (false, Some([lon, lat])) => format!("near {lon},{lat}"),
            (false, None) => String::new(),
        };
        let prompt = if mode.is_empty() {
            "> ".to_string()
        } else {
            format!("[{mode}] > ")
        };
        let line = match editor.readline(&prompt) {
            Ok(line) => line,
            Err(
                rustyline::error::ReadlineError::Interrupted | rustyline::error::ReadlineError::Eof,
            ) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        if !line.trim().is_empty() {
            editor.add_history_entry(line.as_str())?;
        }
        let line = line.trim();
        if line == ":typing" {
            typing = !typing;
        } else if let Some(rest) = line.strip_prefix(":focus") {
            let rest = rest.trim();
            if rest.is_empty() {
                *focus = None;
            } else if let Some(parsed) = parse_focus(rest) {
                *focus = Some(parsed);
            } else {
                eprintln!("  :focus takes lon,lat");
            }
        } else if !line.is_empty() {
            tag_one(line, typing, *focus, true)?;
        }
    }
}

/// A model and what it needs to tag queries.
struct Tagger {
    index: AirmailIndex,
    model: Model,
    vocabulary: Vocabulary,
    lexicon: Lexicon,
}

impl Tagger {
    fn open(index: &std::path::Path, model: &std::path::Path) -> Result<Self> {
        let index = AirmailIndex::open_read_only(index.to_str().expect("path"))?;
        let (model, known) = Model::read(&mut BufReader::new(File::open(model)?))?;
        Ok(Self {
            index,
            model,
            vocabulary: Vocabulary::from_counts(known.into_iter().collect()),
            lexicon: Lexicon::new(),
        })
    }

    fn features(&self, query: &Query) -> Result<Vec<TokenFeatures>> {
        let areas = IndexGazetteer::new(&self.index)?.areas(query);
        Ok(Extractor {
            lexicon: &self.lexicon,
            vocabulary: &self.vocabulary,
        }
        .features(query, &areas))
    }
}

fn eval(index: PathBuf, model: PathBuf, tsv: PathBuf) -> Result<()> {
    let tagger = Tagger::open(&index, &model)?;
    let (mut queries, mut right_queries, mut words, mut right_words) = (0, 0, 0, 0);
    for line in std::fs::read_to_string(&tsv)?.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut columns = line.split('\t');
        let (Some(text), Some(flags), Some(tags)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };
        let query = query_of(text, flags.contains("typing"), None);
        let want: Vec<Tag> = tags
            .split_whitespace()
            .map(|t| Tag::parse(t).unwrap_or_else(|| panic!("tag {t}")))
            .collect();
        if want.len() != query.words.len() {
            eprintln!(
                "skipping {text:?}: {} words, {} tags",
                query.words.len(),
                want.len()
            );
            continue;
        }
        let (got, _) = tagger.model.tag(&tagger.features(&query)?);
        let got: Vec<Tag> = got.into_iter().map(Tag::of_label).collect();
        let right = got.iter().zip(&want).filter(|(g, w)| g == w).count();
        queries += 1;
        words += want.len();
        right_words += right;
        if right == want.len() {
            right_queries += 1;
        } else {
            let show = |tags: &[Tag]| {
                tags.iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            println!(
                "WRONG {text:?}\n  got  {}\n  want {}",
                show(&got),
                show(&want)
            );
        }
    }
    println!("{right_queries}/{queries} queries, {right_words}/{words} words right");
    Ok(())
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::Train {
            index,
            out,
            stride,
            epochs,
            copies,
            min_updates,
            seed,
        } => train(index, out, stride, epochs, copies, min_updates, seed),
        Command::Tag {
            index,
            model,
            typing,
            focus,
            query,
        } => tag(index, model, typing, focus, query),
        Command::Eval { index, model, tsv } => eval(index, model, tsv),
        Command::Score {
            index,
            model,
            stride,
            copies,
            seed,
            max,
            tsv,
            json,
        } => score::score(
            &index,
            &model,
            stride,
            copies,
            seed,
            max,
            tsv.as_deref(),
            json.as_deref(),
        ),
        Command::Usage {
            index,
            model,
            out,
            stride,
            copies,
            seed,
            every,
        } => score::usage(&index, &model, stride, copies, seed, every, &out),
        Command::Calibrate {
            index,
            model,
            out,
            stride,
            copies,
            seed,
        } => {
            let start = Instant::now();
            let (mut model, known) = Model::read(&mut BufReader::new(File::open(model)?))?;
            let vocabulary = Vocabulary::from_counts(known.iter().cloned().collect());
            let lexicon = Lexicon::new();
            let mut test_set = Vec::new();
            let sources = sources(&index, stride, seed, start)?;
            for (source, held) in sources.iter().zip(held_out(&sources, copies, seed)) {
                test_set.extend(featurize(&source.index, &lexicon, &vocabulary, &held));
            }
            calibrate(&mut model, &test_set);
            model.write(&mut BufWriter::new(File::create(out)?), &known)?;
            Ok(())
        }
        Command::Examples {
            index,
            stride,
            seed,
        } => {
            let sources = sources(&[index], stride, seed, Instant::now())?;
            let ranges = ranges(&sources);
            let (_, examples) = by_range(
                &ranges,
                || Generator::new(seed),
                |generator, range| {
                    let mut examples = Vec::new();
                    range_examples(generator, &sources, range, 1, seed, |example, _| {
                        examples.push(example)
                    });
                    examples
                },
            );
            for example in examples.iter().flatten() {
                let words: Vec<String> = example
                    .query
                    .words
                    .iter()
                    .zip(&example.tags)
                    .zip(&example.query.section_starts)
                    .map(|((w, t), comma)| {
                        format!("{}{w}/{}", if *comma { ", " } else { "" }, t.as_str())
                    })
                    .collect();
                println!(
                    "{}{}",
                    words.join(" "),
                    if example.query.typing {
                        "  (typing)"
                    } else {
                        ""
                    }
                );
            }
            Ok(())
        }
        Command::Convert {
            model,
            out,
            min_weight,
        } => {
            let (model, extra) = Model::read(&mut BufReader::new(File::open(model)?))?;
            let mut magnitudes = model.weight_magnitudes();
            magnitudes.sort_by(f32::total_cmp);
            let quantile = |q: f64| {
                magnitudes
                    .get(((magnitudes.len() as f64 - 1.0) * q) as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            eprintln!(
                "{:?} features, {} weights; magnitudes at 10%, 25%, 50%, 75%, 90%: {:.4} {:.4} {:.4} {:.4} {:.4}",
                model.num_features(),
                magnitudes.len(),
                quantile(0.1),
                quantile(0.25),
                quantile(0.5),
                quantile(0.75),
                quantile(0.9)
            );
            let model = match min_weight {
                Some(min) => {
                    let pruned = model.pruned(min);
                    eprintln!(
                        "Pruned below {min}: {:?} features, {} weights",
                        pruned.num_features(),
                        pruned.weight_magnitudes().len()
                    );
                    pruned
                }
                None => model,
            };
            model.write(&mut BufWriter::new(File::create(out)?), &extra)?;
            Ok(())
        }
    }
}
