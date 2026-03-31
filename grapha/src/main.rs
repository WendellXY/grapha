mod changes;
mod classify;
mod compress;
mod config;
mod delta;
mod discover;
mod error;
mod extract;
mod filter;
mod localization;
mod merge;
mod module;
mod progress;
mod query;
mod render;
mod search;
mod serve;
mod store;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, anyhow};
use clap::{Parser, Subcommand, ValueEnum};

use store::Store;

#[derive(Parser)]
#[command(
    name = "grapha",
    version,
    about = "Structural code graph for LLM consumption"
)]
struct Cli {
    /// ANSI color mode for tree output
    #[arg(long, global = true, value_enum, default_value_t = ColorMode::Auto)]
    color: ColorMode,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum QueryOutputFormat {
    Json,
    Tree,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze source files and output graph
    Analyze {
        /// File or directory to analyze
        path: PathBuf,
        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Filter node kinds (comma-separated: fn,struct,enum,trait,impl,mod,field,variant)
        #[arg(long)]
        filter: Option<String>,
        /// Output in compact grouped format (optimized for LLM consumption)
        #[arg(long)]
        compact: bool,
    },
    /// Index a project into persistent storage
    Index {
        /// Project directory to index
        path: PathBuf,
        /// Storage format: "json" or "sqlite" (default: sqlite)
        #[arg(long, default_value = "sqlite")]
        format: String,
        /// Storage directory (default: .grapha/ in project root)
        #[arg(long)]
        store_dir: Option<PathBuf>,
        /// Force a full store/search rebuild instead of using incremental sync
        #[arg(long)]
        full_rebuild: bool,
    },
    /// Query symbol context (callers, callees, implementors)
    Context {
        /// Symbol name or ID
        symbol: String,
        /// Project directory (reads from .grapha/)
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
    /// Analyze blast radius of changing a symbol
    Impact {
        /// Symbol name or ID
        symbol: String,
        /// Maximum traversal depth
        #[arg(long, default_value = "3")]
        depth: usize,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
    /// Detect code changes and analyze their impact
    Changes {
        /// Scope: "unstaged", "staged", "all", or a git ref (e.g., "main")
        #[arg(default_value = "all")]
        scope: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Search symbols by name or file
    Search {
        /// Search query
        query: String,
        /// Max results
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Forward-trace dataflow from an entry point to terminal operations
    Trace {
        /// Entry point symbol name or ID
        entry: String,
        /// Maximum traversal depth
        #[arg(long, default_value = "10")]
        depth: usize,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
    /// Derive a semantic effect graph from an entry point
    Dataflow {
        /// Entry point symbol name or ID
        entry: String,
        /// Maximum traversal depth
        #[arg(long, default_value = "10")]
        depth: usize,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
    /// Reverse query: which entry points are affected by this symbol?
    Reverse {
        /// Symbol name or ID
        symbol: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
    /// List auto-detected entry points
    Entries {
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
    /// Resolve localization records reachable from a SwiftUI symbol subtree
    Localize {
        /// Symbol name or ID
        symbol: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
    /// Find SwiftUI usage sites for a localization key
    Usages {
        /// Localization key
        key: String,
        /// Optional table/catalog name
        #[arg(long)]
        table: Option<String>,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
    /// Launch web UI for interactive graph exploration
    Serve {
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
    },
}

/// Set `node.module` on every node in an extraction result.
fn stamp_module(
    result: extract::ExtractionResult,
    module_name: &Option<String>,
) -> extract::ExtractionResult {
    let module_name = match module_name {
        Some(name) => name,
        None => return result,
    };

    let manifest_id_remap: std::collections::HashMap<String, String> = result
        .nodes
        .iter()
        .filter(|node| {
            node.file.file_name().and_then(|name| name.to_str()) == Some("Package.swift")
        })
        .map(|node| {
            (
                node.id.clone(),
                format!("{}@@module:{}", node.id, module_name),
            )
        })
        .collect();

    let nodes = result
        .nodes
        .into_iter()
        .map(|mut node| {
            node.module = Some(module_name.clone());
            if let Some(remapped_id) = manifest_id_remap.get(&node.id) {
                node.id = remapped_id.clone();
            }
            node
        })
        .collect();

    let edges = result
        .edges
        .into_iter()
        .map(|mut edge| {
            if let Some(remapped_id) = manifest_id_remap.get(&edge.source) {
                edge.source = remapped_id.clone();
            }
            if let Some(remapped_id) = manifest_id_remap.get(&edge.target) {
                edge.target = remapped_id.clone();
            }
            for provenance in &mut edge.provenance {
                if let Some(remapped_id) = manifest_id_remap.get(&provenance.symbol_id) {
                    provenance.symbol_id = remapped_id.clone();
                }
            }
            edge
        })
        .collect();

    extract::ExtractionResult {
        nodes,
        edges,
        imports: result.imports,
    }
}

fn normalize_graph(mut graph: grapha_core::graph::Graph) -> grapha_core::graph::Graph {
    fn visibility_rank(visibility: &grapha_core::graph::Visibility) -> u8 {
        match visibility {
            grapha_core::graph::Visibility::Private => 0,
            grapha_core::graph::Visibility::Crate => 1,
            grapha_core::graph::Visibility::Public => 2,
        }
    }

    fn merge_node(existing: &mut grapha_core::graph::Node, incoming: grapha_core::graph::Node) {
        if visibility_rank(&incoming.visibility) > visibility_rank(&existing.visibility) {
            existing.visibility = incoming.visibility;
        }
        if existing.role.is_none() {
            existing.role = incoming.role;
        }
        if existing.signature.is_none() {
            existing.signature = incoming.signature;
        }
        if existing.doc_comment.is_none() {
            existing.doc_comment = incoming.doc_comment;
        }
        if existing.module.is_none() {
            existing.module = incoming.module;
        }
        for (key, value) in incoming.metadata {
            existing.metadata.entry(key).or_insert(value);
        }
    }

    let mut node_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut normalized_nodes: Vec<grapha_core::graph::Node> = Vec::with_capacity(graph.nodes.len());
    for node in graph.nodes {
        if let Some(existing_index) = node_index.get(&node.id).copied() {
            merge_node(&mut normalized_nodes[existing_index], node);
        } else {
            node_index.insert(node.id.clone(), normalized_nodes.len());
            normalized_nodes.push(node);
        }
    }

    let mut edge_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut normalized_edges: Vec<grapha_core::graph::Edge> = Vec::with_capacity(graph.edges.len());

    for edge in graph.edges {
        let fingerprint = delta::edge_fingerprint(&edge);
        if let Some(existing_index) = edge_index.get(&fingerprint).copied() {
            let existing = &mut normalized_edges[existing_index];
            existing.confidence = existing.confidence.max(edge.confidence);
            for provenance in edge.provenance {
                if !existing
                    .provenance
                    .iter()
                    .any(|current| current == &provenance)
                {
                    existing.provenance.push(provenance);
                }
            }
        } else {
            edge_index.insert(fingerprint, normalized_edges.len());
            normalized_edges.push(edge);
        }
    }

    graph.nodes = normalized_nodes;
    graph.edges = normalized_edges;
    graph
}

/// Run the extraction pipeline on a path, returning a merged graph.
fn run_pipeline(path: &Path, verbose: bool) -> anyhow::Result<grapha_core::graph::Graph> {
    let t = Instant::now();

    let all_extensions: &[&str] = &["rs", "swift"];
    let files =
        discover::discover_files(path, all_extensions).context("failed to discover files")?;

    if verbose {
        progress::done(&format!("discovered {} files", files.len()), t);
    }

    let abs_root = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let module_map = module::ModuleMap::discover(&abs_root);

    // Pre-discover index store before starting the progress bar
    let t = Instant::now();
    grapha_swift::init_index_store(&abs_root);
    if let Some(store) = grapha_swift::index_store_path()
        && verbose
    {
        progress::done(&format!("index store: {}", store.display()), t);
    }

    let t = Instant::now();
    let pb = if verbose && files.len() > 1 {
        Some(progress::bar(files.len() as u64, "extracting"))
    } else {
        None
    };

    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let skipped = AtomicUsize::new(0);

    let results: Vec<_> = files
        .par_iter()
        .filter_map(|file| {
            let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "rs" | "swift") {
                if let Some(ref pb) = pb {
                    pb.inc(1);
                }
                return None;
            }

            let source = match std::fs::read(file) {
                Ok(s) => s,
                Err(_) => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    if let Some(ref pb) = pb {
                        pb.inc(1);
                    }
                    return None;
                }
            };

            let relative = if path.is_dir() {
                file.strip_prefix(path).unwrap_or(file)
            } else {
                file.file_name()
                    .map(|n| n.as_ref())
                    .unwrap_or(file.as_path())
            };

            let abs_file = std::fs::canonicalize(file).unwrap_or_else(|_| abs_root.join(relative));
            let file_module = module_map.module_for_file(&abs_file).or_else(|| {
                relative
                    .components()
                    .next()
                    .and_then(|c| c.as_os_str().to_str())
                    .map(|s| s.to_string())
            });

            let extraction_result = match ext {
                "swift" => grapha_swift::extract_swift(&source, relative, None, Some(&abs_root)),
                "rs" => {
                    let extractor = extract::rust::RustExtractor;
                    use grapha_core::LanguageExtractor;
                    extractor.extract(&source, relative)
                }
                _ => return None,
            };

            if let Some(ref pb) = pb {
                pb.inc(1);
            }

            match extraction_result {
                Ok(result) => Some(stamp_module(result, &file_module)),
                Err(e) => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    if verbose && let Some(ref pb) = pb {
                        pb.suspend(|| {
                            eprintln!("  \x1b[33m!\x1b[0m skipping {}: {e}", file.display())
                        });
                    }
                    None
                }
            }
        })
        .collect();

    let skipped = skipped.load(Ordering::Relaxed);

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    if verbose {
        let msg = if skipped > 0 {
            format!("extracted {} files ({} skipped)", results.len(), skipped)
        } else {
            format!("extracted {} files", results.len())
        };
        progress::done(&msg, t);
    }

    let t = Instant::now();
    let merged = merge::merge(results);
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
    let cfg = config::load_config(path);
    let classifiers: Vec<Box<dyn classify::Classifier>> = vec![
        Box::new(classify::toml_rules::TomlRulesClassifier::new(
            &cfg.classifiers,
        )),
        Box::new(classify::swift::SwiftClassifier::new()),
        Box::new(classify::rust::RustClassifier::new()),
    ];
    let composite = classify::CompositeClassifier::new(classifiers);
    let graph = normalize_graph(classify::pass::classify_graph(&merged, &composite));
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

    Ok(graph)
}

fn load_graph(path: &Path) -> anyhow::Result<grapha_core::graph::Graph> {
    let db_path = path.join(".grapha/grapha.db");
    let s = store::sqlite::SqliteStore::new(db_path);
    s.load()
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

fn kind_label(kind: grapha_core::graph::NodeKind) -> String {
    serde_json::to_string(&kind)
        .unwrap_or_else(|_| format!("{kind:?}"))
        .trim_matches('"')
        .to_string()
}

fn format_ambiguity_error(query: &str, candidates: &[query::QueryCandidate]) -> String {
    let mut message = format!("ambiguous query: {query}\n");
    for candidate in candidates {
        message.push_str(&format!(
            "  - {} [{}] in {} ({})\n",
            candidate.name,
            kind_label(candidate.kind),
            candidate.file,
            candidate.id
        ));
    }
    message.push_str(&format!("hint: {}", query::ambiguity_hint()));
    message
}

fn resolve_query_result<T>(
    result: Result<T, query::QueryResolveError>,
    missing_label: &str,
) -> anyhow::Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(query::QueryResolveError::NotFound { query }) => {
            Err(anyhow!("{missing_label} not found: {query}"))
        }
        Err(query::QueryResolveError::Ambiguous { query, candidates }) => {
            Err(anyhow!(format_ambiguity_error(&query, &candidates)))
        }
    }
}

fn tree_render_options(color: ColorMode) -> render::RenderOptions {
    use std::io::IsTerminal;

    match color {
        ColorMode::Always => render::RenderOptions::color(),
        ColorMode::Never => render::RenderOptions::plain(),
        ColorMode::Auto => {
            if std::io::stdout().is_terminal() {
                render::RenderOptions::color()
            } else {
                render::RenderOptions::plain()
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let render_options = tree_render_options(cli.color);

    match cli.command {
        Commands::Analyze {
            path,
            output,
            filter,
            compact,
        } => {
            // verbose when writing to file (stdout is for JSON)
            let verbose = output.is_some();
            let mut graph = run_pipeline(&path, verbose)?;

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
        }
        Commands::Index {
            path,
            format,
            store_dir,
            full_rebuild,
        } => {
            let total_start = Instant::now();
            let store_path = store_dir.unwrap_or_else(|| path.join(".grapha"));
            let graph = run_pipeline(&path, true)?;

            std::fs::create_dir_all(&store_path)
                .with_context(|| format!("failed to create store dir {}", store_path.display()))?;

            let previous_graph = if full_rebuild {
                None
            } else {
                load_existing_graph(&format, &store_path)?
            };

            // Run store sync and search sync in parallel — they're independent
            let search_index_path = store_path.join("search_index");
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
                    Ok::<_, anyhow::Error>((t, stats))
                });

                let search_handle = scope.spawn(|| {
                    let t = Instant::now();
                    let stats = search::sync_index(
                        previous_graph.as_ref(),
                        &graph,
                        &search_index_path,
                        full_rebuild,
                    )?;
                    Ok::<_, anyhow::Error>((t, stats))
                });

                let save = save_handle.join().expect("save thread panicked")?;
                let search = search_handle.join().expect("search thread panicked")?;
                Ok::<_, anyhow::Error>((save, search))
            });
            let ((save_t, save_stats), (search_t, search_stats)) = save_result?;
            progress::done(
                &format!(
                    "saved to {} ({}; {})",
                    store_path.display(),
                    format,
                    save_stats.summary()
                ),
                save_t,
            );
            progress::done(
                &format!("built search index ({})", search_stats.summary()),
                search_t,
            );

            progress::summary(&format!(
                "\n  {} nodes, {} edges indexed in {:.1}s",
                graph.nodes.len(),
                graph.edges.len(),
                total_start.elapsed().as_secs_f64(),
            ));
        }
        Commands::Context {
            symbol,
            path,
            format,
        } => {
            let graph = load_graph(&path)?;
            let result =
                resolve_query_result(query::context::query_context(&graph, &symbol), "symbol")?;
            match format {
                QueryOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                QueryOutputFormat::Tree => {
                    println!(
                        "{}",
                        render::render_context_with_options(&result, render_options)
                    )
                }
            }
        }
        Commands::Impact {
            symbol,
            depth,
            path,
            format,
        } => {
            let graph = load_graph(&path)?;
            let result = resolve_query_result(
                query::impact::query_impact(&graph, &symbol, depth),
                "symbol",
            )?;
            match format {
                QueryOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                QueryOutputFormat::Tree => {
                    println!(
                        "{}",
                        render::render_impact_with_options(&result, render_options)
                    )
                }
            }
        }
        Commands::Changes { scope, path } => {
            let graph = load_graph(&path)?;
            let report = changes::detect_changes(&path, &graph, &scope)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Commands::Search {
            query: q,
            limit,
            path,
        } => {
            let search_index_path = path.join(".grapha/search_index");
            let index = if search_index_path.exists() {
                tantivy::Index::open_in_dir(&search_index_path)?
            } else {
                let graph = load_graph(&path)?;
                eprintln!("  building search index...");
                search::build_index(&graph, &search_index_path)?
            };
            let results = search::search(&index, &q, limit)?;
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        Commands::Trace {
            entry,
            depth,
            path,
            format,
        } => {
            let graph = load_graph(&path)?;
            let result = resolve_query_result(
                query::trace::query_trace(&graph, &entry, depth),
                "entry point",
            )?;
            match format {
                QueryOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                QueryOutputFormat::Tree => {
                    println!(
                        "{}",
                        render::render_trace_with_options(&result, render_options)
                    )
                }
            }
        }
        Commands::Dataflow {
            entry,
            depth,
            path,
            format,
        } => {
            let graph = load_graph(&path)?;
            let result = resolve_query_result(
                query::dataflow::query_dataflow(&graph, &entry, depth),
                "entry point",
            )?;
            match format {
                QueryOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                QueryOutputFormat::Tree => {
                    println!(
                        "{}",
                        render::render_dataflow_with_options(&result, render_options)
                    )
                }
            }
        }
        Commands::Reverse {
            symbol,
            path,
            format,
        } => {
            let graph = load_graph(&path)?;
            let result =
                resolve_query_result(query::reverse::query_reverse(&graph, &symbol), "symbol")?;
            match format {
                QueryOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                QueryOutputFormat::Tree => {
                    println!(
                        "{}",
                        render::render_reverse_with_options(&result, render_options)
                    )
                }
            }
        }
        Commands::Entries { path, format } => {
            let graph = load_graph(&path)?;
            let result = query::entries::query_entries(&graph);
            match format {
                QueryOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                QueryOutputFormat::Tree => {
                    println!(
                        "{}",
                        render::render_entries_with_options(&result, render_options)
                    )
                }
            }
        }
        Commands::Localize {
            symbol,
            path,
            format,
        } => {
            let graph = load_graph(&path)?;
            let catalogs = localization::load_catalog_index(&path)?;
            let result = resolve_query_result(
                query::localize::query_localize(&graph, &catalogs, &symbol),
                "symbol",
            )?;
            match format {
                QueryOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                QueryOutputFormat::Tree => {
                    println!(
                        "{}",
                        render::render_localize_with_options(&result, render_options)
                    )
                }
            }
        }
        Commands::Usages {
            key,
            table,
            path,
            format,
        } => {
            let graph = load_graph(&path)?;
            let catalogs = localization::load_catalog_index(&path)?;
            let result = query::usages::query_usages(&graph, &catalogs, &key, table.as_deref());
            match format {
                QueryOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                QueryOutputFormat::Tree => {
                    println!(
                        "{}",
                        render::render_usages_with_options(&result, render_options)
                    )
                }
            }
        }
        Commands::Serve { path, port } => {
            let graph = load_graph(&path)?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(serve::run(graph, port))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{normalize_graph, stamp_module};
    use grapha_core::ExtractionResult;
    use grapha_core::graph::{
        Edge, EdgeKind, EdgeProvenance, Graph, Node, NodeKind, Span, Visibility,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn stamp_module_namespaces_package_manifest_ids() {
        let result = ExtractionResult {
            nodes: vec![Node {
                id: "s:4main7package18PackageDescription0C0Cvg".to_string(),
                kind: NodeKind::Function,
                name: "getter:package".to_string(),
                file: PathBuf::from("Package.swift"),
                span: Span {
                    start: [0, 0],
                    end: [0, 0],
                },
                visibility: Visibility::Public,
                metadata: HashMap::new(),
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
            }],
            edges: vec![Edge {
                source: "s:4main7package18PackageDescription0C0Cvg".to_string(),
                target: "external".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: vec![EdgeProvenance {
                    file: PathBuf::from("Package.swift"),
                    span: Span {
                        start: [0, 0],
                        end: [0, 0],
                    },
                    symbol_id: "s:4main7package18PackageDescription0C0Cvg".to_string(),
                }],
            }],
            imports: vec![],
        };

        let stamped = stamp_module(result, &Some("Feature".to_string()));
        assert_eq!(
            stamped.nodes[0].id,
            "s:4main7package18PackageDescription0C0Cvg@@module:Feature"
        );
        assert_eq!(
            stamped.edges[0].source,
            "s:4main7package18PackageDescription0C0Cvg@@module:Feature"
        );
        assert_eq!(
            stamped.edges[0].provenance[0].symbol_id,
            "s:4main7package18PackageDescription0C0Cvg@@module:Feature"
        );
        assert_eq!(stamped.nodes[0].module.as_deref(), Some("Feature"));
    }

    #[test]
    fn normalize_graph_merges_duplicate_edges_and_provenance() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![],
            edges: vec![
                Edge {
                    source: "a".to_string(),
                    target: "b".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.4,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: vec![EdgeProvenance {
                        file: PathBuf::from("a.swift"),
                        span: Span {
                            start: [1, 0],
                            end: [1, 4],
                        },
                        symbol_id: "a".to_string(),
                    }],
                },
                Edge {
                    source: "a".to_string(),
                    target: "b".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: vec![EdgeProvenance {
                        file: PathBuf::from("a.swift"),
                        span: Span {
                            start: [2, 0],
                            end: [2, 4],
                        },
                        symbol_id: "a".to_string(),
                    }],
                },
            ],
        };

        let normalized = normalize_graph(graph);
        assert_eq!(normalized.edges.len(), 1);
        assert_eq!(normalized.edges[0].confidence, 0.9);
        assert_eq!(normalized.edges[0].provenance.len(), 2);
    }

    #[test]
    fn normalize_graph_merges_duplicate_nodes_by_id() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "s:RoomPage.centerContentView".to_string(),
                    kind: NodeKind::Property,
                    name: "centerContentView".to_string(),
                    file: PathBuf::from("RoomPage.swift"),
                    span: Span {
                        start: [0, 0],
                        end: [0, 0],
                    },
                    visibility: Visibility::Private,
                    metadata: std::collections::HashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                },
                Node {
                    id: "s:RoomPage.centerContentView".to_string(),
                    kind: NodeKind::Property,
                    name: "centerContentView".to_string(),
                    file: PathBuf::from("RoomPage.swift"),
                    span: Span {
                        start: [10, 4],
                        end: [10, 20],
                    },
                    visibility: Visibility::Public,
                    metadata: std::collections::HashMap::new(),
                    role: Some(grapha_core::graph::NodeRole::EntryPoint),
                    signature: Some("var centerContentView: some View".to_string()),
                    doc_comment: Some("helper".to_string()),
                    module: Some("Room".to_string()),
                },
            ],
            edges: vec![],
        };

        let normalized = normalize_graph(graph);
        assert_eq!(normalized.nodes.len(), 1);
        assert_eq!(normalized.nodes[0].visibility, Visibility::Public);
        assert_eq!(
            normalized.nodes[0].role,
            Some(grapha_core::graph::NodeRole::EntryPoint)
        );
        assert_eq!(
            normalized.nodes[0].signature.as_deref(),
            Some("var centerContentView: some View")
        );
        assert_eq!(normalized.nodes[0].doc_comment.as_deref(), Some("helper"));
        assert_eq!(normalized.nodes[0].module.as_deref(), Some("Room"));
    }
}
