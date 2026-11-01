use std::sync::Arc;

use airmail::{
    categories::{CategoryModels, QueryCategories},
    index::AirmailIndex,
    poi::AirmailPoi,
};
use anyhow::Result;
use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use deunicode::deunicode;
use geo::{Coord, Rect};
#[cfg(feature = "invasive_logging")]
use log::debug;
use serde::{Deserialize, Serialize};

use crate::error::AirmailServiceError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQueryParams {
    q: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    tags: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    leniency: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    bbox: Option<String>,

    /// The query's language as a BCP-47 tag such as `de-CH`. Categorical
    /// search only runs with a language that has a model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lang: Option<String>,

    /// Overrides the service's `--category-include-threshold`, for tuning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category_include_threshold: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    metadata: MetadataResponse,
    features: Vec<AirmailPoi>,
}

#[derive(Debug, Clone, Serialize)]
struct MetadataResponse {
    query: SearchQueryParams,
    /// ISO 639-3 code of the categorical model used, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    category_language: Option<String>,
    /// What the categorical model understood the query to ask for.
    #[serde(skip_serializing_if = "Option::is_none")]
    categories: Option<QueryCategories>,
}

pub struct AppState {
    pub index: AirmailIndex,
    pub categories: CategoryModels,
    pub category_include_threshold: f32,
}

fn parse_bbox(s: &str) -> Option<Rect> {
    let mut parts = s.split(',');
    let min_lng: f64 = parts.next()?.parse().ok()?;
    let min_lat: f64 = parts.next()?.parse().ok()?;
    let max_lng: f64 = parts.next()?.parse().ok()?;
    let max_lat: f64 = parts.next()?.parse().ok()?;

    Some(Rect::new(
        Coord {
            y: min_lat,
            x: min_lng,
        },
        Coord {
            y: max_lat,
            x: max_lng,
        },
    ))
}

pub async fn search(
    Query(params): Query<SearchQueryParams>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AirmailServiceError> {
    let query = deunicode(params.q.trim()).to_lowercase();
    let tags: Option<Vec<String>> = params
        .tags
        .clone()
        .map(|s| s.split(',').map(std::string::ToString::to_string).collect());
    let leniency = params.leniency.unwrap_or_default();
    let bbox = params.bbox.clone().and_then(|s| parse_bbox(&s));

    // The model normalizes on its own and keeps accents, so it gets the raw query.
    let model = params
        .lang
        .as_deref()
        .and_then(|lang| state.categories.get(lang));
    let categories = model.map(|model| model.understand(&params.q));
    let results = state
        .index
        .search(
            &query,
            leniency,
            tags,
            categories.as_ref().map(|c| {
                let threshold = params
                    .category_include_threshold
                    .unwrap_or(state.category_include_threshold);
                (c, c.mode(threshold))
            }),
            bbox,
            &[],
        )
        .await?;

    #[cfg(feature = "invasive_logging")]
    {
        let start = std::time::Instant::now();
        debug!(
            "Query: {:?} produced: {} results found in {:?}",
            params,
            results.len(),
            start.elapsed()
        );
    }

    let response = Response {
        metadata: MetadataResponse {
            query: params,
            category_language: model.map(|m| m.language().to_string()),
            categories,
        },
        features: results
            .into_iter()
            .map(|(results, _)| results)
            .collect::<Vec<AirmailPoi>>(),
    };

    Ok(Json(serde_json::to_value(response)?))
}
