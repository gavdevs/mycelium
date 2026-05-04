use crate::{Embedder, Reranker, Synthesizer};
use async_trait::async_trait;
use mycel_core::*;
use serde::Deserialize;
use serde_json::json;

pub struct OllamaEmbedder {
    endpoint: String,
    model: String,
    dimension: u32,
    identity: String,
    client: reqwest::Client,
}

impl OllamaEmbedder {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        let model = model.into();
        let identity = format!("ollama/{model}");
        // EmbeddingGemma defaults to 768d.
        Self {
            endpoint: endpoint.into(),
            model,
            dimension: 768,
            identity,
            client: reqwest::Client::new(),
        }
    }
    pub fn with_dimension(mut self, d: u32) -> Self {
        self.dimension = d;
        self
    }
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    fn identity(&self) -> &str {
        &self.identity
    }
    fn dimension(&self) -> u32 {
        self.dimension
    }
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embed", self.endpoint);
        let body = json!({"model": self.model, "input": texts});
        let resp: EmbedResponse = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| MycelError::Model {
                provider: self.identity.clone(),
                message: format!("send: {e}"),
            })?
            .error_for_status()
            .map_err(|e| MycelError::Model {
                provider: self.identity.clone(),
                message: format!("status: {e}"),
            })?
            .json()
            .await
            .map_err(|e| MycelError::Model {
                provider: self.identity.clone(),
                message: format!("json: {e}"),
            })?;
        // Validate dimension matches the embedder's declared dimension. If
        // the model returns a different dimension, fail loudly here rather
        // than letting a corrupt vector reach the FalkorDB vector index.
        if let Some(first) = resp.embeddings.first() {
            if first.len() as u32 != self.dimension {
                return Err(MycelError::Model {
                    provider: self.identity.clone(),
                    message: format!(
                        "embedding dimension mismatch: model returned {} but embedder declares {}",
                        first.len(),
                        self.dimension
                    ),
                });
            }
        }
        Ok(resp.embeddings)
    }
}

pub struct OllamaSynthesizer {
    endpoint: String,
    model: String,
    identity: String,
    client: reqwest::Client,
}

impl OllamaSynthesizer {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        let model = model.into();
        let identity = format!("ollama/{model}");
        Self {
            endpoint: endpoint.into(),
            model,
            identity,
            client: reqwest::Client::builder()
                // Local-Ollama generation can take 10–30s per call on small
                // models and far longer on the qwen3.6:35b-a3b max tier; the
                // default 30s reqwest timeout would silently truncate them.
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("reqwest client builds with default tls"),
        }
    }
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

#[async_trait]
impl Synthesizer for OllamaSynthesizer {
    fn identity(&self) -> &str {
        &self.identity
    }
    async fn synthesize(&self, prompt: &str) -> Result<String> {
        let url = format!("{}/api/generate", self.endpoint);
        // `stream:false` makes Ollama return one JSON object instead of a
        // newline-delimited stream — we want the whole completion in one shot.
        // Low temperature so descriptions are deterministic across re-runs.
        let body = json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "options": {"temperature": 0.2}
        });
        let resp: GenerateResponse = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| MycelError::Model {
                provider: self.identity.clone(),
                message: format!("send: {e}"),
            })?
            .error_for_status()
            .map_err(|e| MycelError::Model {
                provider: self.identity.clone(),
                message: format!("status: {e}"),
            })?
            .json()
            .await
            .map_err(|e| MycelError::Model {
                provider: self.identity.clone(),
                message: format!("json: {e}"),
            })?;
        Ok(resp.response.trim().to_string())
    }
}

pub struct OllamaReranker {
    pub endpoint: String,
    pub model: String,
}
#[async_trait]
impl Reranker for OllamaReranker {
    fn identity(&self) -> &str {
        &self.model
    }
    async fn rerank(&self, _query: &str, _candidates: &[&str]) -> Result<Vec<f32>> {
        todo!("phase 3 — wire reranker endpoint")
    }
}
