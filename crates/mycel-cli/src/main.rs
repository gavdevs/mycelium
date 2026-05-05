mod cli;
mod config;
mod output;
mod supervisor;

use anyhow::Context;
use camino::Utf8PathBuf;
use clap::Parser;
use cli::{Cli, Cmd};
use mycel_graph::GraphClient;
use mycel_index::Indexer;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let cli = Cli::parse();
    let cfg = config::load(cli.repo.as_deref())?;
    let json = cli.json;

    match cli.command {
        Cmd::Index { path, no_descriptions } => {
            let graph_name = format!("mycel:{}", repo_id_from_path(&path));
            let g = GraphClient::connect(&cfg.storage.falkordb_url, &graph_name).await?;
            let embedder = config::embedder_from_cfg(&cfg);
            // LSP refinement runs even from `mycel index` so Tier 1 queries
            // return correct results (CALLS edges with `source: lsp` are the
            // authoritative ones; tree-sitter heuristics alone miss too many
            // cases). First-run is slower as a result; that's acceptable for
            // the v0 dogfood loop.
            let canon: Utf8PathBuf = path.canonicalize_utf8().unwrap_or(path.clone());
            let bridge_cmd = if cfg.lsp.multilspy_path == mycel_core::LspConfig::default().multilspy_path {
                // Default-path users get an auto-resolved absolute path so this works
                // regardless of CWD. Custom multilspy_path values are passed through verbatim.
                format!("python3 {canon}/scripts/multilspy_bridge.py")
            } else {
                cfg.lsp.multilspy_path.clone()
            };
            let lsp = match mycel_lsp::MultilspyResolver::spawn(
                &bridge_cmd, canon.clone()
            ).await {
                Ok(r) => Some(Arc::new(r)),
                Err(e) => {
                    eprintln!("LSP unavailable, falling back to tree-sitter only: {e}");
                    None
                }
            };
            let synthesizer = if no_descriptions {
                None
            } else {
                config::synthesizer_from_cfg(&cfg)
            };
            let indexer = Indexer { graph: g, lsp, embedder, synthesizer };
            let n = indexer.index_repo(&path).await?;
            println!("indexed {n} files");
        }
        Cmd::Synthesize { force, limit } => {
            let g = open(&cfg, &cli.repo).await?;
            let embedder = config::embedder_from_cfg(&cfg);
            let Some(synthesizer) = config::synthesizer_from_cfg(&cfg) else {
                eprintln!("MYCEL_SYNTHESIZER=off — refusing to run. Unset the env var or remove [providers.synthesizer] from config to enable.");
                std::process::exit(2);
            };
            let outcome = mycel_index::synthesize_descriptions(
                &g,
                synthesizer,
                embedder,
                mycel_index::SynthesisOptions { force, limit, ..Default::default() },
            ).await?;
            println!(
                "synthesized {} of {} (skipped {}, failed {})",
                outcome.synthesized, outcome.considered, outcome.skipped, outcome.failed
            );
        }
        Cmd::Callers { symbol } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::callers(&g, &symbol).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Callees { symbol } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::callees(&g, &symbol).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Definers { name } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::definers(&g, &name).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Imports { file } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::imports(&g, &file).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Uses { ty } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::uses(&g, &ty).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Implements { iface } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::implements(&g, &iface).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Find { query, limit } => {
            let g = open(&cfg, &cli.repo).await?;
            let embedder = config::embedder_from_cfg(&cfg);
            let r = mycel_query::find(&g, embedder.as_ref(), &query, limit).await?;
            output::print_find(&r, json);
        }
        Cmd::Daemon { action } => supervisor::dispatch(action).await?,
    }
    Ok(())
}

/// Stable repo id derived from the canonical path, so two repos with the
/// same final component (e.g. ~/work/foo and ~/personal/foo) don't collide.
/// Format: `<basename>-<8 hex chars of blake3(canonical path)>`.
fn repo_id_from_path(path: &camino::Utf8Path) -> String {
    let canon = path.canonicalize_utf8().unwrap_or_else(|_| path.to_path_buf());
    let basename = canon.file_name().unwrap_or("repo");
    let hash = blake3::hash(canon.as_str().as_bytes()).to_hex();
    format!("{basename}-{}", &hash.as_str()[..8])
}

async fn open(cfg: &config::Config, repo: &Option<Utf8PathBuf>) -> anyhow::Result<GraphClient> {
    let graph_name = format!("mycel:{}", repo.as_deref()
        .map(repo_id_from_path)
        .unwrap_or_else(|| "default".into()));
    GraphClient::connect(&cfg.storage.falkordb_url, &graph_name).await
        .context("connect to FalkorDB")
}
