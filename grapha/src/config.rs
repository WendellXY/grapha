use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GraphaConfig {
    #[serde(default)]
    pub classifiers: Vec<ClassifierRule>,
    #[serde(default)]
    #[allow(dead_code)]
    pub entry_points: Vec<EntryPointRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClassifierRule {
    pub pattern: String,
    pub terminal: String,
    pub direction: String,
    pub operation: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct EntryPointRule {
    pub language: String,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub attribute: Option<String>,
}

pub fn load_config(project_root: &Path) -> GraphaConfig {
    let config_path = project_root.join("grapha.toml");
    if !config_path.exists() {
        return GraphaConfig::default();
    }
    match std::fs::read_to_string(&config_path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
        Err(_) => GraphaConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn parse_empty_config() {
        let config: GraphaConfig = toml::from_str("").unwrap();
        assert!(config.classifiers.is_empty());
        assert!(config.entry_points.is_empty());
    }

    #[test]
    fn parse_classifier_rules() {
        let toml_str = r#"
[[classifiers]]
pattern = "URLSession"
terminal = "network"
direction = "read"
operation = "HTTP_GET"

[[classifiers]]
pattern = "CoreData"
terminal = "persistence"
direction = "write"
operation = "INSERT"
"#;
        let config: GraphaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.classifiers.len(), 2);
        assert_eq!(config.classifiers[0].pattern, "URLSession");
        assert_eq!(config.classifiers[0].terminal, "network");
        assert_eq!(config.classifiers[0].direction, "read");
        assert_eq!(config.classifiers[0].operation, "HTTP_GET");
        assert_eq!(config.classifiers[1].pattern, "CoreData");
        assert_eq!(config.classifiers[1].terminal, "persistence");
    }

    #[test]
    fn parse_entry_point_rules() {
        let toml_str = r#"
[[entry_points]]
language = "swift"
attribute = "@main"

[[entry_points]]
language = "rust"
pattern = "^main$"
"#;
        let config: GraphaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.entry_points.len(), 2);
        assert_eq!(config.entry_points[0].language, "swift");
        assert_eq!(config.entry_points[0].attribute.as_deref(), Some("@main"));
        assert!(config.entry_points[0].pattern.is_none());
        assert_eq!(config.entry_points[1].language, "rust");
        assert_eq!(config.entry_points[1].pattern.as_deref(), Some("^main$"));
        assert!(config.entry_points[1].attribute.is_none());
    }

    #[test]
    fn load_missing_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let config = load_config(dir.path());
        assert!(config.classifiers.is_empty());
        assert!(config.entry_points.is_empty());
    }

    #[test]
    fn load_from_file_works() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("grapha.toml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(
            f,
            r#"
[[classifiers]]
pattern = "reqwest"
terminal = "network"
direction = "read_write"
operation = "HTTP"
"#
        )
        .unwrap();

        let config = load_config(dir.path());
        assert_eq!(config.classifiers.len(), 1);
        assert_eq!(config.classifiers[0].pattern, "reqwest");
    }
}
