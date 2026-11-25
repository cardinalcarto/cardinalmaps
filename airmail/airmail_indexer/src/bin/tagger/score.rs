//! Comparing models on the same queries (`tagger score`), and counting how
//! often a model's features fire on training queries (`tagger usage`), for
//! `tagger_prune`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use airmail::gazetteer::IndexGazetteer;
use airmail::index::AirmailIndex;
use airmail::tagger_data::{Example, Generator};
use airmail_tagger::features::{AreaMatch, Extractor, Gazetteer, Query, Vocabulary};
use airmail_tagger::labels::Tag;
use airmail_tagger::lexicon::Lexicon;
use airmail_tagger::{Label, Model};
use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{by_range, held_out, query_of, range_examples, ranges, sources};

/// `f` over `items` in chunks, a thread a core, results in order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&[T]) -> Vec<R> + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let chunk = items.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items.chunks(chunk).map(|c| scope.spawn(|| f(c))).collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a thread panicked"))
            .collect()
    })
}

/// Each query's gazetteer matches in an index.
fn areas(index: &AirmailIndex, queries: &[&Query]) -> Vec<Vec<AreaMatch>> {
    parallel(queries, |chunk| {
        let gazetteer = IndexGazetteer::new(index).expect("searcher");
        chunk.iter().map(|q| gazetteer.areas(q)).collect()
    })
}

/// A labelled query and its gazetteer matches, which don't depend on the
/// model.
struct Case {
    query: Query,
    truth: Vec<Tag>,
    areas: Vec<Vec<AreaMatch>>,
}

fn read_model(path: &Path) -> Result<(Model, Vocabulary)> {
    let (model, known) = Model::read(&mut BufReader::new(
        File::open(path).with_context(|| format!("opening {}", path.display()))?,
    ))?;
    Ok((model, Vocabulary::from_counts(known.into_iter().collect())))
}

/// The labels a model gives each case.
fn tag_all(
    model: &Model,
    vocabulary: &Vocabulary,
    lexicon: &Lexicon,
    cases: &[Case],
) -> Vec<Vec<Label>> {
    parallel(cases, |chunk| {
        let extractor = Extractor {
            lexicon,
            vocabulary,
        };
        chunk
            .iter()
            .map(|case| {
                model
                    .tag(&extractor.features(&case.query, &case.areas[0]))
                    .0
            })
            .collect()
    })
}

