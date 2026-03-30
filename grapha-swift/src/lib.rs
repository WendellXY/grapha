mod bridge;
mod indexstore;
mod swiftsyntax;
mod treesitter;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use treesitter::SwiftExtractor;

use grapha_core::{ExtractionResult, LanguageExtractor};

static INDEX_STORE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

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
    // Use explicit path or auto-discover from DerivedData
    let discovered = INDEX_STORE_PATH.get_or_init(|| {
        // Try project root first, then walk up from file path
        if let Some(root) = project_root {
            if let Some(store) = discover_index_store(root) {
                eprintln!("[grapha] index store:: {}", store.display());
                return Some(store);
            }
        }
        // Walk up from file path looking for a project directory
        let mut dir = if file_path.is_file() {
            file_path.parent().map(Path::to_path_buf)
        } else {
            Some(file_path.to_path_buf())
        };
        while let Some(d) = dir {
            if let Some(store) = discover_index_store(&d) {
                eprintln!("[grapha] index store:: {}", store.display());
                return Some(store);
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        eprintln!("[grapha] index store:");
        None
    });

    let effective_store = index_store_path.or(discovered.as_deref());

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
        if let Some(result) = indexstore::extract_from_indexstore(&canonical_file, store_path) {
            return Ok(result);
        }
    }

    if let Some(result) = swiftsyntax::extract_with_swiftsyntax(source, file_path) {
        return Ok(result);
    }

    let extractor = SwiftExtractor;
    extractor.extract(source, file_path)
}
