//! Export training data for the categorical search model from an OSM PBF file.
//! See `airmail_indexer::categories_export` and `airmail/categorical/README.md`.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use airmail_indexer::{
    ImporterBuilder,
    categories_export::CategoriesExportSink,
    osm_pbf::{self, OsmPbf, ParseOsmTypes},
};
use anyhow::Result;
use clap::Parser;
use env_logger::Env;
use futures_util::future::join_all;
use log::warn;
use tokio::{spawn, task::spawn_blocking};

#[derive(Debug, Parser)]
struct Args {
    /// Who's On First Spatialite database, for the languages spoken where each POI is
    #[clap(long, short)]
    wof_db: PathBuf,

    /// Where the coordinates of the ways multipolygons are made of are kept while parsing, as
    /// for `indexer`
    #[clap(long, short)]
    admin_cache: Option<PathBuf>,

    /// Who's On First point-in-polygon tree cache, as for `indexer`
    #[clap(long, short)]
    pip_tree: Option<PathBuf>,

    /// OSM PBF file to read, with node locations on its ways
    /// (`osmium add-locations-to-ways --keep-member-nodes`)
    pbf: PathBuf,

    /// JSON lines output
    out: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    osm_pbf::require_locations_on_ways(&args.pbf)?;
    let sink = CategoriesExportSink::new(&args.out)?;
    let mut builder = ImporterBuilder::new(&args.wof_db)?;
    if let Some(admin_cache) = &args.admin_cache {
        builder = builder.admin_cache(admin_cache);
    }
    if let Some(pip_tree) = &args.pip_tree {
        builder = builder.pip_tree_cache(pip_tree);
    }
    let importer = builder.build().await?;

    let (poi_sender, poi_receiver) = crossbeam::channel::bounded(16384);
    let indexer_cache = importer.indexer_cache();
    let parse = spawn_blocking(move || {
        OsmPbf::new(
            &args.pbf,
            Vec::<ParseOsmTypes>::new(),
            poi_sender,
            indexer_cache,
        )
        .parse_osm()
        .inspect_err(|e| warn!("Error parsing OSM: {e}"))
    });
    let import = spawn(async move { importer.run_import(poi_receiver, sink).await });

    for result in join_all([parse, import]).await {
        result??;
    }
    Ok(())
}
