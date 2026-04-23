use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, anyhow};

use crate::store::Store;
use crate::{assets, cache, delta, index_status, localization, progress, search, store};

fn load_graph_with_cache(
    path: &Path,
    use_cache: bool,
) -> anyhow::Result<grapha_core::graph::Graph> {
    let store_dir = path.join(".grapha");
    let db_path = store_dir.join("grapha.db");

    if use_cache {
        let graph_cache = cache::GraphCache::new(&store_dir);
        if graph_cache.is_fresh(&db_path)
            && let Ok(graph) = graph_cache.load()
        {
            return Ok(graph);
        }
    }

    let s = store::sqlite::SqliteStore::new(db_path);
    let graph = s
        .load()
        .context("no index found — run `grapha index` first")?;

    if use_cache {
        let _ = cache::GraphCache::new(&store_dir).save(&graph);
    }

    Ok(graph)
}

pub(crate) fn load_graph(path: &Path) -> anyhow::Result<grapha_core::graph::Graph> {
    load_graph_with_cache(path, true)
}

pub(crate) fn load_graph_uncached(path: &Path) -> anyhow::Result<grapha_core::graph::Graph> {
    load_graph_with_cache(path, false)
}

pub(crate) fn load_graph_for_l10n(path: &Path) -> anyhow::Result<grapha_core::graph::Graph> {
    use grapha_core::graph::EdgeKind;
    let db_path = path.join(".grapha/grapha.db");
    let s = store::sqlite::SqliteStore::new(db_path);
    s.load_filtered(
        Some(&[EdgeKind::Contains, EdgeKind::TypeRef]),
        Some("l10n."),
    )
    .context("no index found — run `grapha index` first")
}

pub(crate) fn load_graph_for_l10n_usages(path: &Path) -> anyhow::Result<grapha_core::graph::Graph> {
    use grapha_core::graph::EdgeKind;
    let db_path = path.join(".grapha/grapha.db");
    let s = store::sqlite::SqliteStore::new(db_path);
    s.load_filtered(
        Some(&[
            EdgeKind::Contains,
            EdgeKind::TypeRef,
            EdgeKind::Calls,
            EdgeKind::Implements,
            EdgeKind::Uses,
        ]),
        Some("l10n."),
    )
    .context("no index found — run `grapha index` first")
}

fn store_file_path(format: &str, store_path: &Path) -> anyhow::Result<PathBuf> {
    match format {
        "json" => Ok(store_path.join("graph.json")),
        "sqlite" => Ok(store_path.join("grapha.db")),
        other => Err(anyhow!("unknown store format: {other}")),
    }
}

fn build_store(format: &str, store_path: &Path) -> anyhow::Result<Box<dyn store::Store + Send>> {
    Ok(match format {
        "json" => Box::new(store::json::JsonStore::new(store_path.join("graph.json"))),
        "sqlite" => Box::new(store::sqlite::SqliteStore::new(
            store_path.join("grapha.db"),
        )),
        other => anyhow::bail!("unknown store format: {other}"),
    })
}

fn load_existing_graph(
    format: &str,
    store_path: &Path,
) -> anyhow::Result<Option<grapha_core::graph::Graph>> {
    let store_file = store_file_path(format, store_path)?;
    if !store_file.exists() {
        return Ok(None);
    }

    let store = build_store(format, store_path)?;
    match store.load() {
        Ok(graph) => Ok(Some(graph)),
        Err(error) => {
            eprintln!(
                "  \x1b[33m!\x1b[0m failed to load existing store, falling back to full rebuild: {error}"
            );
            Ok(None)
        }
    }
}

pub(crate) fn open_search_index(path: &Path) -> anyhow::Result<tantivy::Index> {
    let search_index_path = path.join(".grapha/search_index");
    if search_index_path.exists() {
        Ok(tantivy::Index::open_in_dir(&search_index_path)?)
    } else {
        let graph = load_graph(path)?;
        eprintln!("  building search index...");
        Ok(search::build_index(&graph, &search_index_path)?)
    }
}

