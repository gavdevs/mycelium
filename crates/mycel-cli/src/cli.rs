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
        /// Wipe all existing descriptions before indexing — useful when
        /// abandoning eager-synth descriptions in favor of workload-driven
        /// fill via the mycel-graph-care skill.
        #[arg(long)]
        force_cold_rebuild: bool,
    },
    Callers { symbol: String },
    Callees { symbol: String },
    Definers { name: String },
    Imports { file: String },
    Uses { ty: String },
    Implements { iface: String },
    /// `--limit` default is 20 because FalkorDB's HNSW vector index walks the
    /// k-nearest graph adaptively and can return zero rows at very low k for
    /// embeddings that don't have many close neighbors. Bumping the default
    /// keeps single-call invocations honest on Phase 2 description embeddings,
    /// where good matches sometimes sit slightly farther in cosine space than
    /// signature embeddings did.
    Find { query: String, #[arg(long, default_value_t = 20)] limit: usize },
    /// Run only the Phase 2 description-synthesis pass on an already-indexed
    /// graph. Idempotent — symbols with an existing description are skipped
    /// unless `--force` is set.
    Synthesize {
        #[arg(long)]
        force: bool,
        #[arg(long)]
        limit: Option<usize>,
        /// Backfill description_source_hash from body_hash for legacy rows
        /// (descriptions written before 2026-05-05). Skips Ollama entirely.
        #[arg(long, conflicts_with_all = ["force", "limit"])]
        refresh_hashes_only: bool,
    },
    /// Print the current synthesized description for a Symbol, or "(none)".
    /// With --json, emits {qualified_name, description, body_hash, description_source_hash}.
    Describe { qname: String },
    /// Write a behavioral description for a Symbol. Embeds the description
    /// text and atomically updates the Symbol's description + embedding +
    /// description_source_hash. Used by the mycel-graph-care skill to record
    /// understanding gained while reading code.
    SetDescription {
        /// The Symbol's qualified_name. Get this from
        /// `mycel definers <name> --json`.
        #[arg(long)]
        qname: String,
        /// 1-3 sentence behavioral description.
        #[arg(long)]
        description: String,
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
