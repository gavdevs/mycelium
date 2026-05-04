use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mycel", version)]
pub struct Cli {
    /// Output as JSON
    #[arg(long, global = true)]
    pub json: bool,
    /// Repo root for resolving .mycel.toml
    #[arg(long, global = true)]
    pub repo: Option<Utf8PathBuf>,
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    Index {
        path: Utf8PathBuf,
        /// Skip the Phase 2 description-synthesis pass at the end of indexing.
        /// Use this for fast cold first-indexes where signature embeddings are
        /// good enough; you can run `mycel synthesize` later to upgrade.
        #[arg(long)]
        no_descriptions: bool,
    },
    Callers { symbol: String },
    Callees { symbol: String },
    Definers { name: String },
    Imports { file: String },
    Uses { ty: String },
    Implements { iface: String },
    Find { query: String, #[arg(long, default_value_t = 8)] limit: usize },
    /// Run only the Phase 2 description-synthesis pass on an already-indexed
    /// graph. Idempotent — symbols with an existing description are skipped
    /// unless `--force` is set.
    Synthesize {
        #[arg(long)]
        force: bool,
        #[arg(long)]
        limit: Option<usize>,
    },
    Daemon { #[command(subcommand)] action: DaemonAction },
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Register the daemon with the platform supervisor for a repo.
    /// Defaults to the current working directory if no repo is given.
    Install { repo: Option<Utf8PathBuf> },
    Uninstall,
    Start,
    Stop,
    Status,
    Logs { #[arg(long)] follow: bool },
    Run,
}
