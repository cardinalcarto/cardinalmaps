//! Shrinks query tagger models (`tagger.model`) for phones: drops the
//! weights that move label scores least, and the vocabulary words no
//! feature needs, then packs what's left (see `airmail_tagger::model`).
//!
//! ```text
//! tagger_prune stats <model>
//! tagger_prune prune <model> <out> [--min-weight W] [--usage <tsv> [--min-count N] [--min-impact X]]
//!     [--bits 8] [--transition-bits 8] [--keep-vocabulary]
//! ```

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use airmail_tagger::Model;
use airmail_tagger::model::{Bits, Row};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use fst::MapBuilder;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What takes the space: features, weights and their bytes by kind.
    Stats { model: PathBuf },
    /// Writes a smaller model in the packed format.
    Prune {
        model: PathBuf,
        out: PathBuf,
        /// Drop weights smaller in magnitude than this (before the model's
        /// scale), and features left without any.
        #[arg(long, default_value_t = 0.0)]
        min_weight: f32,
        /// How often features fired on training queries, from `tagger usage`.
        #[arg(long)]
        usage: Option<PathBuf>,
        /// Drop features that fired fewer times than this (with `--usage`).
        #[arg(long, default_value_t = 0.0)]
        min_count: f32,
        /// Drop weights whose magnitude times how often their feature fired
        /// is less than this (with `--usage`).
        #[arg(long, default_value_t = 0.0)]
        min_impact: f32,
        /// Bits a state weight: 16 (bfloat16), 8 or 4 (codes into a fitted
        /// table of values).
        #[arg(long, default_value_t = 8)]
        bits: u8,
        /// Bits a state-transition weight.
        #[arg(long, default_value_t = 8)]
        transition_bits: u8,
        /// Keep every vocabulary word, even those no feature left names.
        #[arg(long)]
        keep_vocabulary: bool,
    },
}

/// What a feature is, for counting: its template, the part before its
/// first value (`word`, `prev+word`, `gaz`), and the length of n-grams.
fn kind(feature: &str) -> String {
    let mut parts = feature.split('|');
    let first = parts.next().unwrap_or_default();
    match (first, parts.next()) {
        ("ngram", Some(n)) => format!("ngram|{n}"),
        _ => first.to_string(),
    }
}

fn fst_bytes(rows: &[&Row]) -> usize {
    let mut keys: Vec<&str> = rows.iter().map(|(f, _)| f.as_str()).collect();
    keys.sort_unstable();
    let mut builder = MapBuilder::memory();
    for (i, key) in keys.iter().enumerate() {
        builder.insert(key, i as u64).expect("sorted keys");
    }
    builder.into_inner().expect("an FST").len()
}

#[allow(clippy::cast_precision_loss)]
fn mb(n: usize) -> f64 {
    n as f64 / 1e6
}

fn read(path: &Path) -> Result<(Model, Vec<(String, u32)>)> {
    Model::read(&mut BufReader::new(File::open(path).with_context(|| format!("opening {}", path.display()))?)).with_context(|| format!("reading {}", path.display()))
}

fn stats(path: &Path) -> Result<()> {
    let (model, vocabulary) = read(path)?;
    let file = std::fs::metadata(path)?.len() as usize;
    println!("{}: {:.2} MB, {}", path.display(), mb(file), if model.is_packed() { "packed" } else { "plain" });
    for ((name, (keys, columns, weights)), features) in ["state", "state-transition"].iter().zip(model.sizes()).zip([model.num_features().0, model.num_features().1]) {
        println!("  {name}: {features} features; strings {:.2} MB, columns {:.2} MB, weights {:.2} MB", mb(keys), mb(columns), mb(weights));
    }
    let (state, transition) = model.rows();
    let rare = vocabulary.iter().filter(|(_, c)| *c <= airmail_tagger::features::RARE_WORD_COUNT).count();
    println!("  vocabulary: {} words ({rare} rare)", vocabulary.len());
    for (name, rows) in [("state", &state), ("state-transition", &transition)] {
        let mut by_kind: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
        for row in rows {
            by_kind.entry(kind(&row.0)).or_default().push(row);
        }
        let mut kinds: Vec<(String, Vec<&Row>)> = by_kind.into_iter().collect();
        kinds.sort_by_key(|(_, rows)| std::cmp::Reverse(rows.len()));
        println!("  {name} features by kind: features, weights, FST of their own MB, median |w|, share of rows with max |w| < 0.5, < 2");
        for (kind, rows) in kinds {
            let mut magnitudes: Vec<f32> = rows.iter().flat_map(|(_, w)| w.iter().map(|(_, v)| v.abs())).collect();
            magnitudes.sort_by(f32::total_cmp);
            let weights = magnitudes.len();
            let max = |row: &&Row| row.1.iter().fold(0f32, |m, (_, v)| m.max(v.abs()));
            #[allow(clippy::cast_precision_loss)]
            let share = |t: f32| rows.iter().filter(|r| max(r) < t).count() as f64 / rows.len() as f64;
            println!(
                "    {kind:<14} {:>9} {:>10} {:>7.2} {:>8.3} {:>6.3} {:>6.3}",
                rows.len(),
                weights,
                mb(fst_bytes(&rows)),
                magnitudes[weights / 2],
                share(0.5),
                share(2.0)
            );
        }
    }
    let mut magnitudes = model.weight_magnitudes();
    magnitudes.sort_by(f32::total_cmp);
    let quantile = |q: f64| magnitudes[((magnitudes.len() - 1) as f64 * q) as usize];
    println!(
        "  |w| at 10/25/50/75/90/99%: {:.3} {:.3} {:.3} {:.3} {:.3} {:.3}; scale {:.4}",
        quantile(0.1),
        quantile(0.25),
        quantile(0.5),
        quantile(0.75),
        quantile(0.9),
        quantile(0.99),
        model.scale()
    );
    Ok(())
}

