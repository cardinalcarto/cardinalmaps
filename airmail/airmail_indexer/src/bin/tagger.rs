//! Trains and tries the query tagger.
//!
//! ```text
//! tagger train --index <dir> --out <model> [--stride N] [--epochs N] [--copies N]
//! tagger tag --index <dir> --model <model> [--typing] [--focus lon,lat] [query]
//! tagger eval --index <dir> --model <model> <tsv>
//! tagger convert <model> <out>
//! tagger calibrate --index <dir> --model <model> --out <model> [--stride N] [--copies N] [--seed N]
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
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::time::Instant;

use airmail::gazetteer::IndexGazetteer;
use airmail::index::AirmailIndex;
use airmail::tagger_data::{Example, Generator, sample};
use airmail_tagger::features::{Extractor, Gazetteer, Query, Vocabulary};
use airmail_tagger::labels::Tag;
use airmail_tagger::lexicon::Lexicon;
use airmail_tagger::text::{fold, words};
use airmail_tagger::crf::{Lattice, fit_scale};
use airmail_tagger::{Encoded, Interner, Label, Model, TokenFeatures, Trainer};
use anyhow::Result;
use clap::{Parser, Subcommand};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;

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
    /// Rewrites a model in the current format.
    Convert { model: PathBuf, out: PathBuf },
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
    let mut query = Query { typing, focus, ..Query::default() };
    for (i, section) in text.split(',').enumerate() {
        let folded = fold(section);
        for (j, (_, word)) in words(&folded).into_iter().enumerate() {
            query.section_starts.push(i > 0 && j == 0);
            query.words.push(word.to_string());
        }
    }
    query
}

/// Features of examples as numbers, computed in parallel, each thread
/// numbering its own and then all renumbered into one interner.
fn encode(
    index: &AirmailIndex,
    lexicon: &Lexicon,
    vocabulary: &Vocabulary,
    examples: &[Example],
    interner: &mut Interner,
) -> Vec<(Vec<Encoded>, Vec<Label>)> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = examples.len().div_ceil(threads).max(1);
    let parts: Vec<(Interner, Vec<(Vec<Encoded>, Vec<Label>)>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = examples
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let gazetteer = IndexGazetteer::new(index).expect("searcher");
                    let extractor = Extractor { lexicon, vocabulary };
                    let mut local = Interner::default();
                    let encoded = chunk
                        .iter()
                        .map(|example| {
                            let areas = gazetteer.areas(&example.query);
                            let features = extractor.features(&example.query, &areas);
                            let tokens = features.iter().map(|f| local.encode(f)).collect();
                            (tokens, example.tags.iter().map(|t| t.label()).collect())
                        })
                        .collect::<Vec<_>>();
                    (local, encoded)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("encode")).collect()
    });
    let mut all = Vec::new();
    for (local, encoded) in parts {
        let map = interner.absorb(&local);
        all.extend(encoded.into_iter().map(|(tokens, labels)| (tokens.iter().map(|t| t.remap(&map)).collect(), labels)));
    }
    all
}

/// Features of examples, computed in parallel.
fn featurize(index: &AirmailIndex, lexicon: &Lexicon, vocabulary: &Vocabulary, examples: &[Example]) -> Vec<(Vec<TokenFeatures>, Vec<Label>)> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = examples.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = examples
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let gazetteer = IndexGazetteer::new(index).expect("searcher");
                    let extractor = Extractor { lexicon, vocabulary };
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
        handles.into_iter().flat_map(|h| h.join().expect("featurize")).collect()
    })
}

/// The most held-out examples to measure on.
const MAX_HELD_OUT: usize = 100_000;

