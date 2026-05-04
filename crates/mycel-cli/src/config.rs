//! Layered config loading: defaults -> ~/.config/mycel/config.toml -> <repo>/.mycel.toml -> env.
//!
//! Layered values use `Option<T>` to track presence; only `Some(...)` values
//! from later layers replace earlier ones. Defaults are applied LAST when
//! materializing the resolved config — so a `Default::default()` field on an
//! intermediate layer never accidentally beats a real user/repo value.

use mycel_core::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigLayer {
    #[serde(default)] pub models: Option<ModelsConfig>,
    #[serde(default)] pub storage: Option<StorageOverlay>,
    #[serde(default)] pub providers: Option<ProvidersOverlay>,
    #[serde(default)] pub lsp: Option<LspConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageOverlay {
    #[serde(default)] pub falkordb_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvidersOverlay {
    #[serde(default)] pub embedder: Option<ProviderConfig>,
    #[serde(default)] pub synthesizer: Option<ProviderConfig>,
    #[serde(default)] pub reranker: Option<ProviderConfig>,
}

/// The fully resolved config used by the CLI/daemon. All fields are populated.
#[derive(Debug, Clone)]
pub struct Config {
    /// Models tier selection. Read by Phase 2/3 (synthesizer/reranker
    /// wiring); the Phase 1 CLI doesn't consult it directly.
    #[allow(dead_code)]
    pub models: ModelsConfig,
    pub storage: StorageConfig,
    pub providers: ProvidersConfig,
    pub lsp: LspConfig,
}

fn merge(into: &mut ConfigLayer, from: ConfigLayer) {
    if from.models.is_some() { into.models = from.models; }
    if let Some(s) = from.storage {
        let target = into.storage.get_or_insert_with(StorageOverlay::default);
        if s.falkordb_url.is_some() { target.falkordb_url = s.falkordb_url; }
    }
    if let Some(p) = from.providers {
        let target = into.providers.get_or_insert_with(ProvidersOverlay::default);
        if p.embedder.is_some()    { target.embedder    = p.embedder; }
        if p.synthesizer.is_some() { target.synthesizer = p.synthesizer; }
        if p.reranker.is_some()    { target.reranker    = p.reranker; }
    }
    if from.lsp.is_some() { into.lsp = from.lsp; }
}

pub fn load(repo: Option<&camino::Utf8Path>) -> Result<Config> {
    let mut layered = ConfigLayer::default();

    // user-global
    if let Some(home) = std::env::var_os("HOME") {
        let home: camino::Utf8PathBuf = camino::Utf8PathBuf::from_path_buf(home.into())
            .map_err(|_| MycelError::Config("non-utf8 HOME".into()))?;
        let user = home.join(".config/mycel/config.toml");
        if user.exists() {
            let s = std::fs::read_to_string(&user)?;
            let parsed: ConfigLayer = toml::from_str(&s)
                .map_err(|e| MycelError::Config(format!("user config: {e}")))?;
            merge(&mut layered, parsed);
        }
    }

    // per-repo
    if let Some(repo) = repo {
        let repo_cfg = repo.join(".mycel.toml");
        if repo_cfg.exists() {
            let s = std::fs::read_to_string(&repo_cfg)?;
            let parsed: ConfigLayer = toml::from_str(&s)
                .map_err(|e| MycelError::Config(format!("repo config: {e}")))?;
            merge(&mut layered, parsed);
        }
    }

    // env overrides (highest precedence)
    if let Ok(url) = std::env::var("MYCEL_FALKORDB_URL") {
        layered.storage.get_or_insert_with(StorageOverlay::default).falkordb_url = Some(url);
    }

    // materialize with defaults last
    let storage = StorageConfig {
        falkordb_url: layered.storage.and_then(|s| s.falkordb_url)
            .unwrap_or_else(|| "redis://localhost:6379".into()),
    };
    let providers_overlay = layered.providers.unwrap_or_default();
    let providers = ProvidersConfig {
        embedder:    providers_overlay.embedder,
        synthesizer: providers_overlay.synthesizer,
        reranker:    providers_overlay.reranker,
    };
    Ok(Config {
        models: layered.models.unwrap_or_default(),
        storage,
        providers,
        lsp: layered.lsp.unwrap_or_default(),
    })
}

pub fn embedder_from_cfg(cfg: &Config) -> std::sync::Arc<dyn mycel_models::Embedder> {
    let provider = cfg.providers.embedder.clone().unwrap_or(ProviderConfig::default_ollama());
    match provider {
        ProviderConfig::Ollama { endpoint, model, .. } => {
            let model = model.unwrap_or_else(|| "embeddinggemma".into());
            std::sync::Arc::new(mycel_models::OllamaEmbedder::new(endpoint, model))
        }
    }
}
