//! mycel-models — Embedder/Synthesizer/Reranker trait surface and Ollama provider.
//!
//! Phase 1 shipped Embedder + Synthesizer; Phase 3 fills in the Reranker
//! (`OllamaReranker`) against the qwen3-reranker family.

use async_trait::async_trait;
use mycel_core::Result;

#[async_trait]
pub trait Embedder: Send + Sync {
    /// Stable provider+model identity, e.g., "ollama/embeddinggemma".
    fn identity(&self) -> &str;
    /// Output vector dimension.
    fn dimension(&self) -> u32;
    /// Embed a batch of texts.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

#[async_trait]
pub trait Synthesizer: Send + Sync {
    fn identity(&self) -> &str;
    async fn synthesize(&self, prompt: &str) -> Result<String>;
}

#[async_trait]
pub trait Reranker: Send + Sync {
    fn identity(&self) -> &str;
    /// Score each candidate against the query. Higher = more relevant.
    async fn rerank(&self, query: &str, candidates: &[&str]) -> Result<Vec<f32>>;
}

pub mod ollama;
pub use ollama::{OllamaEmbedder, OllamaReranker, OllamaSynthesizer};
