mod watcher;

use anyhow::Result;
use camino::Utf8PathBuf;
use mycel_graph::GraphClient;
use mycel_index::Indexer;
use mycel_lsp::MultilspyResolver;
use mycel_models::OllamaEmbedder;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let log_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("no home dir"))?
        .join(".cache/mycel");
    std::fs::create_dir_all(&log_dir)?;
    let file_appender = tracing_appender::rolling::daily(&log_dir, "daemon.log");
    let (nb, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(nb)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    info!("daemon starting");

    let repo_str = std::env::var("MYCEL_REPO").unwrap_or_else(|_| ".".into());
    let repo: Utf8PathBuf = repo_str.into();
    let repo_canon = repo.canonicalize_utf8()?;

    let url = std::env::var("MYCEL_FALKORDB_URL")
        .unwrap_or_else(|_| "redis://localhost:6379".into());
    let graph_name = format!("mycel:{}", repo_id_from_path(&repo_canon));
    let graph = GraphClient::connect(&url, &graph_name).await?;
    let embedder: Arc<dyn mycel_models::Embedder> =
        Arc::new(OllamaEmbedder::new("http://localhost:11434", "embeddinggemma"));
    let bridge_cmd = format!("python3 {repo_canon}/scripts/multilspy_bridge.py");
    let lsp = MultilspyResolver::spawn(&bridge_cmd, repo_canon.clone())
        .await
        .ok()
        .map(Arc::new);
    let indexer = Indexer { graph, lsp, embedder };

    info!("starting initial index");
    let n = indexer.index_repo(&repo_canon).await?;
    info!("indexed {n} files");

    let mut rx = watcher::spawn(repo_canon.clone());
    while let Some(path) = rx.recv().await {
        if mycel_extract::for_language(&path).is_none() {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                if let Err(e) = indexer.index_file(&path, &content).await {
                    warn!(file=%path, error=%e, "index_file failed");
                }
            }
            Err(_) => { /* file deleted between event and read; skip */ }
        }
    }
    Ok(())
}

/// Stable repo id derived from the canonical path. MUST match the CLI's
/// equivalent function in `mycel-cli/src/main.rs::repo_id_from_path` so the
/// daemon and CLI converge on the same FalkorDB graph for a given repo.
/// Format: `<basename>-<8 hex chars of blake3(canonical path)>`.
fn repo_id_from_path(canon: &camino::Utf8Path) -> String {
    let basename = canon.file_name().unwrap_or("repo");
    let hash = blake3::hash(canon.as_str().as_bytes()).to_hex();
    format!("{basename}-{}", &hash.as_str()[..8])
}
