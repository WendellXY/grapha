pub mod classify;
pub mod discover;
pub mod extract;
pub mod graph;
pub mod merge;
pub mod module;
pub mod normalize;
pub mod pipeline;
pub mod plugin;
pub mod resolve;
pub mod selector;
pub mod semantic;

pub use classify::*;
pub use discover::discover_files;
pub use extract::{ExtractionResult, LanguageExtractor};
pub use graph::*;
pub use merge::merge;
pub use module::ModuleMap;
pub use normalize::{edge_fingerprint, normalize_graph};
pub use pipeline::{
    build_graph, discover_modules, extract_semantics_with_registry, extract_with_registry,
    file_context, finish_plugins, lower_semantics, prepare_plugins, project_context,
    relative_path_for_input, stamp_semantic_module,
};
pub use plugin::{FileContext, GraphPass, LanguagePlugin, LanguageRegistry, ProjectContext};
pub use resolve::*;
pub use selector::*;
pub use semantic::*;
