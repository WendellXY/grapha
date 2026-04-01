mod binary;
mod bridge;
mod classifier;
mod graph_pass;
mod indexstore;
mod module_discovery;
mod swiftsyntax;
mod treesitter;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use treesitter::SwiftExtractor;

use grapha_core::{
    Classifier, ExtractionResult, FileContext, GraphPass, LanguageExtractor, LanguagePlugin,
    LanguageRegistry, ModuleMap, ProjectContext,
};

static INDEX_STORE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

pub struct SwiftPlugin;

impl LanguagePlugin for SwiftPlugin {
    fn id(&self) -> &'static str {
        "swift"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["swift"]
    }

    fn prepare_project(&self, context: &ProjectContext) -> anyhow::Result<()> {
        init_index_store(&context.project_root);
        Ok(())
    }

    fn discover_modules(&self, context: &ProjectContext) -> anyhow::Result<ModuleMap> {
        Ok(module_discovery::discover_swift_modules(
            &context.project_root,
        ))
    }

    fn extract(&self, source: &[u8], context: &FileContext) -> anyhow::Result<ExtractionResult> {
        extract_swift(
            source,
            &context.relative_path,
            None,
            Some(&context.project_root),
        )
    }

    fn stamp_module(
        &self,
        result: ExtractionResult,
        module_name: Option<&str>,
    ) -> ExtractionResult {
        stamp_swift_module(result, module_name)
    }

    fn classifiers(&self) -> Vec<Box<dyn Classifier>> {
        vec![Box::new(classifier::SwiftClassifier::new())]
    }

    fn graph_passes(&self) -> Vec<Box<dyn GraphPass>> {
        vec![Box::new(graph_pass::SwiftGraphPass)]
    }
}

pub fn register_builtin(registry: &mut LanguageRegistry) -> anyhow::Result<()> {
    registry.register(SwiftPlugin)
}

/// Auto-discover the Xcode index store for a project.
/// Walks up from the given path looking for .xcodeproj/.xcworkspace,
/// then matches the Xcode project name against DerivedData folders.
fn discover_index_store(start_path: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let derived_data = Path::new(&home).join("Library/Developer/Xcode/DerivedData");
    if !derived_data.is_dir() {
        return None;
    }

    // Collect candidate project names by walking up and looking for .xcodeproj
    let mut candidates = Vec::new();
    let mut dir = if start_path.is_file() {
        start_path.parent().map(Path::to_path_buf)
    } else {
        Some(start_path.to_path_buf())
    };

    while let Some(d) = dir {
        // Check for .xcodeproj or .xcworkspace
        if let Ok(entries) = std::fs::read_dir(&d) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.ends_with(".xcodeproj") || name_str.ends_with(".xcworkspace") {
                    let project_name = name_str
                        .trim_end_matches(".xcodeproj")
                        .trim_end_matches(".xcworkspace")
                        .to_string();
                    candidates.push(project_name);
                }
            }
        }
        // Also add the directory name itself as a candidate
        if let Some(dir_name) = d.file_name().and_then(|n| n.to_str()) {
            candidates.push(dir_name.replace(['-', '_', '.'], "").to_lowercase());
        }
        dir = d.parent().map(Path::to_path_buf);
        // Don't walk above home directory
        if dir.as_deref() == Some(Path::new(&home)) {
            break;
        }
    }

    // Match candidates against DerivedData folders
    for entry in std::fs::read_dir(&derived_data).ok()? {
        let entry = entry.ok()?;
        let dd_name = entry.file_name();
        let dd_str = dd_name.to_string_lossy();
        // DerivedData folder format: "<ProjectName>-<hash>"
        let dd_project = dd_str.split('-').next().unwrap_or(&dd_str).to_lowercase();

        for candidate in &candidates {
            let candidate_lower = candidate.to_lowercase();
            if dd_project == candidate_lower {
                let store = entry.path().join("Index.noindex/DataStore");
                if store.is_dir() {
                    return Some(store);
                }
            }
        }
    }
    None
}

/// Pre-discover the index store path. Call before starting extraction
/// to ensure the discovery log appears before the progress bar.
pub fn init_index_store(project_root: &Path) {
    INDEX_STORE_PATH.get_or_init(|| {
        if let Some(store) = discover_index_store(project_root) {
            return Some(store);
        }
        let mut dir = if project_root.is_file() {
            project_root.parent().map(Path::to_path_buf)
        } else {
            Some(project_root.to_path_buf())
        };
        while let Some(d) = dir {
            if let Some(store) = discover_index_store(&d) {
                return Some(store);
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        None
    });
}

/// Get the discovered index store path, if any.
pub fn index_store_path() -> Option<&'static Path> {
    INDEX_STORE_PATH.get().and_then(|p| p.as_deref())
}

fn stamp_swift_module(result: ExtractionResult, module_name: Option<&str>) -> ExtractionResult {
    let Some(module_name) = module_name else {
        return result;
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
            node.module = Some(module_name.to_string());
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

    ExtractionResult {
        nodes,
        edges,
        imports: result.imports,
    }
}

/// Extract Swift source code with waterfall strategy:
/// 1. Xcode index store (confidence 1.0)
/// 2. SwiftSyntax bridge (confidence 0.9)
/// 3. tree-sitter-swift fallback (confidence 0.6-0.8)
pub fn extract_swift(
    source: &[u8],
    file_path: &Path,
    index_store_path: Option<&Path>,
    project_root: Option<&Path>,
) -> anyhow::Result<ExtractionResult> {
    // Use explicit path, pre-discovered path, or auto-discover
    if let Some(root) = project_root {
        init_index_store(root);
    }
    let effective_store =
        index_store_path.or_else(|| INDEX_STORE_PATH.get().and_then(|p| p.as_deref()));

    if let Some(store_path) = effective_store {
        // Index store needs absolute file path for matching
        let abs_file = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else if let Some(root) = project_root {
            if root.is_file() {
                // Single file analysis: root IS the absolute file path
                root.to_path_buf()
            } else {
                root.join(file_path)
            }
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(file_path))
                .unwrap_or_else(|_| file_path.to_path_buf())
        };
        let canonical_file = std::fs::canonicalize(&abs_file).unwrap_or(abs_file);
        if let Some(mut result) = indexstore::extract_from_indexstore(&canonical_file, store_path) {
            // Index store doesn't provide doc comments — enrich via tree-sitter.
            let _ = treesitter::enrich_doc_comments(source, &mut result);
            let _ = treesitter::enrich_swiftui_structure(source, file_path, &mut result);
            let _ = treesitter::enrich_localization_metadata(source, file_path, &mut result);
            return Ok(result);
        }
    }

    if let Some(mut result) = swiftsyntax::extract_with_swiftsyntax(source, file_path) {
        let _ = treesitter::enrich_doc_comments(source, &mut result);
        let _ = treesitter::enrich_swiftui_structure(source, file_path, &mut result);
        let _ = treesitter::enrich_localization_metadata(source, file_path, &mut result);
        return Ok(result);
    }

    let extractor = SwiftExtractor;
    let mut result = extractor.extract(source, file_path)?;
    let _ = treesitter::enrich_localization_metadata(source, file_path, &mut result);
    Ok(result)
}

#[cfg(test)]
mod plugin_tests {
    use super::stamp_swift_module;
    use grapha_core::ExtractionResult;
    use grapha_core::graph::{Edge, EdgeKind, EdgeProvenance, Node, NodeKind, Span, Visibility};
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
                snippet: None,
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

        let stamped = stamp_swift_module(result, Some("Feature"));
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
}
