use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::fields::FieldSet;
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
        Err(query::QueryResolveError::NotFunction { hint }) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": hint })),
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

pub async fn get_index_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let status = match crate::index_status::load_index_status(
        &state.project_path,
        &state.project_path.join(".grapha"),
    ) {
        Ok(status) => serde_json::to_value(status).unwrap_or_default(),
        Err(error) => serde_json::json!({
            "error": error.to_string()
        }),
    };
    Json(status)
}

#[derive(Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub kind: Option<String>,
    pub module: Option<String>,
    pub file: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub fuzzy: bool,
    #[serde(default)]
    pub exact_name: bool,
    #[serde(default)]
    pub declarations_only: bool,
    #[serde(default)]
    pub public_only: bool,
    #[serde(default)]
    pub context: bool,
    pub fields: Option<String>,
}

fn default_limit() -> usize {
    20
}

pub async fn get_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Json<serde_json::Value> {
    let options = crate::search::SearchOptions {
        kind: params.kind,
        module: params.module,
        file_glob: params.file,
        role: params.role,
        fuzzy: params.fuzzy,
        exact_name: params.exact_name,
        declarations_only: params.declarations_only,
        public_only: params.public_only,
    };
    let results =
        crate::search::search_filtered(&state.search_index, &params.q, params.limit, &options)
            .unwrap_or_default();
    let fields = params
        .fields
        .as_deref()
        .map(FieldSet::parse)
        .unwrap_or_default();
    let graph =
        crate::search::needs_graph_for_projection(fields, params.context).then_some(&state.graph);
    let projected = crate::search::project_results(&results, graph, fields, params.context);
    let index_status = crate::index_status::load_index_status(
        &state.project_path,
        &state.project_path.join(".grapha"),
    )
    .ok();
    Json(serde_json::json!({
        "results": projected,
        "total": results.len(),
        "index_status": index_status
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search;
    use crate::serve::AppState;
    use grapha_core::graph::{Edge, EdgeKind, Graph, Node, NodeKind, NodeRole, Span, Visibility};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn make_state() -> (Arc<AppState>, tempfile::TempDir) {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "app::main".into(),
                    kind: NodeKind::Function,
                    name: "main".into(),
                    file: "src/main.rs".into(),
                    span: Span {
                        start: [1, 0],
                        end: [3, 1],
                    },
                    visibility: Visibility::Public,
                    metadata: HashMap::new(),
                    role: Some(NodeRole::EntryPoint),
                    signature: Some("fn main()".into()),
                    doc_comment: None,
                    module: Some("App".into()),
                    snippet: Some("fn main() { helper(); }".into()),
                },
                Node {
                    id: "app::helper".into(),
                    kind: NodeKind::Function,
                    name: "helper".into(),
                    file: "src/lib.rs".into(),
                    span: Span {
                        start: [5, 0],
                        end: [5, 12],
                    },
                    visibility: Visibility::Private,
                    metadata: HashMap::new(),
                    role: None,
                    signature: Some("fn helper()".into()),
                    doc_comment: None,
                    module: Some("Core".into()),
                    snippet: Some("fn helper() {}".into()),
                },
            ],
            edges: vec![Edge {
                source: "app::main".into(),
                target: "app::helper".into(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: Some(false),
                provenance: Vec::new(),
            }],
        };
        let dir = tempdir().unwrap();
        let index = search::build_index(&graph, dir.path()).unwrap();
        (
            Arc::new(AppState {
                project_path: PathBuf::from("."),
                graph,
                search_index: index,
            }),
            dir,
        )
    }

    #[tokio::test]
    async fn search_api_applies_filters_and_context() {
        let (state, _dir) = make_state();
        let response = get_search(
            State(state),
            Query(SearchParams {
                q: "main".into(),
                limit: 10,
                kind: Some("function".into()),
                module: Some("App".into()),
                file: Some("main.rs".into()),
                role: Some("entry_point".into()),
                fuzzy: false,
                exact_name: false,
                declarations_only: false,
                public_only: false,
                context: true,
                fields: Some("id,signature,role,snippet".into()),
            }),
        )
        .await;

        assert_eq!(response.0["total"], 1);
        let result = &response.0["results"][0];
        assert_eq!(result["name"], "main");
        assert_eq!(result["id"], "app::main");
        assert_eq!(result["signature"], "fn main()");
        assert_eq!(result["role"], "entry_point");
        assert_eq!(result["snippet"], "fn main() { helper(); }");
        assert!(result.get("file").is_none());
        assert_eq!(result["calls"][0], "app::helper");
    }
}
