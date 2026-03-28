use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum GraphaError {
    #[error("failed to parse {path}: {reason}")]
    Parse { path: PathBuf, reason: String },

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("unsupported language for file: {path}")]
    UnsupportedLanguage { path: PathBuf },
}
