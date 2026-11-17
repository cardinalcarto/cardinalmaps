//! Tracks OSM POIs and gives each one an identifier that survives edits but
//! changes when an element is reused for a different place.
//!
//!     poi_service bootstrap --planet planet-261005.osm.pbf
//!     poi_service update --follow
//!     poi_service patch --input planet.osm.pbf --output planet-with-ids.osm.pbf \
//!         --locations planet-low.osm.pbf --locations-output planet-low-with-ids.osm.pbf \
//!         --qrank qrank.csv.gz
//!     poi_service checkpoint
//!     poi_service covers --input extract.osm.pbf
//!     poi_service export --staging poi-staging
//!     poi_service fetch --url https://example.org/releases --dir poi-releases
//!     poi_service import --releases poi-releases
//!     poi_service status --releases poi-releases --output poi-status.bin
//!
//! `patch` and `covers` read published releases instead of PostgreSQL when
//! given `--releases`.

mod bootstrap;
mod db;
mod export;
mod fetch;
mod identity;
mod import;
mod importance;
mod osm;
mod patch;
mod pbf;
mod published;
mod replication;
mod rules;
mod tables;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sqlx::PgPool;

use crate::db::Checkpoint;
use crate::identity::Checksum;
use crate::published::Releases;
use crate::replication::Server;
use crate::rules::RULESET_VERSION;