/// How right labels are: by word and query, and by tag.
fn metrics(cases: &[Case], guesses: &[Vec<Label>], reference: Option<&[Vec<Label>]>) -> Value {
    let (mut words, mut right_words, mut right_queries) = (0usize, 0usize, 0usize);
    // Per tag: right, guessed, actual words; queries with it and right.
    let mut tags: HashMap<Tag, [usize; 5]> = HashMap::new();
    for (case, guess) in cases.iter().zip(guesses) {
        let guess: Vec<Tag> = guess.iter().map(|l| Tag::of_label(*l)).collect();
        words += case.truth.len();
        let right = guess
            .iter()
            .zip(&case.truth)
            .filter(|(g, t)| g == t)
            .count();
        right_words += right;
        let all_right = right == case.truth.len();
        right_queries += usize::from(all_right);
        for (g, t) in guess.iter().zip(&case.truth) {
            tags.entry(*g).or_default()[1] += 1;
            tags.entry(*t).or_default()[2] += 1;
            if g == t {
                tags.entry(*t).or_default()[0] += 1;
            }
        }
        let mut present: Vec<Tag> = case.truth.clone();
        present.sort();
        present.dedup();
        for t in present {
            let entry = tags.entry(t).or_default();
            entry[3] += 1;
            entry[4] += usize::from(all_right);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let share = |a: usize, b: usize| if b == 0 { 0.0 } else { a as f64 / b as f64 };
    let mut by_tag = serde_json::Map::new();
    for tag in Tag::ALL {
        let [right, guessed, actual, queries, queries_right] =
            tags.get(&tag).copied().unwrap_or_default();
        if actual == 0 && guessed == 0 {
            continue;
        }
        let (p, r) = (share(right, guessed), share(right, actual));
        by_tag.insert(
            tag.as_str().to_string(),
            json!({
                "precision": p, "recall": r, "f1": if p + r > 0.0 { 2.0 * p * r / (p + r) } else { 0.0 },
                "words": actual, "queries": queries, "queries_right": share(queries_right, queries),
            }),
        );
    }
    let agreement = reference.map(|reference| {
        share(
            guesses
                .iter()
                .zip(reference)
                .filter(|(a, b)| a == b)
                .count(),
            cases.len(),
        )
    });
    json!({
        "queries": cases.len(), "words": words,
        "word_accuracy": share(right_words, words), "query_accuracy": share(right_queries, cases.len()),
        "same_as_first": agreement, "by_tag": by_tag,
    })
}

/// Hand-labelled queries: query, flags, tags (see `eval/tagger.tsv`).
fn tsv_cases(path: &Path) -> Result<Vec<(Query, Vec<Tag>)>> {
    let mut cases = Vec::new();
    for line in std::fs::read_to_string(path)?.lines() {
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
        if want.len() == query.words.len() {
            cases.push((query, want));
        }
    }
    Ok(cases)
}

/// Scores each model on the queries held out of training on the indexes
/// (as `train` holds them out with the same stride, copies and seed) and on
/// hand-labelled queries, with the first index as their gazetteer.
#[allow(clippy::too_many_arguments)]
pub fn score(
    indexes: &[PathBuf],
    models: &[PathBuf],
    stride: u32,
    copies: usize,
    seed: u64,
    max: usize,
    tsv: Option<&Path>,
    out: Option<&Path>,
) -> Result<()> {
    let start = Instant::now();
    let sources = sources(indexes, stride, seed, start)?;
    let mut sets: Vec<(String, Vec<Case>)> = Vec::new();
    let held = held_out(&sources, copies, seed);
    for (path, (source, examples)) in indexes.iter().zip(sources.iter().zip(held)) {
        let every = examples.len().div_ceil(max).max(1);
        let examples: Vec<Example> = examples.into_iter().step_by(every).collect();
        let queries: Vec<&Query> = examples.iter().map(|e| &e.query).collect();
        let found = areas(&source.index, &queries);
        let cases = examples
            .into_iter()
            .zip(found)
            .map(|(e, a)| Case {
                query: e.query,
                truth: e.tags,
                areas: vec![a],
            })
            .collect();
        eprintln!(
            "Held-out queries of {} in {:?}",
            path.display(),
            start.elapsed()
        );
        sets.push((format!("held_out:{}", path.display()), cases));
    }
    if let Some(tsv) = tsv {
        let labelled = tsv_cases(tsv)?;
        let queries: Vec<&Query> = labelled.iter().map(|(q, _)| q).collect();
        let found = areas(&sources[0].index, &queries);
        let cases = labelled
            .into_iter()
            .zip(found)
            .map(|((query, truth), a)| Case {
                query,
                truth,
                areas: vec![a],
            })
            .collect();
        sets.push((format!("labelled:{}", tsv.display()), cases));
    }
    let lexicon = Lexicon::new();
    let mut first: Vec<Vec<Vec<Label>>> = Vec::new();
    let mut results = Vec::new();
    for (m, path) in models.iter().enumerate() {
        let (model, vocabulary) = read_model(path)?;
        let size = std::fs::metadata(path)?.len();
        let mut per_set = serde_json::Map::new();
        for (s, (name, cases)) in sets.iter().enumerate() {
            let guesses = tag_all(&model, &vocabulary, &lexicon, cases);
            let result = metrics(cases, &guesses, first.get(s).map(Vec::as_slice));
            eprintln!(
                "{} {name}: words {:.4} queries {:.4} same {:?}",
                path.display(),
                result["word_accuracy"].as_f64().unwrap_or_default(),
                result["query_accuracy"].as_f64().unwrap_or_default(),
                result["same_as_first"].as_f64()
            );
            per_set.insert(name.clone(), result);
            if m == 0 {
                first.push(guesses);
            }
        }
        results
            .push(json!({ "model": path.display().to_string(), "bytes": size, "sets": per_set }));
    }
    let report = serde_json::to_string_pretty(&results)?;
    match out {
        Some(out) => std::fs::write(out, report)?,
        None => println!("{report}"),
    }
    Ok(())
}

/// Counts how often each of a model's features fires on every `every`th
/// training query (as `train` makes them),
/// and writes `s` or `t` (state or state-transition), the count and the
/// feature, a line each, for the features that fired.
pub fn usage(
    indexes: &[PathBuf],
    model: &Path,
    stride: u32,
    copies: usize,
    seed: u64,
    every: usize,
    out: &Path,
) -> Result<()> {
    let start = Instant::now();
    let (model, vocabulary) = read_model(model)?;
    let (state, transition) = model.rows();
    let ids = |rows: &[airmail_tagger::model::Row]| -> HashMap<String, usize> {
        rows.iter()
            .enumerate()
            .map(|(i, (f, _))| (f.clone(), i))
            .collect()
    };
    let (state_ids, transition_ids) = (ids(&state), ids(&transition));
    let sources = sources(indexes, stride, seed, start)?;
    let chosen = ranges(&sources);
    let every = every.max(1);
    let lexicon = Lexicon::new();
    // State, state-transition and example counts, and examples seen.
    type Counts = (Vec<u32>, Vec<u32>, usize, usize);
    let (states, _) = by_range(
        &chosen,
        || -> (Generator, Option<(usize, IndexGazetteer)>, Counts) {
            (
                Generator::new(seed),
                None,
                (vec![0; state.len()], vec![0; transition.len()], 0, 0),
            )
        },
        |(generator, gazetteer, (s_counts, t_counts, examples, seen)), range| {
            if gazetteer.as_ref().is_none_or(|(s, _)| *s != range.source) {
                *gazetteer = Some((
                    range.source,
                    IndexGazetteer::new(&sources[range.source].index).expect("searcher"),
                ));
            }
            let gazetteer = &gazetteer.as_ref().expect("a gazetteer").1;
            let extractor = Extractor {
                lexicon: &lexicon,
                vocabulary: &vocabulary,
            };
            range_examples(
                generator,
                &sources,
                range,
                copies,
                seed,
                |example, held_out| {
                    if held_out {
                        return;
                    }
                    *seen += 1;
                    if *seen % every != 0 {
                        return;
                    }
                    *examples += 1;
                    let areas = gazetteer.areas(&example.query);
                    for (t, token) in extractor
                        .features(&example.query, &areas)
                        .iter()
                        .enumerate()
                    {
                        for f in &token.state {
                            if let Some(i) = state_ids.get(f) {
                                s_counts[*i] += 1;
                            }
                        }
                        if t > 0 {
                            for f in &token.transition {
                                if let Some(i) = transition_ids.get(f) {
                                    t_counts[*i] += 1;
                                }
                            }
                        }
                    }
                },
            );
            gazetteer.forget_if_over(2_000_000);
        },
    );
    let (mut s_total, mut t_total, mut examples) =
        (vec![0u64; state.len()], vec![0u64; transition.len()], 0);
    for (_, _, (s, t, n, _)) in states {
        for (a, b) in s_total.iter_mut().zip(s) {
            *a += u64::from(b);
        }
        for (a, b) in t_total.iter_mut().zip(t) {
            *a += u64::from(b);
        }
        examples += n;
    }
    let mut w = BufWriter::new(File::create(out)?);
    writeln!(w, "# {examples} training queries, every {every}th")?;
    for (kind, rows, counts) in [("s", &state, &s_total), ("t", &transition, &t_total)] {
        for ((feature, _), count) in rows.iter().zip(counts) {
            if *count > 0 {
                writeln!(w, "{kind}\t{count}\t{feature}")?;
            }
        }
    }
    w.flush()?;
    let fired = |c: &[u64]| c.iter().filter(|c| **c > 0).count();
    eprintln!(
        "{examples} queries: {} of {} state and {} of {} state-transition features fired, in {:?}",
        fired(&s_total),
        state.len(),
        fired(&t_total),
        transition.len(),
        start.elapsed()
    );
    Ok(())
}
