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
    Index { path: Utf8PathBuf },
    Callers { symbol: String },
    Callees { symbol: String },
    Definers { name: String },
    Imports { file: String },
    Uses { ty: String },
    Implements { iface: String },
    Find { query: String, #[arg(long, default_value_t = 8)] limit: usize },
    Daemon { #[command(subcommand)] action: DaemonAction },
}

#[derive(Subcommand)]
pub enum DaemonAction {
    Install,
    Uninstall,
    Start,
    Stop,
    Status,
    Logs { #[arg(long)] follow: bool },
    Run,
}