#[derive(Parser)]
#[command(about)]
struct Cli {
    /// The identity database. Commands given `--releases` don't need one.
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,
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
    /// `cardinal:version`, and given `--locations`, every POI with its
    /// importance as airmail ranks it, as `cardinal:importance`.
    Patch {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// The same extract with node locations on its ways (`osmium
        /// add-locations-to-ways`), for POIs' shapes, which their importance
        /// depends on.
        #[arg(long)]
        locations: Option<PathBuf>,
        /// Also write a patched copy of `--locations` here.
        #[arg(long, requires = "locations")]
        locations_output: Option<PathBuf>,
        /// QRank (`qrank.csv.gz` from https://qrank.wmcloud.org), which
        /// ranks POIs with Wikidata entities by their articles' page views.
        /// Without it, having an entity counts the same for every POI.
        #[arg(long, requires = "locations")]
        qrank: Option<PathBuf>,
        /// Read identifiers from published releases in this directory (as
        /// `fetch` or `export` writes them) instead of the database. The
        /// output is the same.
        #[arg(long)]
        releases: Option<PathBuf>,
        /// With `--releases`, the checkpoint to use (default: the newest).
        #[arg(long, requires = "releases")]
        sequence: Option<i64>,
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
        /// Check the newest release in this directory instead of the database.
        #[arg(long)]
        releases: Option<PathBuf>,
    },
    /// Write a release for every checkpoint not yet in `--staging`, and a
    /// snapshot of the latest when asked or when the newest snapshot there
    /// is a week older, for `cardinal-publish poi` to upload.
    Export {
        #[arg(long)]
        staging: PathBuf,
        /// Write a snapshot of the latest checkpoint whatever the age of the
        /// newest one.
        #[arg(long)]
        snapshot: bool,
        #[arg(long, default_value_t = 7)]
        snapshot_days: i64,
    },
    /// Restore an empty database from published releases, or bring one up to
    /// date with them, checking the checksum chain at every checkpoint.
    Import {
        #[arg(long)]
        releases: PathBuf,
        /// The checkpoint to stop at (default: the newest).
        #[arg(long)]
        sequence: Option<i64>,
    },
    /// Download the releases that reach a checkpoint (default: the one
    /// `latest.json` names) into a directory.
    Fetch {
        /// The bucket's releases prefix over HTTPS, holding latest.json.
        #[arg(long)]
        url: String,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        sequence: Option<i64>,
    },
    /// Write the geocoder's status file for a checkpoint (default: the
    /// newest) from published releases, replacing `--output` by a rename.
    Status {
        #[arg(long)]
        releases: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        sequence: Option<i64>,
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
    let connect = || async {
        let url = cli.database_url.as_deref().context("this needs --database-url or DATABASE_URL")?;
        db::connect(url).await
    };
    match cli.command {
        Command::Patch { input, output, locations, locations_output, qrank, releases: Some(releases), sequence } => {
            let releases = Releases::new(&releases);
            let (ids, checkpoint) = tokio::task::block_in_place(|| import::load_ids(&releases, sequence))?;
            let importances = load_importances(locations.as_deref(), qrank.as_deref())?;
            patch::patch(ids, Some(checkpoint), importances, patch_files(input, output, locations, locations_output))
                .await
        }
        Command::Covers { input, releases: Some(releases) } => {
            covers(&input, &import::latest_checkpoint(&Releases::new(&releases))?)
        }
        Command::Export { staging, snapshot, snapshot_days } => {
            export::export(&connect().await?, &staging, snapshot, snapshot_days).await
        }
        Command::Import { releases, sequence } => {
            import::import(&connect().await?, &Releases::new(&releases), sequence).await
        }
        Command::Fetch { url, dir, sequence } => {
            let sequence = fetch::fetch(&url, &dir, sequence).await?;
            println!("{sequence}");
            Ok(())
        }
        Command::Status { releases, output, sequence } => {
            let checkpoint = import::write_status(&Releases::new(&releases), sequence, &output)?;
            println!("{} {}", checkpoint.sequence, checkpoint.timestamp.to_rfc3339());
            Ok(())
        }
        command => database_command(&connect().await?, command).await,
    }
}

async fn database_command(pool: &PgPool, command: Command) -> Result<()> {
    match command {
        Command::Bootstrap { planet, replication_url } => {
            bootstrap::bootstrap(pool, &planet, &Server::new(&replication_url), &replication_url).await
        }
        Command::Update { follow, poll_seconds, max_diffs } => {
            update(pool, follow, Duration::from_secs(poll_seconds), max_diffs.unwrap_or(usize::MAX)).await
        }
        Command::Patch { input, output, locations, locations_output, qrank, .. } => {
            let checkpoint = db::latest_checkpoint(pool).await?;
            let ids = patch::PoiIds::load(pool).await?;
            let importances = load_importances(locations.as_deref(), qrank.as_deref())?;
            patch::patch(ids, checkpoint, importances, patch_files(input, output, locations, locations_output)).await
        }
        Command::Checkpoint => {
            let checkpoint = db::latest_checkpoint(pool).await?.context("no checkpoint")?;
            println!("{} {}", checkpoint.sequence, hex::encode(&checkpoint.checksum));
            Ok(())
        }
        Command::Covers { input, .. } => covers(&input, &db::latest_checkpoint(pool).await?.context("no checkpoint")?),
        _ => unreachable!("handled in main"),
    }
}

/// Every POI's importance in `locations`, if given.
fn load_importances(
    locations: Option<&std::path::Path>,
    qrank: Option<&std::path::Path>,
) -> Result<Option<importance::Importances>> {
    let Some(locations) = locations else {
        return Ok(None);
    };
    tokio::task::block_in_place(|| {
        let qrank = qrank.map(airmail_importance::qrank::QRank::load).transpose()?;
        if let Some(qrank) = &qrank {
            tracing::info!("loaded QRank for {} Wikidata entities", qrank.len());
        }
        let importances = importance::Importances::compute(locations, qrank.as_ref())?;
        tracing::info!("computed the importance of {} POIs", importances.len());
        Ok(Some(importances))
    })
}

/// The files `patch` writes: `output` from `input`, and `locations_output`
/// from `locations` if asked for.
fn patch_files(
    input: PathBuf,
    output: PathBuf,
    locations: Option<PathBuf>,
    locations_output: Option<PathBuf>,
) -> Vec<(PathBuf, PathBuf)> {
    let mut files = vec![(input, output)];
    if let (Some(locations), Some(locations_output)) = (locations, locations_output) {
        files.push((locations, locations_output));
    }
    files
}

/// Prints the file's timestamp and the checkpoint's sequence and timestamp,
/// failing if the checkpoint is older than the file.
fn covers(input: &std::path::Path, checkpoint: &Checkpoint) -> Result<()> {
    let file = bootstrap::replication_timestamp(input)?;
    println!("{} {} {}", file.to_rfc3339(), checkpoint.sequence, checkpoint.timestamp.to_rfc3339());
    if checkpoint.timestamp < file {
        bail!(
            "the identifiers are at {} ({}), behind {} ({file})",
            checkpoint.sequence,
            checkpoint.timestamp,
            input.display()
        );
    }
    Ok(())
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
    let started = std::time::Instant::now();
    let state = server.state(Some(sequence)).await?;
    let changes = server.diff(sequence).await?;
    let downloaded = started.elapsed();
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
    db::store(pool, &applied.updates, &applied.versions, &checkpoint, None).await?;
    tracing::info!(
        "applied {sequence} ({}): {} elements changed, {} POI versions, {} new identities; checksum {}; \
         {:.1} s downloading and reading, {:.1} s applying",
        state.timestamp,
        applied.updates.len(),
        applied.versions.len(),
        applied.new_identities,
        hex::encode(&checkpoint.checksum),
        downloaded.as_secs_f64(),
        (started.elapsed() - downloaded).as_secs_f64(),
    );
    Ok(checkpoint)
}
