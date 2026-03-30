use std::path::{Path, PathBuf};

use super::Import;

/// Resolve a Rust import path to candidate file paths relative to project root.
pub fn resolve_rust_import(import: &Import, importer_file: &Path) -> Vec<PathBuf> {
    let path = &import.path;
    if path.starts_with("crate::") {
        resolve_crate_import(path)
    } else if path.starts_with("super::") {
        resolve_super_import(path, importer_file)
    } else if path.starts_with("self::") {
        resolve_self_import(path, importer_file)
    } else {
        // External crate — cannot resolve to local file
        vec![]
    }
}

fn resolve_crate_import(path: &str) -> Vec<PathBuf> {
    let segments: Vec<&str> = path.strip_prefix("crate::").unwrap().split("::").collect();
    let module_segments = if segments.len() > 1 {
        &segments[..segments.len() - 1]
    } else {
        &segments[..]
    };
    let module_path = module_segments.join("/");
    vec![
        PathBuf::from(format!("src/{}.rs", module_path)),
        PathBuf::from(format!("src/{}/mod.rs", module_path)),
    ]
}

fn resolve_super_import(path: &str, importer_file: &Path) -> Vec<PathBuf> {
    let parent = importer_file.parent().and_then(|p| p.parent());
    let segments: Vec<&str> = path.strip_prefix("super::").unwrap().split("::").collect();
    let module_segments = if segments.len() > 1 {
        &segments[..segments.len() - 1]
    } else {
        &segments[..]
    };
    match parent {
        Some(dir) => {
            let module_path = module_segments.join("/");
            vec![
                dir.join(format!("{}.rs", module_path)),
                dir.join(format!("{}/mod.rs", module_path)),
            ]
        }
        None => vec![],
    }
}

fn resolve_self_import(path: &str, importer_file: &Path) -> Vec<PathBuf> {
    let parent = importer_file.parent();
    let segments: Vec<&str> = path.strip_prefix("self::").unwrap().split("::").collect();
    let module_segments = if segments.len() > 1 {
        &segments[..segments.len() - 1]
    } else {
        &segments[..]
    };
    match parent {
        Some(dir) => {
            let module_path = module_segments.join("/");
            vec![
                dir.join(format!("{}.rs", module_path)),
                dir.join(format!("{}/mod.rs", module_path)),
            ]
        }
        None => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{Import, ImportKind};

    #[test]
    fn resolves_crate_import() {
        let import = Import {
            path: "crate::graph::Node".to_string(),
            symbols: vec![],
            kind: ImportKind::Relative,
        };
        let candidates = resolve_rust_import(&import, Path::new("src/main.rs"));
        assert!(candidates.contains(&PathBuf::from("src/graph.rs")));
        assert!(candidates.contains(&PathBuf::from("src/graph/mod.rs")));
    }

    #[test]
    fn resolves_super_import() {
        let import = Import {
            path: "super::utils::helper".to_string(),
            symbols: vec![],
            kind: ImportKind::Relative,
        };
        let candidates = resolve_rust_import(&import, Path::new("src/extract/rust.rs"));
        assert!(candidates.contains(&PathBuf::from("src/utils.rs")));
    }

    #[test]
    fn external_crate_returns_empty() {
        let import = Import {
            path: "serde::Serialize".to_string(),
            symbols: vec![],
            kind: ImportKind::Named,
        };
        let candidates = resolve_rust_import(&import, Path::new("src/main.rs"));
        assert!(candidates.is_empty());
    }
}