/// Each index, with its training and held-out examples: the same for the
/// same arguments, so `calibrate` can remake what `train` held out.
fn make_sets(indexes: &[PathBuf], stride: u32, copies: usize, seed: u64, start: Instant) -> Result<Vec<(AirmailIndex, Vec<Example>, Vec<Example>)>> {
    let mut generator = Generator::new(seed);
    // Per index: the index, its training and held-out examples.
    let mut sets = Vec::new();
    for path in indexes {
        let index = AirmailIndex::open_read_only(path.to_str().expect("path"))?;
        let (docs, admins) = sample(&index, stride, seed as u32)?;
        eprintln!("Sampled {} documents of {} in {:?}", docs.len(), path.display(), start.elapsed());
        let (mut training, mut held_out) = (Vec::new(), Vec::new());
        for (i, hit) in docs.iter().enumerate() {
            // Somewhere else, for a focus far away.
            let elsewhere = docs[(i * 7919 + 13) % docs.len()].center;
            for _ in 0..copies * airmail::tagger_data::copies(hit.kind) {
                let examples = generator.examples(hit, &admins, [elsewhere.x(), elsewhere.y()]);
                if i % 10 == 0 { held_out.extend(examples) } else { training.extend(examples) }
            }
        }
        // Enough to measure by, without holding every held-out example's
        // features as strings.
        let keep = MAX_HELD_OUT / indexes.len();
        if held_out.len() > keep {
            let every = held_out.len().div_ceil(keep);
            held_out = held_out.into_iter().step_by(every).collect();
        }
        sets.push((index, training, held_out));
    }
    eprintln!("{} formatted examples left out, their words not all traced to their parts", airmail::tagger_data::unaligned());
    Ok(sets)
}

/// The most held-out examples the scale is fitted on.
const MAX_CALIBRATION: usize = 20_000;

/// Fits the model's scale to the held-out examples (see `crf::fit_scale`),
/// and says how well calibrated it is before and after.
fn calibrate(model: &mut Model, test_set: &[(Vec<TokenFeatures>, Vec<Label>)]) {
    model.set_scale(1.0);
    let every = test_set.len().div_ceil(MAX_CALIBRATION).max(1);
    let labelled: Vec<(Lattice, Vec<Label>)> =
        test_set.iter().step_by(every).map(|(features, truth)| (Lattice::new(&*model, features), truth.clone())).collect();
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
    eprintln!("Scale: {scale:.4}, fitted on {} held-out queries", labelled.len());
    model.set_scale(scale);
}

fn train(indexes: Vec<PathBuf>, out: PathBuf, stride: u32, epochs: usize, copies: usize, min_updates: u32, seed: u64) -> Result<()> {
    let start = Instant::now();
    let sets = make_sets(&indexes, stride, copies, seed, start)?;
    let training: Vec<&Example> = sets.iter().flat_map(|(_, t, _)| t).collect();
    let held_out: Vec<Example> = sets.iter().flat_map(|(_, _, h)| h.iter().cloned()).collect();
    eprintln!("{} training and {} held-out examples", training.len(), held_out.len());
    describe(&training);

    let mut vocabulary = Vocabulary::default();
    for example in &training {
        for word in &example.query.words {
            vocabulary.add(word);
        }
    }
    drop(training);
    let lexicon = Lexicon::new();
    let (mut train_set, mut test_set) = (Vec::new(), Vec::new());
    let mut interner = Interner::default();
    for (index, training, held_out) in &sets {
        train_set.extend(encode(index, &lexicon, &vocabulary, training, &mut interner));
        test_set.extend(featurize(index, &lexicon, &vocabulary, held_out));
    }
    eprintln!("{} distinct features", interner.len());
    eprintln!("Features in {:?}", start.elapsed());

    let mut trainer = Trainer::new(Tag::names(), min_updates);
    let mut order: Vec<usize> = (0..train_set.len()).collect();
    let mut rng = StdRng::seed_from_u64(seed);
    for epoch in 0..epochs {
        order.shuffle(&mut rng);
        let right = order.iter().filter(|&&i| trainer.train(&train_set[i].0, &train_set[i].1)).count();
        eprintln!("Epoch {epoch}: {:.1}% of training examples right before updating, {:?}", 100.0 * right as f64 / order.len() as f64, start.elapsed());
    }
    let mut model = trainer.finish(&interner);
    eprintln!("Model: {:?} state and state-transition features", model.num_features());
    calibrate(&mut model, &test_set);
    report(&model, &test_set, &held_out);

    let known: Vec<(String, u32)> = vocabulary.known().map(|(w, c)| (w.to_string(), c)).collect();
    model.write(&mut BufWriter::new(File::create(&out)?), &known)?;
    eprintln!("Wrote {} in {:?}", out.display(), start.elapsed());
    Ok(())
}

