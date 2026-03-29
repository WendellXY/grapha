mod changes;
mod compress;
mod discover;
mod error;
mod extract;
mod filter;
mod graph;
mod merge;
mod query;
mod resolve;
mod search;
mod store;

use std::path::{Path, PathBuf};

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
fn run_pipeline(path: &Path) -> anyhow::Result<graph::Graph> {
    let all_extensions: &[&str] = &["rs", "swift"];
    let files =
        discover::discover_files(path, all_extensions).context("failed to discover files")?;

    let mut results = Vec::new();
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
            Err(e) => eprintln!("warning: skipping {}: {e}", file.display()),
        }
    }

    Ok(merge::merge(results))
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
            let mut graph = run_pipeline(&path)?;

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
                    eprintln!("wrote {}", p.display());
                }
                None => println!("{json}"),
            }
        }
        Commands::Index {
            path,
            format,
            store_dir,
        } => {
            let store_path = store_dir.unwrap_or_else(|| path.join(".grapha"));
            let graph = run_pipeline(&path)?;

            std::fs::create_dir_all(&store_path)
                .with_context(|| format!("failed to create store dir {}", store_path.display()))?;

            let s: Box<dyn store::Store> = match format.as_str() {
                "json" => Box::new(store::json::JsonStore::new(store_path.join("graph.json"))),
                "sqlite" => Box::new(store::sqlite::SqliteStore::new(
                    store_path.join("grapha.db"),
                )),
                other => anyhow::bail!("unknown store format: {other}"),
            };

            s.save(&graph)?;

            // Also build search index
            let search_index_path = store_path.join("search_index");
            search::build_index(&graph, &search_index_path)?;

            eprintln!(
                "indexed {} nodes, {} edges → {}",
                graph.nodes.len(),
                graph.edges.len(),
                store_path.display()
            );
        }
        Commands::Context { symbol, path } => {
            let db_path = path.join(".grapha/grapha.db");
            let s = store::sqlite::SqliteStore::new(db_path);
            let graph = s
                .load()
                .context("no index found — run `grapha index` first")?;
            let result = query::context::query_context(&graph, &symbol)
                .ok_or_else(|| anyhow::anyhow!("symbol not found: {symbol}"))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Impact {
            symbol,
            depth,
            path,
        } => {
            let db_path = path.join(".grapha/grapha.db");
            let s = store::sqlite::SqliteStore::new(db_path);
            let graph = s
                .load()
                .context("no index found — run `grapha index` first")?;
            let result = query::impact::query_impact(&graph, &symbol, depth)
                .ok_or_else(|| anyhow::anyhow!("symbol not found: {symbol}"))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
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
                let db_path = path.join(".grapha/grapha.db");
                let s = store::sqlite::SqliteStore::new(db_path);
                let graph = s
                    .load()
                    .context("no index found — run `grapha index` first")?;
                search::build_index(&graph, &search_index_path)?
            };
            let results = search::search(&index, &q, limit)?;
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
    }

    Ok(())
}
