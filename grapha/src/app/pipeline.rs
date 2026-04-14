use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Context;

use crate::{cache, classify, compress, config, filter, progress, rust_plugin, snippet};

pub(crate) struct PipelineOutput {
    pub(crate) graph: grapha_core::graph::Graph,
    pub(crate) extraction_cache_entries:
        std::collections::HashMap<String, cache::ExtractionCacheEntry>,
}

fn builtin_registry() -> anyhow::Result<grapha_core::LanguageRegistry> {
    let mut registry = grapha_core::LanguageRegistry::new();
    rust_plugin::register_builtin(&mut registry)?;
    grapha_swift::register_builtin(&mut registry)?;
    Ok(registry)
}

fn extraction_cache_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn make_extraction_cache_entry(
    file: &Path,
    file_context: &grapha_core::FileContext,
    result: &grapha_core::ExtractionResult,
) -> Option<(String, cache::ExtractionCacheEntry)> {
    let stamp = cache::FileStamp::from_path(file)?;
    Some((
        extraction_cache_key(&file_context.relative_path),
        cache::ExtractionCacheEntry {
            stamp,
            module_name: file_context.module_name.clone(),
            result: result.clone(),
        },
    ))
}

/// Run the extraction pipeline on a path, returning a merged graph.
pub(crate) fn run_pipeline(
    path: &Path,
    verbose: bool,
    timing: bool,
    existing_extraction_cache: Option<
        &std::collections::HashMap<String, cache::ExtractionCacheEntry>,
    >,
) -> anyhow::Result<PipelineOutput> {
    let t = Instant::now();
    let registry = builtin_registry()?;
    let mut project_context = grapha_core::project_context(path);

    let cfg = config::load_config(path);
    project_context.index_store_enabled = cfg.swift.index_store;

    let (files, _) = std::thread::scope(|scope| {
        let files_handle = scope.spawn(|| {
            grapha_core::pipeline::discover_files(path, &registry)
                .context("failed to discover files")
        });
        let plugin_handle =
            scope.spawn(|| grapha_core::prepare_plugins(&registry, &project_context));
        let files = files_handle.join().expect("discover thread panicked")?;
        plugin_handle.join().expect("plugin thread panicked")?;
        Ok::<_, anyhow::Error>((files, ()))
    })?;

    let mut external_files: Vec<PathBuf> = Vec::new();
    let mut external_repo_count = 0usize;
    for ext in &cfg.external {
        let ext_path = Path::new(&ext.path);
        if !ext_path.exists() {
            if verbose {
                eprintln!(
                    "  \x1b[33m!\x1b[0m external repo '{}' not found at {}, skipping",
                    ext.name, ext.path
                );
            }
            continue;
        }
        match grapha_core::pipeline::discover_files(ext_path, &registry) {
            Ok(ext_discovered) => {
                external_files.extend(ext_discovered);
                external_repo_count += 1;
            }
            Err(e) => {
                if verbose {
                    eprintln!(
                        "  \x1b[33m!\x1b[0m failed to discover files in '{}': {e}",
                        ext.name
                    );
                }
            }
        }
    }

    let external_file_count = external_files.len();
    let all_files: Vec<PathBuf> = files.into_iter().chain(external_files).collect();

    if verbose {
        let msg = if external_file_count > 0 {
            format!(
                "discovered {} files + {} external ({} repos)",
                all_files.len() - external_file_count,
                external_file_count,
                external_repo_count
            )
        } else {
            format!("discovered {} files", all_files.len())
        };
        progress::done(&msg, t);
        if let Some(store) = grapha_swift::index_store_path(&project_context.project_root) {
            progress::done(&format!("index store: {}", store.display()), t);
        }
    }

    let mut module_map = grapha_core::discover_modules(&registry, &project_context)?;
    for ext in &cfg.external {
        let ext_path = Path::new(&ext.path);
        if !ext_path.exists() {
            continue;
        }
        let mut ext_context = grapha_core::project_context(ext_path);
        ext_context.index_store_enabled = cfg.swift.index_store;
        if !ext_context.index_store_enabled {
            grapha_swift::clear_index_store_path(&ext_context.project_root);
        }
        if let Ok(ext_modules) = grapha_core::discover_modules(&registry, &ext_context) {
            module_map.merge(ext_modules);
        }
    }

    let t = Instant::now();
    let pb = if verbose && all_files.len() > 1 {
        Some(progress::bar(all_files.len() as u64, "extracting"))
    } else {
        None
    };

    use rayon::prelude::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    let skipped = AtomicUsize::new(0);
    let extracted = AtomicUsize::new(0);
    let reused_cached = AtomicUsize::new(0);

    let t_read_ns = AtomicU64::new(0);
    let t_extract_ns = AtomicU64::new(0);
    let t_snippet_ns = AtomicU64::new(0);
    let t_file_context_ns = AtomicU64::new(0);
    let t_total_per_file_ns = AtomicU64::new(0);
    let t_max_single_file_ns = AtomicU64::new(0);
    let extraction_cache_entries = Mutex::new(std::collections::HashMap::new());

    let results: Vec<_> = all_files
        .par_iter()
        .filter_map(|file| {
            let t_file_start = Instant::now();
            let t_fc = Instant::now();
            let file_context = grapha_core::file_context(&project_context, &module_map, file);
            t_file_context_ns.fetch_add(t_fc.elapsed().as_nanos() as u64, Ordering::Relaxed);
            let cache_key = extraction_cache_key(&file_context.relative_path);
            if let Some(existing_cache) = existing_extraction_cache
                && let Some(entry) = existing_cache.get(&cache_key)
                && entry.module_name.as_deref() == file_context.module_name.as_deref()
                && cache::FileStamp::from_path(file).is_some_and(|stamp| stamp == entry.stamp)
            {
                reused_cached.fetch_add(1, Ordering::Relaxed);
                if let Some(ref pb) = pb {
                    pb.inc(1);
                }
                extraction_cache_entries
                    .lock()
                    .expect("extraction cache mutex poisoned")
                    .insert(cache_key, entry.clone());
                let file_ns = t_file_start.elapsed().as_nanos() as u64;
                t_total_per_file_ns.fetch_add(file_ns, Ordering::Relaxed);
                t_max_single_file_ns.fetch_max(file_ns, Ordering::Relaxed);
                return Some(entry.result.clone());
            }

            let t0 = Instant::now();
            let source = match std::fs::read(file) {
                Ok(s) => s,
                Err(_) => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    if let Some(ref pb) = pb {
                        pb.inc(1);
                    }
                    let file_ns = t_file_start.elapsed().as_nanos() as u64;
                    t_total_per_file_ns.fetch_add(file_ns, Ordering::Relaxed);
                    t_max_single_file_ns.fetch_max(file_ns, Ordering::Relaxed);
                    return None;
                }
            };
            t_read_ns.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);

            let t1 = Instant::now();
            let extraction_result =
                grapha_core::extract_with_registry(&registry, &source, &file_context);
            t_extract_ns.fetch_add(t1.elapsed().as_nanos() as u64, Ordering::Relaxed);

            if let Some(ref pb) = pb {
                pb.inc(1);
            }

            match extraction_result {
                Ok(mut result) => {
                    extracted.fetch_add(1, Ordering::Relaxed);
                    let t2 = Instant::now();
                    if result
                        .nodes
                        .iter()
                        .any(|n| snippet::should_extract_snippet(n.kind))
                    {
                        let source_str: std::borrow::Cow<'_, str> =
                            match std::str::from_utf8(&source) {
                                Ok(s) => std::borrow::Cow::Borrowed(s),
                                Err(_) => String::from_utf8_lossy(&source),
                            };
                        let line_idx = snippet::LineIndex::new(&source_str);
                        for node in &mut result.nodes {
                            if snippet::should_extract_snippet(node.kind) {
                                node.snippet = line_idx
                                    .extract_symbol_snippet(&node.span, &node.name, node.kind);
                            }
                        }
                    }
                    t_snippet_ns.fetch_add(t2.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    if let Some((key, entry)) =
                        make_extraction_cache_entry(file, &file_context, &result)
                    {
                        extraction_cache_entries
                            .lock()
                            .expect("extraction cache mutex poisoned")
                            .insert(key, entry);
                    }
                    let file_ns = t_file_start.elapsed().as_nanos() as u64;
                    t_total_per_file_ns.fetch_add(file_ns, Ordering::Relaxed);
                    t_max_single_file_ns.fetch_max(file_ns, Ordering::Relaxed);
                    Some(result)
                }
                Err(e) => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    if verbose && let Some(ref pb) = pb {
                        pb.suspend(|| {
                            eprintln!("  \x1b[33m!\x1b[0m skipping {}: {e}", file.display())
                        });
                    }
                    let file_ns = t_file_start.elapsed().as_nanos() as u64;
                    t_total_per_file_ns.fetch_add(file_ns, Ordering::Relaxed);
                    t_max_single_file_ns.fetch_max(file_ns, Ordering::Relaxed);
                    None
                }
            }
        })
        .collect();

    let skipped = skipped.load(Ordering::Relaxed);
    let extracted = extracted.load(Ordering::Relaxed);
    let reused_cached = reused_cached.load(Ordering::Relaxed);
    let extraction_cache_entries = extraction_cache_entries
        .into_inner()
        .expect("extraction cache mutex poisoned");

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    if timing {
        let read_ms = t_read_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let extract_ms = t_extract_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let snippet_ms = t_snippet_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let is_ms = grapha_swift::TIMING_INDEXSTORE_NS.load(std::sync::atomic::Ordering::Relaxed)
            as f64
            / 1_000_000.0;
        let ts_parse_ms = grapha_swift::TIMING_TS_PARSE_NS
            .load(std::sync::atomic::Ordering::Relaxed) as f64
            / 1_000_000.0;
        let doc_ms = grapha_swift::TIMING_TS_DOC_NS.load(std::sync::atomic::Ordering::Relaxed)
            as f64
            / 1_000_000.0;
        let swiftui_ms = grapha_swift::TIMING_TS_SWIFTUI_NS
            .load(std::sync::atomic::Ordering::Relaxed) as f64
            / 1_000_000.0;
        let l10n_ms = grapha_swift::TIMING_TS_L10N_NS.load(std::sync::atomic::Ordering::Relaxed)
            as f64
            / 1_000_000.0;
        let asset_ms = grapha_swift::TIMING_TS_ASSET_NS.load(std::sync::atomic::Ordering::Relaxed)
            as f64
            / 1_000_000.0;
        let ss_ms = grapha_swift::TIMING_SWIFTSYNTAX_NS.load(std::sync::atomic::Ordering::Relaxed)
            as f64
            / 1_000_000.0;
        let ts_fb_ms = grapha_swift::TIMING_TS_FALLBACK_NS
            .load(std::sync::atomic::Ordering::Relaxed) as f64
            / 1_000_000.0;
        let fc_ms = t_file_context_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let total_per_file_ms = t_total_per_file_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let max_single_file_ms = t_max_single_file_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        eprintln!(
            "    thread-summed: read {:.0}ms, extract {:.0}ms, snippet {:.0}ms, file_context {:.0}ms, total_per_file {:.0}ms",
            read_ms, extract_ms, snippet_ms, fc_ms, total_per_file_ms
        );
        eprintln!("    max_single_file: {:.0}ms", max_single_file_ms);
        eprintln!(
            "    swift: indexstore {:.0}ms, ts-parse {:.0}ms, doc {:.0}ms, swiftui {:.0}ms, l10n {:.0}ms, asset {:.0}ms, swiftsyntax {:.0}ms, ts-fallback {:.0}ms",
            is_ms, ts_parse_ms, doc_ms, swiftui_ms, l10n_ms, asset_ms, ss_ms, ts_fb_ms
        );
    }
    if verbose {
        let msg = if skipped > 0 && reused_cached > 0 {
            format!(
                "extracted {} files, reused {} cached extraction results ({} skipped)",
                extracted, reused_cached, skipped
            )
        } else if skipped > 0 {
            format!("extracted {} files ({} skipped)", extracted, skipped)
        } else if reused_cached > 0 {
            format!(
                "extracted {} files, reused {} cached extraction results",
                extracted, reused_cached
            )
        } else {
            format!("extracted {} files", extracted)
        };
        progress::done(&msg, t);
    }

    let mut classifiers = registry.collect_classifiers();
    classifiers.insert(
        0,
        Box::new(classify::toml_rules::TomlRulesClassifier::new(
            &cfg.classifiers,
        )),
    );
    let composite = grapha_core::CompositeClassifier::new(classifiers);
    let preclassified_results: Vec<_> = results
        .into_iter()
        .map(|result| grapha_core::classify_extraction_result(result, &composite))
        .collect();

    let t = Instant::now();
    let merged = grapha_core::merge(preclassified_results);
    if verbose {
        progress::done(
            &format!(
                "merged → {} nodes, {} edges",
                merged.nodes.len(),
                merged.edges.len()
            ),
            t,
        );
    }

    let t = Instant::now();
    let mut graph = grapha_core::classify_graph(&merged, &composite);
    for pass in registry.collect_graph_passes() {
        graph = pass.apply(graph);
    }
    let graph = grapha_core::normalize_graph(graph);
    if verbose {
        let terminal_count = graph
            .nodes
            .iter()
            .filter(|n| matches!(n.role, Some(grapha_core::graph::NodeRole::Terminal { .. })))
            .count();
        let entry_count = graph
            .nodes
            .iter()
            .filter(|n| matches!(n.role, Some(grapha_core::graph::NodeRole::EntryPoint)))
            .count();
        progress::done(
            &format!(
                "classified → {} entries, {} terminals",
                entry_count, terminal_count
            ),
            t,
        );
    }

    Ok(PipelineOutput {
        graph,
        extraction_cache_entries,
    })
}