/// What the examples are like: their shapes and what they're of.
fn describe(examples: &[&Example]) {
    let mut shapes: HashMap<&str, usize> = HashMap::new();
    let mut subjects: HashMap<&str, usize> = HashMap::new();
    for example in examples {
        let has_area = example.tags.iter().any(|t| t.band().is_some());
        let subject = example.tags.iter().find(|t| t.band().is_none() && **t != Tag::Near && **t != Tag::Other);
        let shape = match (subject.is_some(), has_area) {
            (true, false) => "bare",
            (true, true) => "with areas",
            (false, _) => "areas only",
        };
        *shapes.entry(shape).or_default() += 1;
        *subjects.entry(subject.map_or("area", |t| t.as_str())).or_default() += 1;
        if example.query.typing {
            *shapes.entry("being typed").or_default() += 1;
        }
        if example.query.section_starts.iter().any(|s| *s) {
            *shapes.entry("with commas").or_default() += 1;
        }
    }
    let share = |n: usize| 100.0 * n as f64 / examples.len().max(1) as f64;
    let mut shapes: Vec<_> = shapes.into_iter().collect();
    shapes.sort();
    for (shape, n) in shapes {
        eprintln!("  {shape:<12} {:5.1}%", share(n));
    }
    let mut subjects: Vec<_> = subjects.into_iter().collect();
    subjects.sort_by(|a, b| b.1.cmp(&a.1));
    eprintln!("  by subject: {}", subjects.iter().map(|(s, n)| format!("{s} {:.1}%", share(*n))).collect::<Vec<_>>().join(", "));
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
        eprintln!("  {:<14} precision {:.3} recall {:.3} ({actual} words)", tag.as_str(), precision, recall);
    }
    for (query, guess, truth) in wrong {
        let show = |labels: &[Label]| labels.iter().map(|l| Tag::of_label(*l).as_str()).collect::<Vec<_>>().join(" ");
        eprintln!("  wrong: {query:?}\n    got  {}\n    want {}", show(&guess), show(&truth));
    }
}

fn parse_focus(focus: &str) -> Option<[f64; 2]> {
    let (lon, lat) = focus.split_once(',')?;
    Some([lon.trim().parse().ok()?, lat.trim().parse().ok()?])
}

