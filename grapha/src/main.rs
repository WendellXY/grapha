mod app;
mod assets;
mod cache;
mod changes;
mod classify;
mod compress;
mod concepts;
mod config;
mod delta;
mod extract;
mod fields;
mod filter;
mod localization;
mod mcp;
mod progress;
mod query;
mod recall;
mod render;
mod rust_plugin;
mod search;
mod serve;
mod snippet;
mod store;
mod symbol_locator;
mod watch;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum TraceDirection {
    Forward,
    Reverse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OriginTerminalFilter {
    Network,
    Persistence,
    Cache,
    Event,
    Keychain,
    Search,
}

impl OriginTerminalFilter {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Persistence => "persistence",
            Self::Cache => "cache",
            Self::Event => "event",
            Self::Keychain => "keychain",
            Self::Search => "search",
        }
    }
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
        /// Show per-phase timing breakdown for performance profiling
        #[arg(long)]
        timing: bool,
    },
    /// Launch web UI for interactive graph exploration
    Serve {
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
        /// Run as MCP server over stdio (instead of HTTP)
        #[arg(long)]
        mcp: bool,
        /// Watch for file changes and auto-update the graph
        #[arg(long)]
        watch: bool,
    },
    /// Query symbol relationships and search indexed symbols
    Symbol {
        #[command(subcommand)]
        command: SymbolCommands,
    },
    /// Inspect dataflow between symbols, entries, and effects
    Flow {
        #[command(subcommand)]
        command: FlowCommands,
    },
    /// Inspect localization references and usage sites
    #[command(name = "l10n")]
    L10n {
        #[command(subcommand)]
        command: L10nCommands,
    },
    /// Inspect image asset catalogs and usage sites
    Asset {
        #[command(subcommand)]
        command: AssetCommands,
    },
    /// Resolve business concepts to likely code scopes and manage concept bindings
    Concept {
        #[command(subcommand)]
        command: ConceptCommands,
    },
    /// Run repository-scoped analysis over the indexed graph
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },
}

