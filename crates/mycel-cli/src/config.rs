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

/// Build a Synthesizer from config, falling back to the tier-default Ollama
/// model when the user hasn't pinned one. Returns None only if the user
/// explicitly sets `MYCEL_SYNTHESIZER=off` (escape hatch for incremental
/// indexing without LLM cost).
pub fn synthesizer_from_cfg(cfg: &Config) -> Option<std::sync::Arc<dyn mycel_models::Synthesizer>> {
    if std::env::var("MYCEL_SYNTHESIZER").as_deref() == Ok("off") {
        return None;
    }
    let provider = cfg
        .providers
        .synthesizer
        .clone()
        .unwrap_or_else(ProviderConfig::default_ollama);
    match provider {
        ProviderConfig::Ollama { endpoint, model, .. } => {
            let model = std::env::var("MYCEL_SYNTHESIZER_MODEL")
                .ok()
                .or(model)
                .unwrap_or_else(|| default_synthesizer_model(cfg.models.tier));
            Some(std::sync::Arc::new(mycel_models::OllamaSynthesizer::new(
                endpoint, model,
            )))
        }
    }
}

/// Build a Reranker from config, falling back to the tier-default Ollama
/// model when the user hasn't pinned one. Returns None only if the user
/// explicitly sets `MYCEL_RERANKER=off` (escape hatch for Tier-4 testing
/// without rerank cost — falls through to cosine-only ordering).
///
/// Not yet called from `main.rs`; Workstream C will wire it into the Tier-4
/// query path. Keep `#[allow(dead_code)]` until that lands.
#[allow(dead_code)]
pub fn reranker_from_cfg(cfg: &Config) -> Option<std::sync::Arc<dyn mycel_models::Reranker>> {
    if std::env::var("MYCEL_RERANKER").as_deref() == Ok("off") {
        return None;
    }
    let provider = cfg
        .providers
        .reranker
        .clone()
        .unwrap_or_else(ProviderConfig::default_ollama);
    let concurrency = match cfg.models.tier.unwrap_or_else(detect_tier) {
        Tier::Minimal | Tier::Balanced => 4,
        Tier::Max => 8,
    };
    match provider {
        ProviderConfig::Ollama { endpoint, model, .. } => {
            let model = std::env::var("MYCEL_RERANKER_MODEL")
                .ok()
                .or(model)
                .unwrap_or_else(|| default_reranker_model(cfg.models.tier));
            Some(std::sync::Arc::new(mycel_models::OllamaReranker::new(
                endpoint, model, concurrency,
            )))
        }
    }
}

/// Pick the default synthesizer model for a tier, mirroring DESIGN.md's
/// tier table. If `tier` is None, auto-detect from system memory.
fn default_synthesizer_model(tier: Option<Tier>) -> String {
    let tier = tier.unwrap_or_else(detect_tier);
    match tier {
        Tier::Minimal => "gemma4:e2b".into(),
        Tier::Balanced => "gemma4:e4b".into(),
        Tier::Max => "qwen3.6:35b-a3b".into(),
    }
}

/// Pick the default reranker model for a tier, mirroring DESIGN.md's
/// tier table. Minimal/Balanced share the small model; Max upgrades to 4b
/// (the swap is free at query time — no reindex implications).
#[allow(dead_code)]
fn default_reranker_model(tier: Option<Tier>) -> String {
    let tier = tier.unwrap_or_else(detect_tier);
    match tier {
        Tier::Minimal | Tier::Balanced => "qwen3-reranker:0.6b".into(),
        Tier::Max => "qwen3-reranker:4b".into(),
    }
}

/// Memory-based tier detection mirroring scripts/bootstrap.sh:
/// <12 GB → minimal, <24 GB → balanced, ≥24 GB → max. Falls back to
/// `balanced` on platforms without a /proc/meminfo (macOS), which is the
/// safest middle ground for systems with unknown memory.
fn detect_tier() -> Tier {
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                if let Some(kb) = rest.split_whitespace().next() {
                    if let Ok(kb) = kb.parse::<u64>() {
                        let gb = kb / 1024 / 1024;
                        return if gb < 12 {
                            Tier::Minimal
                        } else if gb < 24 {
                            Tier::Balanced
                        } else {
                            Tier::Max
                        };
                    }
                }
            }
        }
    }
    Tier::Balanced
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycel_core::Tier;

    #[test]
    fn reranker_default_per_tier() {
        assert_eq!(default_reranker_model(Some(Tier::Minimal)), "qwen3-reranker:0.6b");
        assert_eq!(default_reranker_model(Some(Tier::Balanced)), "qwen3-reranker:0.6b");
        assert_eq!(default_reranker_model(Some(Tier::Max)), "qwen3-reranker:4b");
    }
}