fn tag(index: PathBuf, model: PathBuf, typing: bool, focus: Option<String>, text: Option<String>) -> Result<()> {
    let index = AirmailIndex::open_read_only(index.to_str().expect("path"))?;
    let (model, known) = Model::read(&mut BufReader::new(File::open(model)?))?;
    let vocabulary = Vocabulary::from_counts(known.into_iter().collect());
    let lexicon = Lexicon::new();
    let mut focus = focus.map(|f| parse_focus(&f).expect("--focus lon,lat"));
    let tag_one = |text: &str, typing: bool, focus: Option<[f64; 2]>, areas_too: bool| -> Result<()> {
        let started = Instant::now();
        let query = query_of(text, typing, focus);
        let gazetteer = IndexGazetteer::new(&index)?;
        let areas = gazetteer.areas(&query);
        let features = Extractor { lexicon: &lexicon, vocabulary: &vocabulary }.features(&query, &areas);
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
            println!("  {probability:.3}  (score {:.1})  {}", lattice.path_score(&path), tags.join(" "));
        }
        if areas_too {
            for area in &areas {
                let km = area.distance_km.map_or(String::new(), |km| format!(", {km:.0} km away"));
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
    eprintln!("Type a query; :typing toggles the last word being unfinished, :focus lon,lat sets a focus (:focus clears it), Ctrl-D quits.");
    let mut editor = rustyline::DefaultEditor::new()?;
    loop {
        let mode = match (typing, *focus) {
            (true, Some([lon, lat])) => format!("typing, near {lon},{lat}"),
            (true, None) => "typing".to_string(),
            (false, Some([lon, lat])) => format!("near {lon},{lat}"),
            (false, None) => String::new(),
        };
        let prompt = if mode.is_empty() { "> ".to_string() } else { format!("[{mode}] > ") };
        let line = match editor.readline(&prompt) {
            Ok(line) => line,
            Err(rustyline::error::ReadlineError::Interrupted | rustyline::error::ReadlineError::Eof) => return Ok(()),
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
        Ok(Self { index, model, vocabulary: Vocabulary::from_counts(known.into_iter().collect()), lexicon: Lexicon::new() })
    }

    fn features(&self, query: &Query) -> Result<Vec<TokenFeatures>> {
        let areas = IndexGazetteer::new(&self.index)?.areas(query);
        Ok(Extractor { lexicon: &self.lexicon, vocabulary: &self.vocabulary }.features(query, &areas))
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
        let (Some(text), Some(flags), Some(tags)) = (columns.next(), columns.next(), columns.next()) else { continue };
        let query = query_of(text, flags.contains("typing"), None);
        let want: Vec<Tag> = tags.split_whitespace().map(|t| Tag::parse(t).unwrap_or_else(|| panic!("tag {t}"))).collect();
        if want.len() != query.words.len() {
            eprintln!("skipping {text:?}: {} words, {} tags", query.words.len(), want.len());
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
            let show = |tags: &[Tag]| tags.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(" ");
            println!("WRONG {text:?}\n  got  {}\n  want {}", show(&got), show(&want));
        }
    }
    println!("{right_queries}/{queries} queries, {right_words}/{words} words right");
    Ok(())
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::Train { index, out, stride, epochs, copies, min_updates, seed } => {
            train(index, out, stride, epochs, copies, min_updates, seed)
        }
        Command::Tag { index, model, typing, focus, query } => tag(index, model, typing, focus, query),
        Command::Eval { index, model, tsv } => eval(index, model, tsv),
        Command::Calibrate { index, model, out, stride, copies, seed } => {
            let start = Instant::now();
            let (mut model, known) = Model::read(&mut BufReader::new(File::open(model)?))?;
            let vocabulary = Vocabulary::from_counts(known.iter().cloned().collect());
            let lexicon = Lexicon::new();
            let mut test_set = Vec::new();
            for (index, _, held_out) in make_sets(&index, stride, copies, seed, start)? {
                test_set.extend(featurize(&index, &lexicon, &vocabulary, &held_out));
            }
            calibrate(&mut model, &test_set);
            model.write(&mut BufWriter::new(File::create(out)?), &known)?;
            Ok(())
        }
        Command::Examples { index, stride, seed } => {
            let sets = make_sets(&[index], stride, 1, seed, Instant::now())?;
            for (_, training, held_out) in &sets {
                for example in training.iter().chain(held_out) {
                    let words: Vec<String> = example
                        .query
                        .words
                        .iter()
                        .zip(&example.tags)
                        .zip(&example.query.section_starts)
                        .map(|((w, t), comma)| format!("{}{w}/{}", if *comma { ", " } else { "" }, t.as_str()))
                        .collect();
                    println!("{}{}", words.join(" "), if example.query.typing { "  (typing)" } else { "" });
                }
            }
            Ok(())
        }
        Command::Convert { model, out } => {
            let (model, extra) = Model::read(&mut BufReader::new(File::open(model)?))?;
            model.write(&mut BufWriter::new(File::create(out)?), &extra)?;
            Ok(())
        }
    }
}
