use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::query;

use super::AppState;

pub async fn get_graph(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&state.graph).unwrap_or_default())
}

pub async fn get_entries(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let result = query::entries::query_entries(&state.graph);
    Json(serde_json::to_value(&result).unwrap_or_default())
}

#[derive(Serialize)]
struct QueryErrorPayload {
    error: &'static str,
    query: String,
    candidates: Vec<query::QueryCandidate>,
    hint: &'static str,
}

fn query_response<T: Serialize>(result: Result<T, query::QueryResolveError>) -> Response {
    match result {
        Ok(value) => Json(serde_json::to_value(&value).unwrap_or_default()).into_response(),
        Err(query::QueryResolveError::NotFound { .. }) => StatusCode::NOT_FOUND.into_response(),
        Err(query::QueryResolveError::Ambiguous { query, candidates }) => (
            StatusCode::BAD_REQUEST,
            Json(QueryErrorPayload {
                error: "ambiguous",
                query,
                candidates,
                hint: query::ambiguity_hint(),
            }),
        )
            .into_response(),
    }
}

pub async fn get_context(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> impl IntoResponse {
    let decoded = urlencoding::decode(&symbol).unwrap_or_default();
    query_response(query::context::query_context(&state.graph, &decoded))
}

pub async fn get_trace(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> impl IntoResponse {
    let decoded = urlencoding::decode(&symbol).unwrap_or_default();
    query_response(query::trace::query_trace(&state.graph, &decoded, 10))
}

pub async fn get_reverse(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> impl IntoResponse {
    let decoded = urlencoding::decode(&symbol).unwrap_or_default();
    query_response(query::reverse::query_reverse(&state.graph, &decoded, None))
}

#[derive(Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

pub async fn get_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Json<serde_json::Value> {
    let options = crate::search::SearchOptions::default();
    let results = match crate::search::search_filtered(
        &state.search_index,
        &params.q,
        params.limit,
        &options,
    ) {
        Ok(results) => results,
        Err(_) => vec![],
    };
    Json(serde_json::json!({ "results": results, "total": results.len() }))
}
