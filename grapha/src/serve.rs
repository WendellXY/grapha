pub mod api;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::response::Html;
use axum::routing::get;

use grapha_core::graph::Graph;
use tantivy::Index;

const INDEX_HTML: &str = include_str!("serve/web/index.html");

pub struct AppState {
    pub project_path: PathBuf,
    pub graph: Graph,
    pub search_index: Index,
}

pub async fn run(
    project_path: PathBuf,
    graph: Graph,
    search_index: Index,
    port: u16,
) -> anyhow::Result<()> {
    let state = Arc::new(AppState {
        project_path,
        graph,
        search_index,
    });
    let app = Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route("/api/graph", get(api::get_graph))
        .route("/api/entries", get(api::get_entries))
        .route("/api/context/{symbol}", get(api::get_context))
        .route("/api/trace/{symbol}", get(api::get_trace))
        .route("/api/reverse/{symbol}", get(api::get_reverse))
        .route("/api/status", get(api::get_index_status))
        .route("/api/search", get(api::get_search))
        .route(
            "/api/annotations",
            get(api::list_annotations).post(api::post_annotation),
        )
        .route(
            "/api/annotations/sync",
            axum::routing::post(api::sync_annotations),
        )
        .route("/api/annotations/{symbol}", get(api::get_annotation))
        .with_state(state)
        .layer(tower_http::cors::CorsLayer::permissive());

    eprintln!("  \x1b[32m✓\x1b[0m serving at http://localhost:{port}");
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
