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

/// Maps a qwen3-reranker-style yes/no completion to a 0/1 score.
///
/// Returns `f32::NEG_INFINITY` for responses we can't classify, so the Tier-4
/// caller can treat them as "no opinion" rather than "definite no". Case- and
/// whitespace-insensitive; only the first lexeme matters because we set
/// `temperature: 0.0` and expect a single-token answer.
#[allow(dead_code)] // wired into OllamaReranker::rerank in a follow-up task
fn parse_rerank_response(raw: &str) -> f32 {
    let head = raw.split_whitespace().next().unwrap_or("");
    let head = head.trim_end_matches(|c: char| !c.is_alphanumeric()).to_ascii_lowercase();
    match head.as_str() {
        "yes" => 1.0,
        "no" => 0.0,
        _ => f32::NEG_INFINITY,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_affirmative_responses_as_one() {
        for s in ["yes", "Yes", "  yes  ", "yes.", "yes\n"] {
            assert_eq!(parse_rerank_response(s), 1.0, "expected 1.0 for {s:?}");
        }
    }

    #[test]
    fn parses_negative_responses_as_zero() {
        for s in ["no", "No", "  no  ", "no.", "no\n"] {
            assert_eq!(parse_rerank_response(s), 0.0, "expected 0.0 for {s:?}");
        }
    }

    #[test]
    fn unparseable_responses_are_neg_infinity() {
        for s in ["", "maybe", "definitely", "  "] {
            assert!(parse_rerank_response(s).is_infinite() && parse_rerank_response(s).is_sign_negative(),
                "expected -inf for {s:?}");
        }
    }
}