pub(crate) fn handle_analyze(
    path: PathBuf,
    output: Option<PathBuf>,
    filter: Option<String>,
    compact: bool,
) -> anyhow::Result<()> {
    let verbose = output.is_some();
    let mut graph = run_pipeline(&path, verbose, false, None)?.graph;

    if let Some(ref filter_str) = filter {
        let kinds = filter::parse_filter(filter_str)?;
        graph = filter::filter_graph(graph, &kinds);
    }

    let json = if compact {
        let pruned = compress::prune::prune(graph, false);
        let grouped = compress::group::group(&pruned);
        match &output {
            Some(_) => serde_json::to_string(&grouped)?,
            None => serde_json::to_string_pretty(&grouped)?,
        }
    } else {
        match &output {
            Some(_) => serde_json::to_string(&graph)?,
            None => serde_json::to_string_pretty(&graph)?,
        }
    };

    match output {
        Some(p) => {
            std::fs::write(&p, &json)
                .with_context(|| format!("failed to write {}", p.display()))?;
            eprintln!("  \x1b[32m✓\x1b[0m wrote {}", p.display());
        }
        None => println!("{json}"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_pipeline;
    use grapha_core::graph::NodeKind;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn run_pipeline_honors_swift_index_store_config_false_end_to_end() {
        let project_dir = TempDir::new().unwrap();
        let project_root = project_dir.path().join("MyApp");
        let source_dir = project_root.join("Sources");

        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            project_root.join("grapha.toml"),
            "[swift]\nindex_store = false\n",
        )
        .unwrap();
        fs::write(
            source_dir.join("ContentView.swift"),
            r#"
            import SwiftUI

            struct ContentView: View {
                var body: some View {
                    Text("Hello")
                }
            }
            "#,
        )
        .unwrap();

        let project_root = fs::canonicalize(&project_root).unwrap();

        grapha_swift::set_index_store_path(
            &project_root,
            Some(project_root.join("DerivedData/MyApp-abc123/Index.noindex/DataStore")),
        );

        let output = run_pipeline(&project_root, false, false, None).unwrap();

        assert!(
            output
                .graph
                .nodes
                .iter()
                .any(|node| node.name == "ContentView" && node.kind == NodeKind::Struct),
            "pipeline should still extract Swift symbols through fallback parsing"
        );
        assert!(
            grapha_swift::index_store_path(&project_root).is_none(),
            "cached index store should be cleared when [swift].index_store = false"
        );
    }
}
