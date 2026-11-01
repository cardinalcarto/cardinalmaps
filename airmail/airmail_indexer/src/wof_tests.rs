use anyhow::Result;
use env_logger::Env;
use std::path::PathBuf;

use log::debug;

const DEFAULT_LOG_LEVEL: &str = "debug,sqlx=info";
const DEFAULT_WOF_DB: &str = "../data/whosonfirst-data-admin-latest.spatial.db";
const DEFAULT_PIP_TREE: &str = "../data/pip_tree.bin";

/// These tests need a Who's On First database covering Australia, and
/// SpatiaLite. Run them with `cargo test -- --ignored`, setting
/// `AIRMAIL_TEST_WOF_DB` and `AIRMAIL_TEST_PIP_TREE` to override the paths.
fn test_path(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map_or_else(|| PathBuf::from(default), PathBuf::from)
}

use crate::{
    pip_tree::PipTree,
    wof::{ConcisePipResponse, PipLangsResponse, WhosOnFirst},
};

/// Connect to WOF and perform some queries
#[tokio::test]
#[ignore = "needs a Who's On First database, see test_path"]
async fn wof_read() -> Result<()> {
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or(DEFAULT_LOG_LEVEL))
        .is_test(true)
        .try_init();

    // Connect to the WhosOnFirst database.
    // Ensuring the database is present, and mod_spatialite extension is loaded.
    let wof = WhosOnFirst::new(&test_path("AIRMAIL_TEST_WOF_DB", DEFAULT_WOF_DB)).await?;

    // Test point_in_polygon.
    // This should exist in Global and Australia.
    let pip = wof
        .point_in_polygon(150.93800658307805, -33.88246407738443)
        .await?;
    debug!("point_in_polygon: {:?}", pip);
    assert!(!pip.is_empty());

    // Lookup a place by ID, this should return the locality of Sydney
    let place_name = wof.place_name_by_id(101932003).await?;
    debug!("place_name: {:?}", place_name);
    assert!(
        place_name
            .iter()
            .any(|name| name.lang == "eng" && name.name == "Sydney")
    );

    // Lookup a country
    let country: PipLangsResponse = wof.properties_for_id(85632793).await?.into();
    debug!("country: {:?}", country);

    // Ensure eng is in the languages
    assert!(country.langs.unwrap().contains("eng"));

    Ok(())
}

/// Connect to WOF, create a PipTree, and perform some queries
#[tokio::test]
#[ignore = "needs a Who's On First database, see test_path"]
async fn wof_pip_tree() -> Result<()> {
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or(DEFAULT_LOG_LEVEL))
        .is_test(true)
        .try_init();

    // Connect to the WhosOnFirst database.
    // Ensuring the database is present, and mod_spatialite extension is loaded.
    let wof = WhosOnFirst::new(&test_path("AIRMAIL_TEST_WOF_DB", DEFAULT_WOF_DB)).await?;

    // Create a PipTree from the WhosOnFirst database.
    let pip_tree = PipTree::<ConcisePipResponse>::new_or_load(
        &wof,
        &test_path("AIRMAIL_TEST_PIP_TREE", DEFAULT_PIP_TREE),
    )
    .await?;
    debug!("Tree size: {}", pip_tree.len());
    assert!(!pip_tree.is_empty());

    // Should not match (coords are backwards)
    let invalid_pip = pip_tree
        .point_in_polygon(-33.88246407738443, 150.93800658307805)
        .await?;
    debug!("invalid_pip: {:?}", invalid_pip);
    assert!(invalid_pip.is_empty());

    let pip_from_tree = pip_tree
        .point_in_polygon(150.93800658307805, -33.88246407738443)
        .await?;
    debug!("pip_from_tree: {:?}", pip_from_tree);
    assert!(!pip_from_tree.is_empty());

    // Ensure the PipTree matches the database
    let pip_from_db = wof
        .point_in_polygon(150.93800658307805, -33.88246407738443)
        .await?;
    debug!("pip_from_db: {:?}", pip_from_db);
    let mut tree_ids: Vec<_> = pip_from_tree.into_iter().map(|p| p.id).collect();
    let mut db_ids: Vec<_> = pip_from_db.into_iter().map(|p| p.id).collect();
    tree_ids.sort();
    db_ids.sort();
    assert_eq!(tree_ids, db_ids);

    Ok(())
}