#[derive(Subcommand)]
enum SymbolCommands {
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
        /// Filter by symbol kind (function, struct, enum, trait, etc.)
        #[arg(long)]
        kind: Option<String>,
        /// Filter by module name
        #[arg(long)]
        module: Option<String>,
        /// Filter by file path glob
        #[arg(long)]
        file: Option<String>,
        /// Filter by role (entry_point, terminal, internal)
        #[arg(long)]
        role: Option<String>,
        /// Enable fuzzy matching (tolerates typos)
        #[arg(long)]
        fuzzy: bool,
        /// Include source snippet and relationships in results
        #[arg(long)]
        context: bool,
        /// Fields to display (comma-separated: file,id,locator,module,span,snippet,visibility,signature,role; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
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
        /// Fields to display (comma-separated: file,id,locator,module,span,snippet,visibility,signature,role; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
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
        /// Fields to display (comma-separated: file,id,locator,module,span,snippet,visibility,signature,role; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
    /// Analyze structural complexity of a type (properties, dependencies, invalidation surface)
    Complexity {
        /// Type name or ID to analyze
        symbol: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// List all declarations in a file, ordered by source position
    File {
        /// File name or path suffix (e.g. "RoomPage.swift" or "src/main.rs")
        file: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum FlowCommands {
    /// Trace dataflow forward to terminals or backward to entry points
    Trace {
        /// Symbol name or ID
        symbol: String,
        /// Trace direction
        #[arg(long, value_enum, default_value_t = TraceDirection::Forward)]
        direction: TraceDirection,
        /// Maximum traversal depth
        #[arg(long)]
        depth: Option<usize>,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
        /// Fields to display in tree output (comma-separated: file; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
    /// Derive a semantic effect graph from a symbol
    Graph {
        /// Symbol name or ID
        symbol: String,
        /// Maximum traversal depth
        #[arg(long, default_value = "10")]
        depth: usize,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
        /// Fields to display in tree output (comma-separated: file; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
    /// Trace backward to likely API/data origins for a UI symbol
    Origin {
        /// Symbol name or ID
        symbol: String,
        /// Maximum traversal depth
        #[arg(long, default_value = "10")]
        depth: usize,
        /// Keep only origins whose terminal kind matches
        #[arg(long, value_enum)]
        terminal_kind: Option<OriginTerminalFilter>,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
        /// Fields to display in output (comma-separated: file,snippet; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
    /// List auto-detected entry points
    Entries {
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Filter entry points by module name
        #[arg(long)]
        module: Option<String>,
        /// Filter entry points by file path or suffix
        #[arg(long)]
        file: Option<String>,
        /// Limit the number of shown entries
        #[arg(long)]
        limit: Option<usize>,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
        /// Fields to display in tree output (comma-separated: file; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
}

#[derive(Subcommand)]
enum L10nCommands {
    /// Resolve localization records reachable from a SwiftUI symbol subtree
    Symbol {
        /// Symbol name or ID
        symbol: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
        /// Fields to display in tree output (comma-separated: file; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
    /// Find SwiftUI usage sites for a localization key or translated value
    Usages {
        /// Localization key or translated string value
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
        /// Fields to display in tree output (comma-separated: file; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
}

#[derive(Subcommand)]
enum AssetCommands {
    /// List image assets from indexed catalogs
    List {
        /// Only show assets with no references in source code
        #[arg(long)]
        unused: bool,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Find source code usage sites for an image asset
    Usages {
        /// Asset name (e.g., "icon_gift" or "Room/voiceWave")
        name: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
        /// Fields to display in tree output (comma-separated: file; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConceptCommands {
    /// Search for likely scopes related to a business concept
    Search {
        /// Business concept text
        term: String,
        /// Max results
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
        /// Fields to display in tree output (comma-separated: file,id,locator,module,span,snippet,visibility,signature,role; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
    /// Show a stored concept mapping and its bound symbols
    Show {
        /// Business concept text
        term: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
        /// Fields to display in tree output (comma-separated: file,id,locator,module,span,snippet,visibility,signature,role; or "full"/"all"/"none")
        #[arg(long)]
        fields: Option<String>,
    },
    /// Bind a business concept to one or more symbols
    Bind {
        /// Business concept text
        term: String,
        /// One or more symbols to bind
        #[arg(long = "symbol", required = true)]
        symbols: Vec<String>,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Add aliases for an existing or new concept
    Alias {
        /// Business concept text
        term: String,
        /// One or more aliases to add
        #[arg(long = "add", required = true)]
        aliases: Vec<String>,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Remove a concept from the project concept store
    Remove {
        /// Business concept text
        term: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Remove bindings whose symbols no longer exist in the graph
    Prune {
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum RepoCommands {
    /// Detect code changes and analyze their impact
    Changes {
        /// Scope: "unstaged", "staged", "all", or a git ref (e.g., "main")
        #[arg(default_value = "all")]
        scope: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Show file/symbol map for orientation in large projects
    Map {
        /// Filter by module name
        #[arg(long)]
        module: Option<String>,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Detect code smells across the graph (god types, deep nesting, wide invalidation, etc.)
    Smells {
        /// Filter to a specific module
        #[arg(long)]
        module: Option<String>,
        /// Limit smell analysis to symbols declared in a matching file
        #[arg(long)]
        file: Option<String>,
        /// Limit smell analysis to a specific symbol and its local neighborhood
        #[arg(long)]
        symbol: Option<String>,
        /// Bypass both cached graph loads and cached smell results
        #[arg(long)]
        no_cache: bool,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Show per-module metrics (symbol counts, coupling, entry points)
    Modules {
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let render_options = app::query::tree_render_options(cli.color);

    match cli.command {
        Commands::Analyze {
            path,
            output,
            filter,
            compact,
        } => app::pipeline::handle_analyze(path, output, filter, compact)?,
        Commands::Index {
            path,
            format,
            store_dir,
            full_rebuild,
            timing,
        } => app::index::handle_index(path, format, store_dir, full_rebuild, timing)?,
        Commands::Serve {
            path,
            port,
            mcp,
            watch,
        } => app::serve::handle_serve(path, port, mcp, watch)?,
        Commands::Symbol { command } => app::query::handle_symbol_command(command, render_options)?,
        Commands::Flow { command } => app::query::handle_flow_command(command, render_options)?,
        Commands::L10n { command } => app::query::handle_l10n_command(command, render_options)?,
        Commands::Asset { command } => app::query::handle_asset_command(command, render_options)?,
        Commands::Concept { command } => {
            app::query::handle_concept_command(command, render_options)?
        }
        Commands::Repo { command } => app::query::handle_repo_command(command)?,
    }

    Ok(())
}
