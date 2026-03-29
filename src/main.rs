mod compress;
mod discover;
mod error;
mod extract;
mod filter;
mod graph;
mod merge;
mod resolve;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

use extract::LanguageExtractor;
use extract::rust::RustExtractor;
use extract::swift::SwiftExtractor;

#[derive(Parser)]
#[command(
    name = "grapha",
    version,
    about = "Structural code graph for LLM consumption"
)]
struct Cli {
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
}

fn extractor_for_path(path: &std::path::Path) -> Option<Box<dyn LanguageExtractor>> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "rs" => Some(Box::new(RustExtractor)),
        "swift" => Some(Box::new(SwiftExtractor)),
        _ => None,
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let all_extensions: &[&str] = &["rs", "swift"];
    let files =
        discover::discover_files(&cli.path, all_extensions).context("failed to discover files")?;

    let mut results = Vec::new();
    for file in &files {
        let extractor = match extractor_for_path(file) {
            Some(e) => e,
            None => continue,
        };

        let source =
            std::fs::read(file).with_context(|| format!("failed to read {}", file.display()))?;

        // Make path relative to the input path for cleaner IDs
        let relative = if cli.path.is_dir() {
            file.strip_prefix(&cli.path).unwrap_or(file)
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

    let mut graph = merge::merge(results);

    if let Some(ref filter_str) = cli.filter {
        let kinds = filter::parse_filter(filter_str)?;
        graph = filter::filter_graph(graph, &kinds);
    }

    let json = if cli.compact {
        let pruned = compress::prune::prune(graph, false);
        let grouped = compress::group::group(&pruned);
        match &cli.output {
            Some(_) => serde_json::to_string(&grouped)?,
            None => serde_json::to_string_pretty(&grouped)?,
        }
    } else {
        match &cli.output {
            Some(_) => serde_json::to_string(&graph)?,
            None => serde_json::to_string_pretty(&graph)?,
        }
    };

    match cli.output {
        Some(path) => {
            std::fs::write(&path, &json)
                .with_context(|| format!("failed to write {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => println!("{json}"),
    }

    Ok(())
}
