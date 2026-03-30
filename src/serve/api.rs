use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::query;

use super::AppState;

pub async fn get_graph(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&state.graph).unwrap_or_default())
}

pub async fn get_entries(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let result = query::entries::query_entries(&state.graph);
    Json(serde_json::to_value(&result).unwrap_or_default())
}

pub async fn get_context(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let decoded = urlencoding::decode(&symbol).unwrap_or_default();
    let result = query::context::query_context(&state.graph, &decoded)
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(serde_json::to_value(&result).unwrap_or_default()))
}

pub async fn get_trace(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let decoded = urlencoding::decode(&symbol).unwrap_or_default();
    let result = query::trace::query_trace(&state.graph, &decoded, 10)
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(serde_json::to_value(&result).unwrap_or_default()))
}

pub async fn get_reverse(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let decoded = urlencoding::decode(&symbol).unwrap_or_default();
    let result = query::reverse::query_reverse(&state.graph, &decoded)
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(serde_json::to_value(&result).unwrap_or_default()))
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
    let query_lower = params.q.to_lowercase();
    let results: Vec<serde_json::Value> = state
        .graph
        .nodes
        .iter()
        .filter(|n| n.name.to_lowercase().contains(&query_lower))
        .take(params.limit)
        .map(|n| {
            serde_json::json!({
                "id": n.id,
                "name": n.name,
                "kind": n.kind,
                "file": n.file,
            })
        })
        .collect();
    Json(serde_json::json!({ "results": results, "total": results.len() }))
}