/// The values features name, which are vocabulary words where a word is
/// known: every `|`-separated part of every feature.
fn named(rows: &[Row]) -> HashSet<&str> {
    rows.iter().flat_map(|(f, _)| f.split('|')).collect()
}

/// How often features fired, from `tagger usage`: state and
/// state-transition counts by feature.
type Usage = (HashMap<String, u64>, HashMap<String, u64>);

fn read_usage(path: &Path) -> Result<Usage> {
    let (mut state, mut transition) = (HashMap::new(), HashMap::new());
    for line in BufReader::new(File::open(path).with_context(|| format!("opening {}", path.display()))?).lines() {
        let line = line?;
        if line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(kind), Some(count), Some(feature)) = (parts.next(), parts.next(), parts.next()) else { bail!("bad usage line {line:?}") };
        let map = if kind == "s" { &mut state } else { &mut transition };
        map.insert(feature.to_string(), count.parse()?);
    }
    Ok((state, transition))
}

/// Rows without their weights of less than `min_weight`, and, with usage
/// counts, without the features that fired fewer than `min_count` times or
/// whose weights times their count are less than `min_impact`.
fn kept(rows: Vec<Row>, settings: &Settings, usage: Option<&HashMap<String, u64>>) -> Vec<Row> {
    rows.into_iter()
        .filter_map(|(feature, weights)| {
            #[allow(clippy::cast_precision_loss)]
            let count = usage.map(|u| u.get(&feature).copied().unwrap_or(0) as f32);
            if count.is_some_and(|c| c < settings.min_count) {
                return None;
            }
            let weights: Vec<(u32, f32)> = weights
                .into_iter()
                .filter(|(_, w)| w.abs() >= settings.min_weight && count.is_none_or(|c| c * w.abs() >= settings.min_impact))
                .collect();
            (!weights.is_empty()).then_some((feature, weights))
        })
        .collect()
}

struct Settings {
    min_weight: f32,
    min_count: f32,
    min_impact: f32,
    bits: Bits,
    transition_bits: Bits,
    keep_vocabulary: bool,
}

fn prune(path: &Path, out: &Path, settings: &Settings, usage: Option<&Usage>) -> Result<()> {
    let (model, vocabulary) = read(path)?;
    let (state, transition) = model.rows();
    let state = kept(state, settings, usage.map(|u| &u.0));
    let transition = kept(transition, settings, usage.map(|u| &u.1));
    let pruned = model.with_rows(state, transition).packed(settings.bits, settings.transition_bits);
    let vocabulary: Vec<(String, u32)> = if settings.keep_vocabulary {
        vocabulary
    } else {
        let (state, transition) = pruned.rows();
        let (a, b) = (named(&state), named(&transition));
        vocabulary.into_iter().filter(|(w, _)| a.contains(w.as_str()) || b.contains(w.as_str())).collect()
    };
    let mut bytes = Vec::new();
    pruned.write(&mut bytes, &vocabulary)?;
    // What was written must read back as the same model.
    let (again, _) = Model::read(&mut bytes.as_slice()).context("re-reading the pruned model")?;
    if again.num_features() != pruned.num_features() {
        bail!("the pruned model reads back differently");
    }
    BufWriter::new(File::create(out).with_context(|| format!("writing {}", out.display()))?).write_all(&bytes)?;
    let before = std::fs::metadata(path)?.len() as usize;
    println!(
        "{}: {:.1} MB -> {:.1} MB ({:?} features, {} weights, {} vocabulary words)",
        out.display(),
        mb(before),
        mb(bytes.len()),
        pruned.num_features(),
        pruned.weight_magnitudes().len(),
        vocabulary.len()
    );
    Ok(())
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::Stats { model } => stats(&model),
        Command::Prune { model, out, min_weight, usage, min_count, min_impact, bits, transition_bits, keep_vocabulary } => {
            let (Some(bits), Some(transition_bits)) = (Bits::of(bits), Bits::of(transition_bits)) else { bail!("bits are 16, 8 or 4") };
            if usage.is_none() && (min_count > 0.0 || min_impact > 0.0) {
                bail!("--min-count and --min-impact need --usage");
            }
            let usage = usage.map(|u| read_usage(&u)).transpose()?;
            let settings = Settings { min_weight, min_count, min_impact, bits, transition_bits, keep_vocabulary };
            prune(&model, &out, &settings, usage.as_ref())
        }
    }
}
