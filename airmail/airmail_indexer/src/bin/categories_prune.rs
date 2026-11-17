//! Shrink categorical search models (`.amcats`) for phones: fold small heads
//! into label-score space and drop the feature rows that move label scores
//! least. See `airmail::categories::Prune` and `airmail/categorical/README.md`.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use airmail::categories::{CategoryModel, Prune, PruneHead};
use anyhow::{Context, Result, ensure};
use clap::Parser;

#[derive(Debug, Parser)]
struct Args {
    /// A model file, or a directory of them
    input: PathBuf,

    /// The pruned model file, or a directory for them
    output: PathBuf,

    /// Fraction of the categories head's rows to keep
    #[clap(long, default_value_t = 0.8)]
    keep_categories: f64,

    /// Fraction of the cuisine head's rows to keep
    #[clap(long, default_value_t = 0.6)]
    keep_cuisine: f64,

    /// Fraction of the diet head's rows to keep
    #[clap(long, default_value_t = 0.1)]
    keep_diet: f64,

    /// Don't store heads with fewer labels than dimensions as label scores
    #[clap(long)]
    no_fold: bool,
}

fn prune_file(input: &Path, output: &Path, prune: &Prune) -> Result<()> {
    let model = CategoryModel::open(input)?;
    let pruned = model.pruned(prune);
    let bytes = pruned.to_bytes();
    // What we wrote must read back as the same model.
    CategoryModel::from_bytes(&bytes).context("re-reading the pruned model")?;
    std::fs::write(output, &bytes).with_context(|| format!("writing {}", output.display()))?;
    let before = std::fs::metadata(input)?.len();
    #[allow(clippy::cast_precision_loss)]
    let mb = |n: u64| n as f64 / 1e6;
    let heads: Vec<String> = pruned
        .head_sizes()
        .iter()
        .map(|(name, stored, rows, dim)| format!("{name} {stored}/{rows} rows x {dim}"))
        .collect();
    println!(
        "{}: {:.1} MB -> {:.1} MB ({})",
        model.language(),
        mb(before),
        mb(bytes.len() as u64),
        heads.join(", ")
    );
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let head = |keep: f64| -> Result<PruneHead> {
        ensure!(
            keep > 0.0 && keep <= 1.0,
            "keep fractions must be in (0, 1]"
        );
        Ok(PruneHead {
            fold: !args.no_fold,
            keep,
        })
    };
    let prune = Prune {
        categories: head(args.keep_categories)?,
        cuisine: head(args.keep_cuisine)?,
        diet: head(args.keep_diet)?,
    };
    if args.input.is_dir() {
        std::fs::create_dir_all(&args.output)?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(&args.input)?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<_, _>>()?;
        files.retain(|f| f.extension().is_some_and(|e| e == "amcats"));
        files.sort();
        for file in files {
            prune_file(&file, &args.output.join(file.file_name().unwrap()), &prune)?;
        }
    } else {
        prune_file(&args.input, &args.output, &prune)?;
    }
    Ok(())
}
