//! Tracks OSM POIs and gives each one an identifier that survives edits but
//! changes when an element is reused for a different place.
//!
//!     poi_service bootstrap --planet planet-261005.osm.pbf
//!     poi_service update --follow
//!     poi_service patch --input planet.osm.pbf --output planet-with-ids.osm.pbf
//!     poi_service checkpoint
//!     poi_service covers --input extract.osm.pbf

mod bootstrap;
mod db;
mod identity;
mod osm;
mod patch;
mod pbf;
mod replication;
mod rules;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sqlx::PgPool;

use crate::db::Checkpoint;
use crate::identity::Checksum;
use crate::replication::Server;
use crate::rules::RULESET_VERSION;

#[derive(Parser)]
#[command(about)]
struct Cli {
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Load every POI from a planet file into an empty database.
    Bootstrap {
        #[arg(long)]
        planet: PathBuf,
        /// Daily replication diffs following the planet. Every deployment that
        /// should agree on identifiers must use the same planet and diffs.
        #[arg(long, default_value = replication::DEFAULT_URL)]
        replication_url: String,
    },
    /// Apply replication diffs until caught up.
    Update {
        /// Keep checking for new diffs once caught up.
        #[arg(long)]
        follow: bool,
        #[arg(long, default_value_t = 600)]
        poll_seconds: u64,
        /// Stop after applying this many diffs.
        #[arg(long)]
        max_diffs: Option<usize>,
    },
    /// Copy an .osm.pbf file, tagging every POI version with a known
    /// identifier with `cardinal:poi_id` and its OSM version with
    /// `cardinal:version`.
    Patch {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Print the latest checkpoint's sequence and checksum, which together
    /// identify every identifier `patch` can add.
    Checkpoint,
    /// Check that the database has applied diffs up to an .osm.pbf file's
    /// replication timestamp, so `patch` can identify all of its POIs. Prints
    /// the file's timestamp, then the latest checkpoint's sequence and
    /// timestamp, and fails if the database is behind.
    Covers {
        #[arg(long)]
        input: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    let pool = db::connect(&cli.database_url).await?;
    match cli.command {
        Command::Bootstrap { planet, replication_url } => {
            bootstrap::bootstrap(&pool, &planet, &Server::new(&replication_url), &replication_url).await
        }
        Command::Update { follow, poll_seconds, max_diffs } => {
            update(&pool, follow, Duration::from_secs(poll_seconds), max_diffs.unwrap_or(usize::MAX)).await
        }
        Command::Patch { input, output } => patch::patch(&pool, &input, &output).await,
        Command::Checkpoint => {
            let checkpoint = db::latest_checkpoint(&pool).await?.context("no checkpoint")?;
            println!("{} {}", checkpoint.sequence, hex::encode(&checkpoint.checksum));
            Ok(())
        }
        Command::Covers { input } => {
            let file = bootstrap::replication_timestamp(&input)?;
            let checkpoint = db::latest_checkpoint(&pool).await?.context("no checkpoint")?;
            println!("{} {} {}", file.to_rfc3339(), checkpoint.sequence, checkpoint.timestamp.to_rfc3339());
            if checkpoint.timestamp < file {
                bail!(
                    "the database is at {} ({}), behind {} ({file})",
                    checkpoint.sequence,
                    checkpoint.timestamp,
                    input.display()
                );
            }
            Ok(())
        }
    }
}

async fn update(pool: &PgPool, follow: bool, poll: Duration, max_diffs: usize) -> Result<()> {
    let origin = db::origin(pool).await?.context("the database isn't bootstrapped")?;
    if origin.ruleset != RULESET_VERSION {
        bail!("the database uses ruleset {}, but this build has ruleset {RULESET_VERSION}", origin.ruleset);
    }
    tracing::info!("identifiers from the planet at {} with ruleset {RULESET_VERSION}", origin.planet_timestamp);
    let server = Server::new(&origin.replication_url);
    let mut checkpoint = db::latest_checkpoint(pool).await?.context("no checkpoint")?;
    let mut applied = 0;
    loop {
        let latest = server.state(None).await?;
        while checkpoint.sequence < latest.sequence && applied < max_diffs {
            checkpoint = apply_diff(pool, &server, &checkpoint).await?;
            applied += 1;
        }
        if !follow || applied >= max_diffs {
            return Ok(());
        }
        tokio::time::sleep(poll).await;
    }
}

async fn apply_diff(pool: &PgPool, server: &Server, previous: &Checkpoint) -> Result<Checkpoint> {
    let sequence = previous.sequence + 1;
    let state = server.state(Some(sequence)).await?;
    let changes = server.diff(sequence).await?;
    let keys: HashSet<_> = changes.iter().map(|c| c.key).collect();
    let tracked = db::tracked(pool, keys.into_iter()).await?;
    let applied = identity::apply(&tracked, changes, previous.next_poi_id);

    let mut checksum = Checksum::new(&previous.checksum);
    for row in &applied.versions {
        checksum.add(row);
    }
    let checkpoint = Checkpoint {
        sequence,
        timestamp: state.timestamp,
        next_poi_id: previous.next_poi_id + applied.new_identities as i64,
        checksum: checksum.finish(),
    };
    db::store(pool, &applied.updates, &applied.versions, &checkpoint).await?;
    tracing::info!(
        "applied {sequence} ({}): {} elements changed, {} POI versions, {} new identities; checksum {}",
        state.timestamp,
        applied.updates.len(),
        applied.versions.len(),
        applied.new_identities,
        hex::encode(&checkpoint.checksum),
    );
    Ok(checkpoint)
}
