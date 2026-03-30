mod changes;
mod classify;
mod compress;
mod config;
mod discover;
mod error;
mod extract;
mod filter;
mod graph;
mod merge;
#[allow(dead_code)]
mod module;
mod progress;
mod query;
mod resolve;
mod search;
mod serve;
mod store;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Context;
use clap::{Parser, Subcommand};

use extract::LanguageExtractor;
use extract::rust::RustExtractor;
use extract::swift::SwiftExtractor;
use store::Store;

#[derive(Parser)]
#[command(
    name = "grapha",
    version,
    about = "Structural code graph for LLM consumption"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
    },
    /// Query symbol context (callers, callees, implementors)
    Context {
        /// Symbol name or ID
        symbol: String,
        /// Project directory (reads from .grapha/)
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
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
    },
    /// Reverse query: which entry points are affected by this symbol?
    Reverse {
        /// Symbol name or ID
        symbol: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// List auto-detected entry points
    Entries {
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
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

fn extractor_for_path(path: &Path) -> Option<Box<dyn LanguageExtractor>> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "rs" => Some(Box::new(RustExtractor)),
        "swift" => Some(Box::new(SwiftExtractor)),
        _ => None,
    }
}

/// Run the extraction pipeline on a path, returning a merged graph.
fn run_pipeline(path: &Path, verbose: bool) -> anyhow::Result<graph::Graph> {
    let t = Instant::now();

    let all_extensions: &[&str] = &["rs", "swift"];
    let files =
        discover::discover_files(path, all_extensions).context("failed to discover files")?;

    if verbose {
        progress::done(&format!("discovered {} files", files.len()), t);
    }

    let t = Instant::now();
    let pb = if verbose && files.len() > 1 {
        Some(progress::bar(files.len() as u64, "extracting"))
    } else {
        None
    };

    let mut results = Vec::new();
    let mut skipped = 0usize;
    for file in &files {
        let extractor = match extractor_for_path(file) {
            Some(e) => e,
            None => continue,
        };

        let source =
            std::fs::read(file).with_context(|| format!("failed to read {}", file.display()))?;

        let relative = if path.is_dir() {
            file.strip_prefix(path).unwrap_or(file)
        } else {
            file.file_name()
                .map(|n| n.as_ref())
                .unwrap_or(file.as_path())
        };

        match extractor.extract(&source, relative) {
            Ok(result) => results.push(result),
            Err(e) => {
                skipped += 1;
                if verbose {
                    if let Some(ref pb) = pb {
                        pb.suspend(|| {
                            eprintln!("  \x1b[33m!\x1b[0m skipping {}: {e}", file.display())
                        });
                    } else {
                        eprintln!("  \x1b[33m!\x1b[0m skipping {}: {e}", file.display());
                    }
                }
            }
        }

        if let Some(ref pb) = pb {
            pb.inc(1);
        }
    }

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
        Box::new(classify::toml_rules::TomlRulesClassifier::new(&cfg.classifiers)),
        Box::new(classify::swift::SwiftClassifier::new()),
        Box::new(classify::rust::RustClassifier::new()),
    ];
    let composite = classify::CompositeClassifier::new(classifiers);
    let graph = classify::pass::classify_graph(&merged, &composite);
    if verbose {
        let terminal_count = graph
            .nodes
            .iter()
            .filter(|n| matches!(n.role, Some(graph::NodeRole::Terminal { .. })))
            .count();
        let entry_count = graph
            .nodes
            .iter()
            .filter(|n| matches!(n.role, Some(graph::NodeRole::EntryPoint)))
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

fn load_graph(path: &Path) -> anyhow::Result<graph::Graph> {
    let db_path = path.join(".grapha/grapha.db");
    let s = store::sqlite::SqliteStore::new(db_path);
    s.load()
        .context("no index found — run `grapha index` first")
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

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
        } => {
            let total_start = Instant::now();
            let store_path = store_dir.unwrap_or_else(|| path.join(".grapha"));
            let graph = run_pipeline(&path, true)?;

            std::fs::create_dir_all(&store_path)
                .with_context(|| format!("failed to create store dir {}", store_path.display()))?;

            let t = Instant::now();
            let s: Box<dyn store::Store> = match format.as_str() {
                "json" => Box::new(store::json::JsonStore::new(store_path.join("graph.json"))),
                "sqlite" => Box::new(store::sqlite::SqliteStore::new(
                    store_path.join("grapha.db"),
                )),
                other => anyhow::bail!("unknown store format: {other}"),
            };
            s.save(&graph)?;
            progress::done(
                &format!("saved to {} ({})", store_path.display(), format),
                t,
            );

            let t = Instant::now();
            let search_index_path = store_path.join("search_index");
            search::build_index(&graph, &search_index_path)?;
            progress::done("built search index", t);

            progress::summary(&format!(
                "\n  {} nodes, {} edges indexed in {:.1}s",
                graph.nodes.len(),
                graph.edges.len(),
                total_start.elapsed().as_secs_f64(),
            ));
        }
        Commands::Context { symbol, path } => {
            let graph = load_graph(&path)?;
            let result = query::context::query_context(&graph, &symbol)
                .ok_or_else(|| anyhow::anyhow!("symbol not found: {symbol}"))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Impact {
            symbol,
            depth,
            path,
        } => {
            let graph = load_graph(&path)?;
            let result = query::impact::query_impact(&graph, &symbol, depth)
                .ok_or_else(|| anyhow::anyhow!("symbol not found: {symbol}"))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
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
        Commands::Trace { entry, depth, path } => {
            let graph = load_graph(&path)?;
            let result = query::trace::query_trace(&graph, &entry, depth)
                .ok_or_else(|| anyhow::anyhow!("entry point not found: {entry}"))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Reverse { symbol, path } => {
            let graph = load_graph(&path)?;
            let result = query::reverse::query_reverse(&graph, &symbol)
                .ok_or_else(|| anyhow::anyhow!("symbol not found: {symbol}"))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Entries { path } => {
            let graph = load_graph(&path)?;
            let result = query::entries::query_entries(&graph);
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Serve { path, port } => {
            let graph = load_graph(&path)?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(serve::run(graph, port))?;
        }
    }

    Ok(())
}