pub(crate) fn handle_index(
    path: PathBuf,
    format: String,
    store_dir: Option<PathBuf>,
    full_rebuild: bool,
    timing: bool,
) -> anyhow::Result<()> {
    let total_start = Instant::now();
    let store_path = store_dir.unwrap_or_else(|| path.join(".grapha"));
    let config = crate::config::load_config(&path);

    if !full_rebuild {
        match index_status::can_skip_index(&path, &store_path, &config) {
            Ok(Some(status)) => {
                progress::done_elapsed(
                    "index is up to date, skipping rebuild",
                    total_start.elapsed(),
                );
                progress::summary(&format!(
                    "\n  {} nodes, {} edges indexed in {:.1}s",
                    status.node_count,
                    status.edge_count,
                    total_start.elapsed().as_secs_f64(),
                ));
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "  \x1b[33m!\x1b[0m failed to evaluate no-op fast path, falling back to full index: {error}"
                );
            }
        }
    }

    let extraction_cache = cache::ExtractionCache::new(&store_path);
    let previous_extraction_cache = if full_rebuild {
        None
    } else {
        match extraction_cache.load_entries() {
            Ok(entries) => Some(entries),
            Err(error) => {
                eprintln!(
                    "  \x1b[33m!\x1b[0m failed to load extraction cache, falling back to fresh extraction: {error}"
                );
                None
            }
        }
    };
    let pipeline = crate::app::pipeline::run_pipeline(
        &path,
        true,
        timing,
        previous_extraction_cache.as_ref(),
    )?;
    let graph = pipeline.graph;

    std::fs::create_dir_all(&store_path)
        .with_context(|| format!("failed to create store dir {}", store_path.display()))?;

    let previous_graph = if full_rebuild {
        None
    } else {
        load_existing_graph(&format, &store_path)?
    };

    let delta = if full_rebuild {
        None
    } else {
        previous_graph
            .as_ref()
            .map(|prev| delta::GraphDelta::between(prev, &graph))
    };

    let graph_unchanged = delta.as_ref().is_some_and(|d| d.is_empty());

    let search_index_path = store_path.join("search_index");
    let index_root = path.clone();

    if graph_unchanged {
        let snapshot_result = std::thread::scope(|scope| {
            let localization_handle = scope.spawn(|| {
                let t = Instant::now();
                let stats =
                    localization::build_and_save_catalog_snapshot(&index_root, &store_path)?;
                Ok::<_, anyhow::Error>((t.elapsed(), stats))
            });

            let assets_handle = scope.spawn(|| {
                let t = Instant::now();
                let stats = assets::build_and_save_snapshot(&index_root, &store_path)?;
                Ok::<_, anyhow::Error>((t.elapsed(), stats))
            });

            let localization = localization_handle
                .join()
                .expect("localization thread panicked")?;
            let assets = assets_handle.join().expect("assets thread panicked")?;
            Ok::<_, anyhow::Error>((localization, assets))
        });
        let ((localize_elapsed, localize_stats), (assets_elapsed, assets_stats)) = snapshot_result?;

        extraction_cache
            .save_entries(&pipeline.extraction_cache_entries)
            .with_context(|| "failed to save extraction cache".to_string())?;
        index_status::save_index_status(
            &path,
            &store_path,
            graph.nodes.len(),
            graph.edges.len(),
            &config,
        )?;

        eprintln!("  \x1b[32m✓\x1b[0m no graph changes detected, skipping store and search sync");
        progress::done_elapsed(
            &format!(
                "saved localization snapshot ({} records)",
                localize_stats.record_count
            ),
            localize_elapsed,
        );
        for warning in &localize_stats.warnings {
            eprintln!(
                "  \x1b[33m!\x1b[0m skipped invalid localization catalog {}: {}",
                warning.catalog_file, warning.reason
            );
        }
        progress::done_elapsed(
            &format!(
                "saved asset snapshot ({} images)",
                assets_stats.record_count
            ),
            assets_elapsed,
        );
        for warning in &assets_stats.warnings {
            eprintln!(
                "  \x1b[33m!\x1b[0m skipped invalid asset catalog {}: {}",
                warning.catalog_path, warning.reason
            );
        }

        progress::summary(&format!(
            "\n  {} nodes, {} edges indexed in {:.1}s",
            graph.nodes.len(),
            graph.edges.len(),
            total_start.elapsed().as_secs_f64(),
        ));

        return Ok(());
    }

    let save_result = std::thread::scope(|scope| {
        let save_handle = scope.spawn(|| {
            let t = Instant::now();
            let s = build_store(&format, &store_path)?;
            let stats = if full_rebuild {
                let stats = store::StoreWriteStats::from_graphs(
                    previous_graph.as_ref(),
                    &graph,
                    delta::SyncMode::FullRebuild,
                );
                s.save(&graph)?;
                stats
            } else {
                s.save_incremental(previous_graph.as_ref(), &graph)?
            };
            Ok::<_, anyhow::Error>((t.elapsed(), stats))
        });

        let search_handle = scope.spawn(|| {
            let t = Instant::now();
            let stats = search::sync_index(
                previous_graph.as_ref(),
                &graph,
                &search_index_path,
                full_rebuild,
                delta.as_ref(),
            )?;
            Ok::<_, anyhow::Error>((t.elapsed(), stats))
        });

        let localization_handle = scope.spawn(|| {
            let t = Instant::now();
            let stats = localization::build_and_save_catalog_snapshot(&index_root, &store_path)?;
            Ok::<_, anyhow::Error>((t.elapsed(), stats))
        });

        let assets_handle = scope.spawn(|| {
            let t = Instant::now();
            let stats = assets::build_and_save_snapshot(&index_root, &store_path)?;
            Ok::<_, anyhow::Error>((t.elapsed(), stats))
        });

        let save = save_handle.join().expect("save thread panicked")?;
        let search = search_handle.join().expect("search thread panicked")?;
        let localization = localization_handle
            .join()
            .expect("localization thread panicked")?;
        let assets = assets_handle.join().expect("assets thread panicked")?;
        Ok::<_, anyhow::Error>((save, search, localization, assets))
    });
    let (
        (save_elapsed, save_stats),
        (search_elapsed, search_stats),
        (localize_elapsed, localize_stats),
        (assets_elapsed, assets_stats),
    ) = save_result?;

    cache::GraphCache::new(&store_path).invalidate();
    cache::QueryCache::new(&store_path).invalidate();
    extraction_cache
        .save_entries(&pipeline.extraction_cache_entries)
        .with_context(|| "failed to save extraction cache".to_string())?;
    index_status::save_index_status(
        &path,
        &store_path,
        graph.nodes.len(),
        graph.edges.len(),
        &config,
    )?;

    progress::done_elapsed(
        &format!(
            "saved to {} ({}; {})",
            store_path.display(),
            format,
            save_stats.summary()
        ),
        save_elapsed,
    );
    progress::done_elapsed(
        &format!("built search index ({})", search_stats.summary()),
        search_elapsed,
    );
    progress::done_elapsed(
        &format!(
            "saved localization snapshot ({} records)",
            localize_stats.record_count
        ),
        localize_elapsed,
    );
    for warning in &localize_stats.warnings {
        eprintln!(
            "  \x1b[33m!\x1b[0m skipped invalid localization catalog {}: {}",
            warning.catalog_file, warning.reason
        );
    }
    progress::done_elapsed(
        &format!(
            "saved asset snapshot ({} images)",
            assets_stats.record_count
        ),
        assets_elapsed,
    );
    for warning in &assets_stats.warnings {
        eprintln!(
            "  \x1b[33m!\x1b[0m skipped invalid asset catalog {}: {}",
            warning.catalog_path, warning.reason
        );
    }

    progress::summary(&format!(
        "\n  {} nodes, {} edges indexed in {:.1}s",
        graph.nodes.len(),
        graph.edges.len(),
        total_start.elapsed().as_secs_f64(),
    ));

    Ok(())
}
