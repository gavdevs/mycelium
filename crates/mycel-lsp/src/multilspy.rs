use crate::protocol::*;
use camino::Utf8PathBuf;
use mycel_core::*;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, error, warn};

/// Spawns `scripts/multilspy_bridge.py` as a long-running subprocess and
/// translates bridge responses into `Edge` records with `EdgeSource::Lsp`.
///
/// **PATH inheritance constraint:** the bridge requires the spawn process's
/// PATH to resolve `python3` to a Python interpreter where multilspy is
/// importable. Configure `lsp.multilspy_path` to an absolute interpreter path
/// when the runtime PATH doesn't agree with the bootstrap PATH (common on
/// systems with multiple Pythons — Linuxbrew, mise-managed, system).
///
/// If the bridge subprocess dies (import error, crash, killed externally),
/// any in-flight `refine` calls fail with `MycelError::Lsp("bridge died: ...")`
/// instead of hanging forever waiting on a oneshot that will never resolve.
pub struct MultilspyResolver {
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<EdgesForFileResp>>>>,
    stdin: Arc<Mutex<ChildStdin>>,
    repo_root: Utf8PathBuf,
    dead: Arc<AtomicBool>,
}

impl MultilspyResolver {
    pub async fn spawn(cmd: &str, repo_root: Utf8PathBuf) -> Result<Self> {
        let mut parts = cmd.split_whitespace();
        let exe = parts.next().ok_or_else(|| MycelError::Lsp("empty cmd".into()))?;
        let args: Vec<&str> = parts.collect();

        let mut child = tokio::process::Command::new(exe)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| MycelError::Lsp(format!("spawn: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MycelError::Lsp("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MycelError::Lsp("no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| MycelError::Lsp("no stderr".into()))?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<EdgesForFileResp>>>> =
            Default::default();
        let dead = Arc::new(AtomicBool::new(false));

        tokio::spawn(reader_loop(stdout, pending.clone()));
        tokio::spawn(stderr_loop(stderr));
        tokio::spawn(wait_loop(child, pending.clone(), dead.clone()));

        Ok(Self {
            next_id: AtomicU64::new(1),
            pending,
            stdin: Arc::new(Mutex::new(stdin)),
            repo_root,
            dead,
        })
    }
}

async fn reader_loop(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<EdgesForFileResp>>>>,
) {
    let mut reader = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        debug!(line = %line, "bridge response");
        match serde_json::from_str::<EdgesForFileResp>(&line) {
            Ok(resp) => {
                if let Some(tx) = pending.lock().await.remove(&resp.id) {
                    let _ = tx.send(resp);
                }
            }
            Err(e) => warn!(error = %e, line = %line, "could not parse bridge response"),
        }
    }
}

async fn stderr_loop(stderr: ChildStderr) {
    let mut reader = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        // Bridge stderr is operationally important — surface at WARN so failures
        // don't get hidden under the default RUST_LOG=info filter.
        warn!(target: "mycel_lsp::bridge", line = %line);
    }
}

/// Reaps the child process and fails any in-flight requests when it exits.
/// Without this, a crashed bridge leaves `refine` callers waiting on a
/// oneshot that nothing will ever send to — a classic silent hang.
async fn wait_loop(
    mut child: tokio::process::Child,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<EdgesForFileResp>>>>,
    dead: Arc<AtomicBool>,
) {
    let status = child.wait().await;
    dead.store(true, Ordering::Release);
    let msg = match status {
        Ok(s) => format!("bridge exited: {s}"),
        Err(e) => format!("bridge wait failed: {e}"),
    };
    error!("{msg}");
    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(EdgesForFileResp {
            id: 0,
            edges: Vec::new(),
            partial: false,
            error: Some(msg.clone()),
        });
    }
}

impl MultilspyResolver {
    pub async fn refine(
        &self,
        path: &camino::Utf8Path,
        language: &str,
        _extraction: &mycel_extract::ExtractionOutput,
    ) -> Result<Vec<Edge>> {
        if self.dead.load(Ordering::Acquire) {
            return Err(MycelError::Lsp("bridge died earlier; refusing further requests".into()));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let req = EdgesForFileReq {
            id,
            op: "edges_for_file",
            repo_root: self.repo_root.as_str(),
            language,
            path: path.as_str(),
        };
        let line = serde_json::to_string(&req)? + "\n";
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| MycelError::Lsp(format!("write: {e}")))?;
            stdin
                .flush()
                .await
                .map_err(|e| MycelError::Lsp(format!("flush: {e}")))?;
        }
        let resp = rx
            .await
            .map_err(|_| MycelError::Lsp("bridge closed".into()))?;
        if let Some(err) = resp.error {
            warn!(language, %path, "LSP bridge error: {err}");
            return Ok(Vec::new());
        }
        if resp.partial {
            warn!(language, %path, "LSP refinement returned partial results");
        }
        Ok(resp
            .edges
            .into_iter()
            .filter_map(|r| {
                let kind = match r.kind.as_str() {
                    "calls" => EdgeKind::Calls,
                    "uses_type" => EdgeKind::UsesType,
                    "implements" => EdgeKind::Implements,
                    "imports" => EdgeKind::Imports,
                    "references" => EdgeKind::References,
                    _ => return None,
                };
                Some(Edge {
                    from: r.from,
                    to: r.to,
                    kind,
                    source: EdgeSource::Lsp,
                })
            })
            .collect())
    }
}
