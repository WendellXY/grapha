use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModuleMap {
    pub modules: HashMap<String, Vec<PathBuf>>,
}

impl ModuleMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn merge(&mut self, other: ModuleMap) {
        for (name, dirs) in other.modules {
            self.modules.entry(name).or_default().extend(dirs);
        }
    }

    pub fn with_fallback(mut self, root: &Path) -> Self {
        if self.modules.is_empty() {
            let name = root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("root")
                .to_string();
            self.modules.insert(name, vec![root.to_path_buf()]);
        }
        self.canonicalize_dirs();
        self
    }

    fn canonicalize_dirs(&mut self) {
        for dirs in self.modules.values_mut() {
            for dir in dirs.iter_mut() {
                if let Ok(canonical) = dir.canonicalize() {
                    *dir = canonical;
                }
            }
        }
    }

    pub fn module_for_file(&self, file: &Path) -> Option<String> {
        let canonical_file = normalize_path(file);
        let mut best_match: Option<(&str, usize)> = None;

        for (name, dirs) in &self.modules {
            for dir in dirs {
                if let Ok(suffix) = canonical_file.strip_prefix(dir) {
                    let depth = suffix.components().count();
                    match best_match {
                        Some((_, best_depth)) if depth < best_depth => {
                            best_match = Some((name, depth));
                        }
                        None => {
                            best_match = Some((name, depth));
                        }
                        _ => {}
                    }
                }

                if best_match.is_none()
                    && file.is_relative()
                    && let Some(dir_name) = dir.file_name().and_then(|name| name.to_str())
                {
                    let file_str = file.to_string_lossy();
                    if file_str.starts_with(dir_name)
                        || file_str.starts_with(&format!("{dir_name}/"))
                    {
                        best_match = Some((name, usize::MAX));
                    }
                }
            }
        }

        best_match.map(|(name, _)| name.to_string())
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_module_fragments() {
        let mut left = ModuleMap::new();
        left.modules
            .insert("Alpha".to_string(), vec![PathBuf::from("/tmp/alpha/src")]);
        let mut right = ModuleMap::new();
        right
            .modules
            .insert("Beta".to_string(), vec![PathBuf::from("/tmp/beta/src")]);

        left.merge(right);

        assert_eq!(left.modules.len(), 2);
        assert!(left.modules.contains_key("Alpha"));
        assert!(left.modules.contains_key("Beta"));
    }

    #[test]
    fn module_for_file_prefers_deepest_match() {
        let mut map = ModuleMap::new();
        map.modules.insert(
            "Root".to_string(),
            vec![PathBuf::from("/workspace/project/src")],
        );
        map.modules.insert(
            "Feature".to_string(),
            vec![PathBuf::from("/workspace/project/src/feature")],
        );

        let resolved = map.module_for_file(Path::new("/workspace/project/src/feature/file.rs"));
        assert_eq!(resolved.as_deref(), Some("Feature"));
    }

    #[test]
    fn fallback_uses_root_name() {
        let map = ModuleMap::new().with_fallback(Path::new("/workspace/grapha"));
        assert_eq!(map.modules.len(), 1);
        assert!(map.modules.contains_key("grapha"));
    }
}
