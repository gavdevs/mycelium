mod cli;
mod config;
mod output;
mod skill_install;
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
        Cmd::Index { path, force_cold_rebuild } => {
            let graph_name = std::env::var("MYCEL_TEST_GRAPH")
                .unwrap_or_else(|_| format!("mycel:{}", repo_id_from_path(&path)));
            let g = GraphClient::connect(&cfg.storage.falkordb_url, &graph_name).await?;
            if force_cold_rebuild {
                let n = g.clear_all_descriptions().await?;
                println!("cleared {n} description(s) before re-index");
            }
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
            let indexer = Indexer { graph: g, lsp, embedder };
            let n = indexer.index_repo(&path).await?;
            println!("indexed {n} files");
        }
        Cmd::Synthesize { force, limit, refresh_hashes_only } => {
            let g = open(&cfg, &cli.repo).await?;
            if refresh_hashes_only {
                let n = g.refresh_description_source_hashes().await?;
                println!("backfilled {n} legacy description_source_hash row(s)");
                return Ok(());
            }
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
        Cmd::SetDescription { qname, description } => {
            let g = open(&cfg, &cli.repo).await?;
            // Verify the qname resolves before paying for an embedding.
            if g.get_symbol_description(&qname).await?.is_none() {
                anyhow::bail!(
                    "no Symbol with qualified_name '{qname}' — run `mycel definers <name>` to find the canonical qname"
                );
            }
            let embedder = config::embedder_from_cfg(&cfg);
            let mut vecs = embedder
                .embed(&[description.as_str()])
                .await
                .context("embed description")?;
            let vec = vecs
                .pop()
                .ok_or_else(|| anyhow::anyhow!("embedder returned no vectors"))?;
            g.set_symbol_description_and_embedding(&qname, &description, &vec)
                .await
                .context("write description + embedding")?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({"qualified_name": qname, "ok": true})
                );
            } else {
                println!("ok");
            }
        }
        Cmd::Describe { qname } => {
            let g = open(&cfg, &cli.repo).await?;
            let info = g
                .get_symbol_description(&qname)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no Symbol with qualified_name '{qname}'"))?;
            if json {
                let payload = serde_json::json!({
                    "qualified_name": qname,
                    "description": info.description,
                    "body_hash": info.body_hash,
                    "description_source_hash": info.description_source_hash,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                match info.description {
                    Some(d) => println!("{d}"),
                    None => println!("(none)"),
                }
            }
        }
        Cmd::Skill { action } => match action {
            cli::SkillAction::Install => {
                let repo = cli.repo.clone().unwrap_or_else(|| ".".into());
                let source = skill_install::source_for_repo(&repo);
                let target = skill_install::default_target()?;
                let outcome = skill_install::install(&source, &target)?;
                println!("installed: {} -> {}", outcome.target, outcome.source);
            }
        },
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
    // MYCEL_TEST_GRAPH overrides the derived graph name. Test-only — production
    // callers leave it unset; cli_integration tests use it to target deterministic
    // graph names for setup + assertion.
    let graph_name = std::env::var("MYCEL_TEST_GRAPH").unwrap_or_else(|_| {
        format!(
            "mycel:{}",
            repo.as_deref()
                .map(repo_id_from_path)
                .unwrap_or_else(|| "default".into())
        )
    });
    GraphClient::connect(&cfg.storage.falkordb_url, &graph_name).await
        .context("connect to FalkorDB")
}
