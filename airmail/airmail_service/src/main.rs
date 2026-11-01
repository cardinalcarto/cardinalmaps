#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::future::IntoFuture;
use std::sync::Arc;

use std::path::PathBuf;

use airmail::categories::{CategoryModels, DEFAULT_INCLUDE_THRESHOLD};
use airmail::index::AirmailIndex;
use anyhow::Result;
use api::{AppState, search};
use axum::{Router, http::HeaderValue, routing::get};
use clap::Parser;
use env_logger::Env;
use log::{debug, info, warn};
use tokio::net::TcpListener;
use tokio::select;
use tower_http::cors::CorsLayer;

mod api;
mod error;

#[derive(Debug, Parser)]
struct Args {
    /// The path to the index to load
    #[arg(short, long, env = "AIRMAIL_INDEX")]
    index: String,

    /// The address to bind to
    #[arg(short, long, env = "AIRMAIL_BIND", default_value = "127.0.0.1:3000")]
    bind: String,

    /// Categorical search models: a model file or a directory of them, one
    /// per language. Requests choose one with `lang`.
    #[arg(long, env = "AIRMAIL_CATEGORIES")]
    categories: Option<PathBuf>,

    /// Minimum model confidence for search to also return places that match a
    /// query only by category; below it, categories only rerank word matches
    #[arg(long, env = "AIRMAIL_CATEGORY_INCLUDE_THRESHOLD", default_value_t = DEFAULT_INCLUDE_THRESHOLD)]
    category_include_threshold: f32,

    /// Cors origins to allow
    #[arg(
        short,
        long,
        env = "AIRMAIL_CORS",
        default_value = "http://localhost:5173"
    )]
    cors: Option<Vec<String>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    debug!("Loading index from {}", args.index);
    let index = AirmailIndex::new(&args.index)?;
    let categories = match &args.categories {
        Some(path) => {
            let models = CategoryModels::open(path)?;
            info!(
                "Loaded categorical search models from {}: {}",
                path.display(),
                models.languages().join(", ")
            );
            models
        }
        None => CategoryModels::default(),
    };
    let state = Arc::new(AppState {
        index,
        categories,
        category_include_threshold: args.category_include_threshold,
    });

    let mut cors = CorsLayer::new();
    for origin in args.cors.unwrap_or_default() {
        cors = cors.allow_origin(origin.parse::<HeaderValue>()?);
    }

    info!("Loaded {} docs from index", state.index.num_docs().await?);
    let app = Router::new()
        .route("/search", get(search).with_state(state))
        .layer(cors);

    info!("Listening at: {}/search?q=query", args.bind);
    let listener = TcpListener::bind(args.bind).await?;
    let server = axum::serve(listener, app.into_make_service()).into_future();

    select! {
        _ = server => {}
        _ = tokio::signal::ctrl_c() => {
            warn!("Received ctrl-c, shutting down");
        }
    }

    Ok(())
}
