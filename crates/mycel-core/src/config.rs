use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Minimal,
    Balanced,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsConfig {
    pub tier: Option<Tier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub falkordb_url: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self { falkordb_url: "redis://localhost:6379".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProviderConfig {
    Ollama {
        endpoint: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        dimension: Option<u32>,
    },
}

impl ProviderConfig {
    pub fn default_ollama() -> Self {
        ProviderConfig::Ollama {
            endpoint: "http://localhost:11434".into(),
            model: None,
            dimension: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersConfig {
    pub embedder: Option<ProviderConfig>,
    pub synthesizer: Option<ProviderConfig>,
    pub reranker: Option<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspConfig {
    pub multilspy_path: String,
    pub languages: Vec<String>,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            multilspy_path: "python3 scripts/multilspy_bridge.py".into(),
            languages: vec!["typescript".into(), "rust".into()],
        }
    }
}
