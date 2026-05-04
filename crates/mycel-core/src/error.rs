use thiserror::Error;

#[derive(Error, Debug)]
pub enum MycelError {
    #[error("graph error: {0}")]
    Graph(String),
    #[error("extraction error in {file}: {message}")]
    Extract { file: String, message: String },
    #[error("LSP error: {0}")]
    Lsp(String),
    #[error("model provider error ({provider}): {message}")]
    Model { provider: String, message: String },
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("embedder mismatch: graph manifest has {graph}, runtime has {runtime}")]
    EmbedderMismatch { graph: String, runtime: String },
}

pub type Result<T> = std::result::Result<T, MycelError>;
