use camino::Utf8PathBuf;
use notify::{EventKind, RecursiveMode, Watcher};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Spawns a watcher over `root` and returns an mpsc receiver that emits
/// changed file paths after a 2-second debounce. The sender side is owned
/// by the spawned task; dropping the receiver causes the task to exit.
pub fn spawn(root: Utf8PathBuf) -> mpsc::Receiver<Utf8PathBuf> {
    let (tx, rx) = mpsc::channel(256);
    tokio::spawn(async move {
        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = raw_tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                warn!(error=%e, "watcher init");
                return;
            }
        };
        if let Err(e) = watcher.watch(root.as_std_path(), RecursiveMode::Recursive) {
            warn!(error=%e, "watch root");
            return;
        }
        info!(root=%root, "watching");

        // 2s debounce: collect events into a HashMap keyed by path so we
        // emit each path at most once per tick even under rapid edits.
        let mut pending: std::collections::HashMap<Utf8PathBuf, ()> =
            std::collections::HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_secs(2));

        loop {
            tokio::select! {
                Some(ev) = raw_rx.recv() => {
                    if let Ok(ev) = ev {
                        if !matches!(ev.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                            continue;
                        }
                        for p in ev.paths {
                            if let Ok(u) = camino::Utf8PathBuf::from_path_buf(p) {
                                if !u.as_str().contains("/.git/") && !u.as_str().contains("/target/") && !u.as_str().contains("/node_modules/") {
                                    pending.insert(u, ());
                                }
                            }
                        }
                    }
                },
                _ = tick.tick() => {
                    for (p, _) in pending.drain() {
                        if tx.send(p).await.is_err() { return; }
                    }
                }
            }
        }
    });
    rx
}
