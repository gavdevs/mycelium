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
    endpoint: String,
    model: String,
    identity: String,
    client: reqwest::Client,
    concurrency: usize,
}

impl OllamaReranker {
    /// `concurrency` is the max in-flight rerank requests; tune to match
    /// Ollama's `num_parallel` (default 4; raise to 8 on Max tier).
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>, concurrency: usize) -> Self {
        let model = model.into();
        let identity = format!("ollama/{model}");
        Self {
            endpoint: endpoint.into(),
            model,
            identity,
            // 5s per call. The Tier-4 pipeline applies its own outer budget;
            // individual stragglers shouldn't drag the whole rerank batch.
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("reqwest client builds with default tls"),
            concurrency: concurrency.max(1),
        }
    }
}

#[async_trait]
impl Reranker for OllamaReranker {
    fn identity(&self) -> &str {
        &self.identity
    }
    async fn rerank(&self, query: &str, candidates: &[&str]) -> Result<Vec<f32>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        use futures::stream::{self, StreamExt};
        // Build prompts up front so the async closures don't need to borrow
        // `candidates` across awaits — sidesteps a higher-ranked-lifetime
        // mismatch that bites when using `&[&str]` inside `buffer_unordered`.
        let prompts: Vec<(usize, String)> = candidates
            .iter()
            .enumerate()
            .map(|(i, cand)| (i, build_rerank_prompt(query, cand)))
            .collect();
        let scored = stream::iter(prompts)
            .map(|(i, prompt)| {
                let url = format!("{}/api/generate", self.endpoint);
                let client = self.client.clone();
                let model = self.model.clone();
                let identity = self.identity.clone();
                async move {
                    let body = serde_json::json!({
                        "model": model,
                        "prompt": prompt,
                        "stream": false,
                        "options": {"temperature": 0.0}
                    });
                    let resp_result = client.post(&url).json(&body).send().await;
                    match resp_result.and_then(|r| r.error_for_status()) {
                        Ok(resp) => match resp.json::<GenerateResponse>().await {
                            Ok(parsed) => (i, parse_rerank_response(&parsed.response)),
                            Err(e) => {
                                tracing::warn!(provider = %identity, error = %e, "rerank: bad JSON, scoring as -inf");
                                (i, f32::NEG_INFINITY)
                            }
                        },
                        Err(e) => {
                            tracing::warn!(provider = %identity, error = %e, "rerank: request failed, scoring as -inf");
                            (i, f32::NEG_INFINITY)
                        }
                    }
                }
            })
            .buffer_unordered(self.concurrency)
            .collect::<Vec<_>>()
            .await;
        let mut out = vec![f32::NEG_INFINITY; candidates.len()];
        for (i, score) in scored {
            out[i] = score;
        }
        // If every candidate returned -inf, the reranker is effectively dead;
        // surface that loudly so Tier-4 can fall through to cosine ordering.
        if out.iter().all(|s| s.is_infinite()) {
            return Err(MycelError::Model {
                provider: self.identity.clone(),
                message: "rerank: every candidate failed; reranker is unhealthy".into(),
            });
        }
        Ok(out)
    }
}

fn build_rerank_prompt(query: &str, doc: &str) -> String {
    // Trim docs to ~1500 chars to keep prompt sizes manageable; the reranker
    // doesn't need the whole body, just enough to judge relevance.
    let doc_trunc: String = doc.chars().take(1500).collect();
    format!("Query: {query}\nDocument: {doc_trunc}\nRelevant (yes/no):")
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

    #[test]
    fn rerank_prompt_includes_query_and_document() {
        let p = build_rerank_prompt("validate auth", "fn check_token() {}");
        assert!(p.contains("Query: validate auth"));
        assert!(p.contains("Document: fn check_token() {}"));
        assert!(p.ends_with("Relevant (yes/no):"));
    }

    #[test]
    fn rerank_prompt_truncates_long_docs() {
        let long_doc = "x".repeat(5000);
        let p = build_rerank_prompt("q", &long_doc);
        assert!(p.len() <= 1600, "prompt was {} chars", p.len());
    }
}
