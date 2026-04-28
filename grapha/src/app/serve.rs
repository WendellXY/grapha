use std::path::PathBuf;

use crate::store::Store;
use crate::{index_status, mcp, recall, search, serve, store, watch};

use super::index::{load_graph, open_search_index};

fn run_mcp_server_with_optional_watch(
    path: PathBuf,
    graph: grapha_core::graph::Graph,
    search_index: tantivy::Index,
    watch_mode: bool,
) -> anyhow::Result<()> {
    let state = mcp::handler::McpState {
        graph,
        search_index,
        project_root: path.clone(),
        store_path: path.join(".grapha"),
        recall: recall::Recall::new(),
    };

    let _watcher_guard = if watch_mode {
        let (rx, _guard) =
            watch::start_watcher(&path, &["swift", "rs", "ts", "tsx", "js", "jsx", "vue"])?;
        let store_path = path.join(".grapha");
        let project_path = path.clone();

        let (state_tx, state_rx) =
            std::sync::mpsc::channel::<(grapha_core::graph::Graph, tantivy::Index)>();

        std::thread::Builder::new()
            .name("grapha-watch-reindex".into())
            .spawn(move || {
                for event in rx {
                    match event {
                        watch::WatchEvent::FilesChanged(files) => {
                            eprintln!("watch: {} file(s) changed, re-indexing...", files.len());
                            match crate::app::pipeline::run_pipeline(
                                &project_path,
                                false,
                                false,
                                None,
                            ) {
                                Ok(output) => {
                                    let graph = output.graph;
                                    let store_file = store_path.join("grapha.db");
                                    let store = store::sqlite::SqliteStore::new(store_file);
                                    if let Err(e) = store.save(&graph) {
                                        eprintln!("watch: failed to save graph: {e}");
                                        continue;
                                    }
                                    let search_path = store_path.join("search_index");
                                    match search::build_index(&graph, &search_path) {
                                        Ok(index) => {
                                            if let Err(e) = index_status::save_index_status(
                                                &project_path,
                                                &store_path,
                                                graph.nodes.len(),
                                                graph.edges.len(),
                                                &crate::config::load_config(&project_path),
                                            ) {
                                                eprintln!(
                                                    "watch: failed to save index status: {e}"
                                                );
                                                continue;
                                            }
                                            if state_tx.send((graph, index)).is_err() {
                                                break;
                                            }
                                            eprintln!("watch: re-index complete");
                                        }
                                        Err(e) => {
                                            eprintln!("watch: failed to build search index: {e}");
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("watch: re-index failed: {e}");
                                }
                            }
                        }
                    }
                }
            })?;

        mcp::run_mcp_server_with_watch(state, state_rx)?;
        return Ok(());
    } else {
        None::<watch::WatcherGuard>
    };

    mcp::run_mcp_server(state)
}

pub(crate) fn handle_serve(
    path: PathBuf,
    port: u16,
    mcp_mode: bool,
    watch_mode: bool,
) -> anyhow::Result<()> {
    let graph = load_graph(&path)?;
    let search_index = open_search_index(&path)?;

    if mcp_mode {
        run_mcp_server_with_optional_watch(path, graph, search_index, watch_mode)
    } else {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(serve::run(path, graph, search_index, port))?;
        Ok(())
    }
}
