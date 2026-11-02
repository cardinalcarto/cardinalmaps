use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use log::warn;
use serde::Serialize;

/// An error response, as RFC 9457 Problem Details.
#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    pub problem_type: &'static str,
    pub title: &'static str,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invalid_params: Vec<InvalidParam>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvalidParam {
    pub name: String,
    pub reason: String,
}

impl Problem {
    #[must_use]
    pub fn new(status: StatusCode, title: &'static str, detail: Option<String>) -> Self {
        Self { problem_type: "about:blank", title, status: status.as_u16(), detail, invalid_params: Vec::new() }
    }

    #[must_use]
    pub fn invalid_params(params: Vec<(String, String)>) -> Self {
        let mut problem = Self::new(StatusCode::BAD_REQUEST, "Bad Request", Some("Some parameters are invalid.".into()));
        problem.invalid_params = params.into_iter().map(|(name, reason)| InvalidParam { name, reason }).collect();
        problem
    }

    #[must_use]
    pub fn internal(error: &anyhow::Error) -> Self {
        warn!("Search failed: {error:#}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error", None)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::to_string(&self).unwrap_or_default();
        (status, [(header::CONTENT_TYPE, "application/problem+json")], body).into_response()
    }
}
