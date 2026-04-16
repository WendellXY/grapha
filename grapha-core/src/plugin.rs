use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};

use crate::classify::Classifier;
use crate::extract::ExtractionResult;
use crate::graph::Graph;
use crate::module::ModuleMap;

#[derive(Debug, Clone)]
pub struct ProjectContext {
    pub input_path: PathBuf,
    pub project_root: PathBuf,
    pub index_store_enabled: bool,
}

impl ProjectContext {
    pub fn new(input_path: &Path) -> Self {
        Self {
            input_path: input_path.to_path_buf(),
            project_root: std::fs::canonicalize(input_path)
                .unwrap_or_else(|_| input_path.to_path_buf()),
            index_store_enabled: true,
        }
    }

    pub fn is_single_file(&self) -> bool {
        self.project_root.is_file()
    }
}

#[derive(Debug, Clone)]
pub struct FileContext {
    pub input_path: PathBuf,
    pub project_root: PathBuf,
    pub relative_path: PathBuf,
    pub absolute_path: PathBuf,
    pub module_name: Option<String>,
    pub index_store_enabled: bool,
}

pub trait GraphPass: Send + Sync {
    fn apply(&self, graph: Graph) -> Graph;
}

pub trait LanguagePlugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];

    fn prepare_project(&self, _context: &ProjectContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn finish_project(&self, _context: &ProjectContext) -> anyhow::Result<()> {
        Ok(())
    }

    fn discover_modules(&self, _context: &ProjectContext) -> anyhow::Result<ModuleMap> {
        Ok(ModuleMap::new())
    }

    fn extract(&self, source: &[u8], context: &FileContext) -> anyhow::Result<ExtractionResult>;

    fn stamp_module(
        &self,
        result: ExtractionResult,
        module_name: Option<&str>,
    ) -> ExtractionResult {
        crate::pipeline::stamp_module(result, module_name)
    }

    fn classifiers(&self) -> Vec<Box<dyn Classifier>> {
        Vec::new()
    }

    fn graph_passes(&self) -> Vec<Box<dyn GraphPass>> {
        Vec::new()
    }
}

pub struct LanguageRegistry {
    plugins: Vec<Arc<dyn LanguagePlugin>>,
    plugins_by_extension: HashMap<String, Arc<dyn LanguagePlugin>>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            plugins_by_extension: HashMap::new(),
        }
    }

    pub fn register<P>(&mut self, plugin: P) -> anyhow::Result<()>
    where
        P: LanguagePlugin + 'static,
    {
        let plugin = Arc::new(plugin) as Arc<dyn LanguagePlugin>;
        for extension in plugin.extensions() {
            if let Some(existing) = self.plugins_by_extension.get(*extension) {
                bail!(
                    "language plugin '{}' conflicts with '{}' for extension '{}'",
                    plugin.id(),
                    existing.id(),
                    extension
                );
            }
        }

        for extension in plugin.extensions() {
            self.plugins_by_extension
                .insert((*extension).to_string(), Arc::clone(&plugin));
        }
        self.plugins.push(plugin);
        Ok(())
    }

    pub fn supported_extensions(&self) -> Vec<String> {
        let mut extensions: Vec<_> = self.plugins_by_extension.keys().cloned().collect();
        extensions.sort();
        extensions
    }

    pub fn plugin_for_extension(&self, extension: &str) -> Option<Arc<dyn LanguagePlugin>> {
        self.plugins_by_extension.get(extension).cloned()
    }

    pub fn plugin_for_path(&self, path: &Path) -> anyhow::Result<Arc<dyn LanguagePlugin>> {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .ok_or_else(|| anyhow!("unsupported language for file: {}", path.display()))?;
        self.plugin_for_extension(extension)
            .ok_or_else(|| anyhow!("unsupported language for file: {}", path.display()))
    }

    pub fn plugins(&self) -> &[Arc<dyn LanguagePlugin>] {
        &self.plugins
    }

    pub fn collect_classifiers(&self) -> Vec<Box<dyn Classifier>> {
        self.plugins
            .iter()
            .flat_map(|plugin| plugin.classifiers())
            .collect()
    }

    pub fn collect_graph_passes(&self) -> Vec<Box<dyn GraphPass>> {
        self.plugins
            .iter()
            .flat_map(|plugin| plugin.graph_passes())
            .collect()
    }

    pub fn prepare_plugins(&self, context: &ProjectContext) -> anyhow::Result<()> {
        for plugin in &self.plugins {
            plugin
                .prepare_project(context)
                .with_context(|| format!("failed to prepare plugin '{}'", plugin.id()))?;
        }
        Ok(())
    }

    pub fn finish_plugins(&self, context: &ProjectContext) -> anyhow::Result<()> {
        for plugin in &self.plugins {
            plugin
                .finish_project(context)
                .with_context(|| format!("failed to finish plugin '{}'", plugin.id()))?;
        }
        Ok(())
    }
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::ExtractionResult;

    struct TestPlugin {
        id: &'static str,
        exts: &'static [&'static str],
    }

    impl LanguagePlugin for TestPlugin {
        fn id(&self) -> &'static str {
            self.id
        }

        fn extensions(&self) -> &'static [&'static str] {
            self.exts
        }

        fn extract(
            &self,
            _source: &[u8],
            _context: &FileContext,
        ) -> anyhow::Result<ExtractionResult> {
            Ok(ExtractionResult::new())
        }
    }

    #[test]
    fn rejects_duplicate_extensions() {
        let mut registry = LanguageRegistry::new();
        registry
            .register(TestPlugin {
                id: "first",
                exts: &["rs"],
            })
            .unwrap();

        let error = registry
            .register(TestPlugin {
                id: "second",
                exts: &["rs"],
            })
            .unwrap_err();

        assert!(error.to_string().contains("conflicts"));
    }
}
