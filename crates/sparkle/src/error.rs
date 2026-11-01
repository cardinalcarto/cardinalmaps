use mlt_core::MltError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid style JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid style: {0}")]
    InvalidStyle(String),
    #[error("invalid tile: {0}")]
    Tile(#[from] MltError),
    #[error("invalid image: {0}")]
    Image(String),
    #[error("invalid GeoJSON: {0}")]
    GeoJson(String),
    #[error("{0}")]
    Unsupported(String),
}
