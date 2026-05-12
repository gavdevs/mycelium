# Phase 3 — Workstream A: Cross-File Edge Resolution Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cross-file CALLS / USES_TYPE / IMPLEMENTS edges land correctly in the graph so `mycel callers <fn>`, `mycel uses <type>`, and `mycel implements <iface>` return non-empty results for cross-file references — and so Workstream C's Tier-4 graph-expand step has real edges to walk.

**Architecture:** The bridge today does the wrong thing in `scripts/multilspy_bridge.py:70-113` — it walks each documentSymbol, calls `request_definition` on the symbol's *own* name, and emits a `REFERENCES` edge whose `to` is a file URI (not a Symbol qname). `upsert_edge_batch` silently drops those edges because the MATCH on `qualified_name = "<file_uri>"` returns nothing.

The fix is symmetric: tree-sitter records call-site coordinates for each tentative CALLS / USES_TYPE / IMPLEMENTS edge; the pipeline forwards those sites to a new bridge op `resolve_refs_for_file` which calls `request_definition` on each site and returns `(target_uri, target_line, ref_kind)` tuples; the pipeline maps each tuple to a Symbol qname via a new graph query `symbol_containing(file_path, line)`; the pipeline emits an `Edge` with the resolved qname and `EdgeSource::Lsp`. Tree-sitter's old unresolved edges continue to drop silently (current behavior) — LSP edges become the source of truth for cross-file.

Layering note: the qname mapping lives in `mycel-index` (the pipeline), not `mycel-lsp`, to keep `mycel-lsp` free of a `mycel-graph` dependency. The spec's "Rust side translates" intent is preserved; only the crate boundary moves one layer down.

**Tech Stack:** Rust 2024 (mycel-extract / mycel-lsp / mycel-graph / mycel-index), Python 3 (multilspy bridge), tree-sitter, multilspy (`request_definition`), FalkorDB Cypher.

**Worktree:** Use `superpowers:using-git-worktrees` to create `.worktrees/phase-3-workstream-a` on branch `phase-3-workstream-a`.

**Prerequisites:** FalkorDB on `redis://127.0.0.1:16379` for integration tests (start with `docker run -d --rm --name mycel-falkordb-test -p 127.0.0.1:16379:6379 falkordb/falkordb:v4.18.3`). Python multilspy installed for live bridge tests (gated on `MYCEL_TEST_LSP=1`).

---

## File structure (decomposition decisions locked here)

| File | Responsibility | Action |
|---|---|---|
| `crates/mycel-core/src/edge.rs` | Edge value type | **Modify** — add optional `from_line: Option<u32>` |
| `crates/mycel-extract/src/languages/typescript.rs` | TS tree-sitter extraction | **Modify** — populate `from_line` on CALLS / USES_TYPE / IMPLEMENTS emissions |
| `crates/mycel-extract/src/languages/rust.rs` | Rust tree-sitter extraction | **Modify** — same as TS, for Rust |
| `crates/mycel-lsp/src/protocol.rs` | JSON wire types for the bridge | **Modify** — add `ResolveRefsReq`, `ResolveRefsResp`, `RawResolvedRef` |
| `crates/mycel-lsp/src/multilspy.rs` | Rust-side bridge client | **Modify** — add `resolve_refs(path, sites)` method; existing `refine` stays as-is for now (no callers after Chunk 5 lands) |
| `scripts/multilspy_bridge.py` | Python bridge subprocess | **Modify** — add `resolve_refs_for_file` op; deprecate `edges_for_file` (still answer it but log a WARN that it's legacy) |
| `crates/mycel-graph/src/queries.rs` | Graph query methods | **Modify** — add `symbol_containing(file_path, line) -> Option<String>` |
| `crates/mycel-index/src/pipeline.rs` | Per-file orchestration | **Modify** — collect tree-sitter sites whose `to` didn't resolve same-file, call `resolve_refs`, map URIs to qnames via graph, append resolved edges |
| `crates/mycel-graph/tests/integration.rs` | Graph integration tests | **Modify** — add `symbol_containing_returns_qname_for_enclosing_symbol` test |
| `crates/mycel-index/tests/cross_file_calls.rs` | New end-to-end test | **Create** — fixture with cross-file CALLS, assert it lands in the graph |
| `crates/mycel-extract/tests/fixtures/typescript/cross_file_caller.ts` | New TS fixture | **Create** — calls a fn from `simple_function.ts` |

---

## Chunk 1: Edge schema + tree-sitter site coords

### Task 1: Add `from_line` to `Edge`

**Files:**
- Modify: `crates/mycel-core/src/edge.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Read the current `Edge` definition**

Open `crates/mycel-core/src/edge.rs` and confirm the struct layout (`from`, `to`, `kind`, `source`).

- [ ] **Step 2: Write a failing test for the new field**

Append to a `#[cfg(test)] mod tests` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_default_has_no_from_line() {
        let e = Edge {
            from: "a".into(),
            to: "b".into(),
            kind: EdgeKind::Calls,
            source: EdgeSource::TreeSitter,
            from_line: None,
        };
        assert!(e.from_line.is_none());
    }

    #[test]
    fn edge_with_from_line_carries_coord() {
        let e = Edge {
            from: "a".into(),
            to: "b".into(),
            kind: EdgeKind::Calls,
            source: EdgeSource::TreeSitter,
            from_line: Some(42),
        };
        assert_eq!(e.from_line, Some(42));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p mycel-core edge`
Expected: build fails (struct has no `from_line` field).

- [ ] **Step 4: Add the field to `Edge`**

Add `pub from_line: Option<u32>` to the `Edge` struct. Default to `None` everywhere it's constructed outside the tests we'll write next. Use 1-indexed line numbers (tree-sitter uses 0-indexed internally; convert at extraction time, NOT here).

- [ ] **Step 5: Fix all compile errors workspace-wide**

Run: `cargo build --workspace 2>&1 | grep "missing field"`. Add `from_line: None` to every `Edge { ... }` construction site that the compiler flags. Expected sites (verify via grep): `crates/mycel-extract/src/languages/typescript.rs`, `crates/mycel-extract/src/languages/rust.rs`, `crates/mycel-lsp/src/multilspy.rs`, plus any test code constructing edges directly.

- [ ] **Step 6: Run the workspace build + clippy**

Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Run tests**

Run: `cargo test --workspace --lib`
Expected: all previous tests still pass, plus the 2 new edge tests.

- [ ] **Step 8: Commit**

```bash
git add crates/mycel-core/src/edge.rs crates/mycel-extract crates/mycel-lsp
git commit -m "Add optional from_line to Edge for call-site coordinates"
```

---

### Task 2: Tree-sitter populates `from_line` on cross-file-eligible edges

**Files:**
- Modify: `crates/mycel-extract/src/languages/typescript.rs` (around the CALLS / USES_TYPE / IMPLEMENTS emissions)
- Modify: `crates/mycel-extract/src/languages/rust.rs` (same)
- Test: `crates/mycel-extract/tests/typescript_fixtures.rs` and/or `crates/mycel-extract/tests/rust_fixtures.rs`

Rationale: tree-sitter emits the bare name (`to: "add"`) today; we add the line of the call site so the pipeline can ask LSP "what does this resolve to" later.

- [ ] **Step 1: Identify the exact emission sites in `typescript.rs`**

Per the explore: CALL emissions land around `crates/mycel-extract/src/languages/typescript.rs:203-208`. USES_TYPE and IMPLEMENTS emissions are nearby. Read the surrounding 20 lines to confirm the node variable that holds the call-site syntax node (will have a `start_position()` method from tree-sitter).

- [ ] **Step 2: Write a failing test**

Append to `crates/mycel-extract/tests/typescript_fixtures.rs`:

```rust
#[test]
fn typescript_call_edge_carries_from_line() {
    // Fixture imports_and_exports.ts contains a top-level call to `add`.
    let out = extract_fixture("typescript", "imports_and_exports");
    let call_edges: Vec<_> = out
        .edges
        .iter()
        .filter(|e| matches!(e.kind, mycel_core::EdgeKind::Calls))
        .collect();
    assert!(!call_edges.is_empty(), "fixture should produce >=1 CALL edge");
    for e in &call_edges {
        assert!(
            e.from_line.is_some(),
            "CALL edge {e:?} missing from_line — site coord must be captured"
        );
        assert!(e.from_line.unwrap() > 0, "from_line should be 1-indexed");
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p mycel-extract typescript_call_edge_carries_from_line`
Expected: assertion failure on `e.from_line.is_some()`.

- [ ] **Step 4: Modify the TypeScript extractor**

At each `Edge { ... kind: EdgeKind::Calls ... }` emission in `typescript.rs`, capture the call-site node's start row and convert to 1-indexed:

```rust
from_line: Some(call_site_node.start_position().row as u32 + 1),
```

Repeat for `EdgeKind::UsesType` and `EdgeKind::Implements` emission sites in the same file.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p mycel-extract typescript_call_edge_carries_from_line`
Expected: pass.

- [ ] **Step 6: Repeat for Rust extractor**

In `crates/mycel-extract/src/languages/rust.rs`, do the same: find every emission of `EdgeKind::Calls | UsesType | Implements` and capture the call-site row + 1. Mirror the test in `crates/mycel-extract/tests/rust_fixtures.rs` if a suitable Rust fixture exists; if not, skip (TS test is enough to lock the contract).

- [ ] **Step 7: Snapshot tests may drift — review and accept**

Run: `cargo test -p mycel-extract` — extractor snapshot tests (in `tests/typescript_fixtures.rs` / `rust_fixtures.rs`) may now include `from_line` in their serialized output. Review the snapshot diff and accept if it just reflects the new field. If `INSTA_UPDATE=auto cargo test -p mycel-extract` is the project pattern (check `Cargo.toml` for `insta` config), use that.

- [ ] **Step 8: Workspace gate**

Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --lib`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add crates/mycel-extract
git commit -m "Capture call-site row in tree-sitter CALLS/USES_TYPE/IMPLEMENTS edges"
```

---

## Chunk 2: Graph helper for location → qname

### Task 3: Add `symbol_containing(file_path, line)` to GraphClient

**Files:**
- Modify: `crates/mycel-graph/src/queries.rs`
- Test: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write a failing integration test**

Append to `crates/mycel-graph/tests/integration.rs` (study the existing `edge_upsert_callers_query()` test for the setup pattern):

```rust
#[tokio::test]
async fn symbol_containing_finds_enclosing_symbol() {
    let client = test_client("symbol_containing_finds_enclosing_symbol").await;
    // Upsert a Symbol that spans lines 10..=20.
    let sym = mycel_core::Symbol {
        qualified_name: "src/foo.rs::bar".into(),
        name: "bar".into(),
        kind: mycel_core::SymbolKind::Function,
        file_path: "src/foo.rs".into(),
        start_line: 10,
        end_line: 20,
        signature: "fn bar()".into(),
        exported: true,
        ..Default::default()
    };
    client.upsert_symbol_batch(&[sym]).await.unwrap();

    // A line inside the span -> Some(qname)
    let hit = client.symbol_containing("src/foo.rs", 15).await.unwrap();
    assert_eq!(hit.as_deref(), Some("src/foo.rs::bar"));

    // Boundary lines (inclusive both ends per the spec query)
    let lo = client.symbol_containing("src/foo.rs", 10).await.unwrap();
    let hi = client.symbol_containing("src/foo.rs", 20).await.unwrap();
    assert_eq!(lo.as_deref(), Some("src/foo.rs::bar"));
    assert_eq!(hi.as_deref(), Some("src/foo.rs::bar"));

    // A line outside the span -> None
    let miss = client.symbol_containing("src/foo.rs", 5).await.unwrap();
    assert!(miss.is_none(), "line 5 is outside 10..=20");

    // A line in a different file -> None
    let other = client.symbol_containing("src/other.rs", 15).await.unwrap();
    assert!(other.is_none());
}
```

The test assumes `Symbol::default()` exists with sensible defaults (no embedding, no description); if it doesn't, populate the remaining fields explicitly. Check the existing `edge_upsert_callers_query` test for the construction pattern.

- [ ] **Step 2: Verify FalkorDB is running for the test**

Run: `docker ps | grep mycel-falkordb-test`
If not running: `docker run -d --rm --name mycel-falkordb-test -p 127.0.0.1:16379:6379 falkordb/falkordb:v4.18.3`

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p mycel-graph --test integration symbol_containing_finds_enclosing_symbol`
Expected: compile failure — `symbol_containing` doesn't exist.

- [ ] **Step 4: Implement the method**

Add to `crates/mycel-graph/src/queries.rs`:

```rust
impl GraphClient {
    /// Returns the qname of the Symbol whose start_line..=end_line range contains
    /// `line` in `file_path`. Used by Workstream A's pipeline to map LSP-returned
    /// definition locations back to Symbol qnames.
    ///
    /// Returns `Ok(None)` when no Symbol spans that location — common case for
    /// definitions outside the indexed surface (stdlib, node_modules) or for
    /// references into module-decl symbols whose range we don't model.
    pub async fn symbol_containing(
        &self,
        file_path: &str,
        line: u32,
    ) -> Result<Option<String>> {
        let cypher = format!(
            "MATCH (s:Symbol) WHERE s.file_path = '{p}' \
               AND s.start_line <= {l} AND s.end_line >= {l} \
             RETURN s.qualified_name LIMIT 1",
            p = escape(file_path),
            l = line,
        );
        let rows = self.query(&cypher).await?;
        Ok(rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .and_then(|v| match v {
                falkordb::FalkorValue::String(s) => Some(s),
                _ => None,
            }))
    }
}
```

`escape` is the same helper used by the other queries in this file. `LIMIT 1` because nested symbols (method-in-class) could match both; we want the closest, but for now any match is correct — refine if/when it bites.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p mycel-graph --test integration symbol_containing_finds_enclosing_symbol`
Expected: 4 assertions pass.

- [ ] **Step 6: Run the broader graph test suite**

Run: `cargo test -p mycel-graph`
Expected: all existing tests still pass.

- [ ] **Step 7: Commit**

```bash
git add crates/mycel-graph
git commit -m "Add GraphClient::symbol_containing for file+line lookups"
```

---

## Chunk 3: Bridge — `resolve_refs_for_file` op

### Task 4: Add the new op to `scripts/multilspy_bridge.py`

**Files:**
- Modify: `scripts/multilspy_bridge.py`
- Modify: `crates/mycel-lsp/src/protocol.rs` (add request/response types)

- [ ] **Step 1: Add wire types in `protocol.rs`**

Append to `crates/mycel-lsp/src/protocol.rs`:

```rust
#[derive(serde::Serialize)]
pub struct ResolveRefsReq<'a> {
    pub id: u64,
    pub op: &'static str, // "resolve_refs_for_file"
    pub repo_root: &'a str,
    pub language: &'a str,
    pub path: &'a str,
    pub sites: Vec<RefSite>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct RefSite {
    pub line: u32, // 1-indexed
    pub col: u32,  // 0-indexed (LSP convention)
    pub kind: String, // "calls" | "uses_type" | "implements"
}

#[derive(serde::Deserialize, Debug)]
pub struct ResolveRefsResp {
    pub id: u64,
    pub refs: Vec<RawResolvedRef>,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
pub struct RawResolvedRef {
    pub from_path: String,
    pub from_line: u32,
    pub to_path: String,  // file path (already converted from URI by the bridge)
    pub to_line: u32,     // start line of the definition (1-indexed)
    pub kind: String,     // mirrors RefSite.kind
}
```

Bridge converts URIs to repo-relative paths so the Rust side can pass them straight to `symbol_containing`. Keep the conversion in Python (it has `urllib.parse` ready); don't push it to Rust.

- [ ] **Step 2: Run cargo build to verify the types compile**

Run: `cargo build -p mycel-lsp`
Expected: clean.

- [ ] **Step 3: Add the new op to the Python bridge**

In `scripts/multilspy_bridge.py`, add a method to `BridgeState`:

```python
def resolve_refs_for_file(self, repo_root: str, language: str, path: str, sites):
    """Per-site request_definition; map each result back to (path, line)."""
    server = self.get_server(repo_root, language)
    refs = []
    partial = False
    for site in (sites or []):
        try:
            # LSP positions are 0-indexed lines; tree-sitter sites are 1-indexed.
            defs = server.request_definition(path, site["line"] - 1, site["col"])
            for d in (defs or []):
                target_uri = d.get("uri") or d.get("targetUri")
                target_range = d.get("range") or d.get("targetSelectionRange") or d.get("targetRange")
                if not target_uri or not target_range:
                    continue
                # Convert file:// URI to a repo-relative path.
                to_path = _uri_to_repo_path(target_uri, repo_root)
                if to_path is None:
                    continue  # definition is outside the repo (stdlib, node_modules)
                to_line = target_range["start"]["line"] + 1  # back to 1-indexed
                refs.append({
                    "from_path": path,
                    "from_line": site["line"],
                    "to_path": to_path,
                    "to_line": to_line,
                    "kind": site["kind"],
                })
        except Exception:
            partial = True
    return {"refs": refs, "partial": partial}
```

Add a free helper at module scope:

```python
def _uri_to_repo_path(uri: str, repo_root: str):
    """Convert a file:// URI to a repo-relative path, or None if outside the repo."""
    from urllib.parse import urlparse, unquote
    p = urlparse(uri)
    if p.scheme != "file":
        return None
    abs_path = unquote(p.path)
    if not abs_path.startswith(repo_root.rstrip("/") + "/"):
        return None
    return abs_path[len(repo_root.rstrip("/")) + 1:]
```

Wire it into the dispatch loop in `main()`:

```python
elif op == "resolve_refs_for_file":
    result = state.resolve_refs_for_file(
        req["repo_root"], req["language"], req["path"], req.get("sites", []),
    )
    emit({"id": rid, **result})
```

Place it next to the existing `if op == "edges_for_file":` branch.

- [ ] **Step 4: Add a tiny Python self-test (run, don't unit-test)**

The bridge has no Python unit tests today; we won't add a framework. Instead, manually verify the op responds to a malformed request without crashing:

```bash
echo '{"id": 1, "op": "resolve_refs_for_file", "repo_root": "/tmp", "language": "typescript", "path": "nonexistent.ts", "sites": []}' | python3 scripts/multilspy_bridge.py
```

Expected: one JSON-line response with `{"id":1, "refs":[], "partial": ...}`. (May error if multilspy can't spawn a tsserver in `/tmp`; that's fine — `partial: true` is the contract.)

- [ ] **Step 5: Commit**

```bash
git add scripts/multilspy_bridge.py crates/mycel-lsp/src/protocol.rs
git commit -m "Add resolve_refs_for_file op to multilspy bridge + Rust wire types"
```

---

### Task 5: Rust-side `MultilspyResolver::resolve_refs`

**Files:**
- Modify: `crates/mycel-lsp/src/multilspy.rs`
- Test: `crates/mycel-lsp/tests/smoke.rs`

- [ ] **Step 1: Add the `resolve_refs` method**

Add to `impl MultilspyResolver` in `crates/mycel-lsp/src/multilspy.rs`. Pattern matches the existing `refine` method (read `refine` first to copy the request/response loop):

```rust
impl MultilspyResolver {
    /// For each (line, col, kind) site, calls multilspy's `request_definition`
    /// and returns the resolved (target_path, target_line, kind) tuples.
    /// `path` is repo-relative.
    pub async fn resolve_refs(
        &self,
        path: &camino::Utf8Path,
        language: &str,
        sites: Vec<crate::protocol::RefSite>,
    ) -> Result<Vec<crate::protocol::RawResolvedRef>> {
        if self.dead.load(std::sync::atomic::Ordering::Acquire) {
            return Err(MycelError::Lsp("bridge died earlier; refusing further requests".into()));
        }
        if sites.is_empty() {
            return Ok(Vec::new());
        }
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Use a one-shot channel keyed off the same `pending` map as `refine`.
        // Since the reader loop's typed channel is EdgesForFileResp-shaped, we
        // need a parallel path. Simplest: parse the bridge's JSON line ourselves
        // by adding a small `pending_resolve` map alongside `pending`.
        // See implementation note below.
        todo!("see implementation note in plan");
    }
}
```

**Implementation note:** the existing `pending` map sends `EdgesForFileResp` over a `oneshot::Sender`. We have two options:

A) Add a parallel `pending_resolve: Arc<Mutex<HashMap<u64, oneshot::Sender<ResolveRefsResp>>>>` and a second reader path that tries each response shape.

B) Make the bridge's responses self-describing (include `op` field) and demux in the reader loop.

Option B is cleaner long-term but bigger. Option A is what fits this plan's TDD shape best — one new field on the struct, one new map check in the reader, one new oneshot. **Use Option A.**

Refactor sketch (apply this BEFORE writing the `resolve_refs` body):

1. In `MultilspyResolver` struct, add a sibling map:
   ```rust
   pending_resolve: Arc<Mutex<HashMap<u64, oneshot::Sender<ResolveRefsResp>>>>,
   ```
2. In `reader_loop`, after the `EdgesForFileResp` parse attempt fails, try parsing as `ResolveRefsResp` and dispatch to `pending_resolve`.
3. `wait_loop` clears both maps on bridge death.

Then fill in `resolve_refs`:

```rust
let (tx, rx) = oneshot::channel();
self.pending_resolve.lock().await.insert(id, tx);
let req = crate::protocol::ResolveRefsReq {
    id,
    op: "resolve_refs_for_file",
    repo_root: self.repo_root.as_str(),
    language,
    path: path.as_str(),
    sites,
};
let line = serde_json::to_string(&req)? + "\n";
{
    let mut stdin = self.stdin.lock().await;
    stdin.write_all(line.as_bytes()).await
        .map_err(|e| MycelError::Lsp(format!("write: {e}")))?;
    stdin.flush().await
        .map_err(|e| MycelError::Lsp(format!("flush: {e}")))?;
}
let resp = rx.await.map_err(|_| MycelError::Lsp("bridge closed".into()))?;
if let Some(err) = resp.error {
    tracing::warn!(language, %path, "resolve_refs error: {err}");
    return Ok(Vec::new());
}
if resp.partial {
    tracing::warn!(language, %path, "resolve_refs returned partial results");
}
Ok(resp.refs)
```

- [ ] **Step 2: Write a smoke test (gated on `MYCEL_TEST_LSP=1`)**

Append to `crates/mycel-lsp/tests/smoke.rs`:

```rust
#[tokio::test]
async fn lsp_resolve_refs_typescript() {
    if std::env::var("MYCEL_TEST_LSP").ok().as_deref() != Some("1") {
        eprintln!("skipping (set MYCEL_TEST_LSP=1; requires multilspy + tsserver)");
        return;
    }
    // The fixture `crates/mycel-extract/tests/fixtures/typescript/cross_file_caller.ts`
    // (created in Task 7) imports `add` from `simple_function.ts` and calls it.
    // Run resolve_refs against the call site and expect a hit pointing back to
    // simple_function.ts.
    use mycel_lsp::protocol::RefSite;
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().join("mycel-extract/tests/fixtures/typescript");
    let repo_root = camino::Utf8PathBuf::try_from(repo_root).unwrap();
    let resolver = mycel_lsp::MultilspyResolver::spawn(
        "python3 ../../../scripts/multilspy_bridge.py",
        repo_root.clone(),
    ).await.unwrap();

    let sites = vec![RefSite {
        line: 3, // adjust to the actual call line in the fixture
        col: 0,
        kind: "calls".into(),
    }];
    let refs = resolver
        .resolve_refs(camino::Utf8Path::new("cross_file_caller.ts"), "typescript", sites)
        .await
        .expect("resolve_refs should succeed");
    assert!(!refs.is_empty(), "expected at least one resolved ref");
    let first = &refs[0];
    assert!(first.to_path.contains("simple_function"), "got {:?}", first);
    assert!(first.to_line > 0);
}
```

The fixture line number (`line: 3`) will need to match the actual call site in Task 7's fixture; revise then.

- [ ] **Step 3: Run the smoke test (skipped without env var)**

Run: `cargo test -p mycel-lsp --test smoke lsp_resolve_refs_typescript`
Expected: skip message printed; test returns OK.

- [ ] **Step 4: Workspace gate**

Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --lib`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-lsp
git commit -m "Add MultilspyResolver::resolve_refs + parallel response demux"
```

---

## Chunk 4: Pipeline integration

### Task 6: Pipeline collects sites, resolves, maps to qnames

**Files:**
- Modify: `crates/mycel-index/src/pipeline.rs`

- [ ] **Step 1: Read the current pipeline flow**

Open `crates/mycel-index/src/pipeline.rs`. Find `index_file_collect_edges` (lines 24-64 per the explore). Identify where `lsp.refine(...)` is called (around line 55-63) and where tree-sitter edges land (`all_edges` from `resolve_same_file_edges`).

- [ ] **Step 2: Replace `refine` with `resolve_refs` site collection + call**

Restructure the LSP path roughly as follows. The exact merge with existing code requires reading the function — this is the shape, not the verbatim diff:

```rust
// After resolve_same_file_edges runs, collect cross-file-eligible sites:
let sites: Vec<crate::protocol::RefSite> = all_edges
    .iter()
    .filter_map(|e| {
        let kind = match e.kind {
            EdgeKind::Calls => "calls",
            EdgeKind::UsesType => "uses_type",
            EdgeKind::Implements => "implements",
            _ => return None,
        };
        // Only sites with a captured from_line are eligible; only those whose
        // `to` is a bare name (didn't resolve same-file) need LSP help — but
        // for v1 we send them all and let LSP win. Refine if perf bites.
        e.from_line.map(|line| crate::protocol::RefSite {
            line,
            col: 0, // LSP will hover the line; column-accuracy is a v2 refinement
            kind: kind.into(),
        })
    })
    .collect();

if let Some(lsp) = lsp.as_ref() {
    if !sites.is_empty() {
        match lsp.resolve_refs(path, language, sites).await {
            Ok(refs) => {
                for r in refs {
                    let to_qname = match graph.symbol_containing(&r.to_path, r.to_line).await {
                        Ok(Some(q)) => q,
                        _ => continue, // dropped: definition outside indexed surface
                    };
                    let kind = match r.kind.as_str() {
                        "calls" => EdgeKind::Calls,
                        "uses_type" => EdgeKind::UsesType,
                        "implements" => EdgeKind::Implements,
                        _ => continue,
                    };
                    all_edges.push(Edge {
                        from: format!("{}::{}", r.from_path, "<unknown>"), // see note
                        to: to_qname,
                        kind,
                        source: EdgeSource::Lsp,
                        from_line: Some(r.from_line),
                    });
                }
            }
            Err(e) => tracing::warn!(error = %e, "lsp resolve_refs failed; using tree-sitter edges only"),
        }
    }
}
```

**Note on `from` qname:** the bridge returns `from_path` and `from_line`; to reconstruct the from-qname, we'd need the same `symbol_containing(from_path, from_line)` lookup. **Two ways to get from_qname**, pick one:

A) Recompute via `graph.symbol_containing(from_path, from_line)`. Costs 1 extra graph query per site.

B) Carry `from_qname` through tree-sitter's original edge — the pipeline already knows it (it's `e.from` in the original tree-sitter edge before resolution). Build a map `from_line -> from_qname` from `all_edges` before the LSP call, then look up.

**Use B.** Build a `HashMap<u32, String>` keyed by `from_line` from the tree-sitter edges; use it to fill `from` on each resolved ref. If a line maps to multiple from_qnames (shouldn't happen for single-statement lines but possible with nested calls), accept the first — the trade-off is negligible.

This is the pipeline's core change. Take it slowly; the reviewer will read it carefully.

- [ ] **Step 3: Verify build**

Run: `cargo build -p mycel-index`
Expected: clean. If there are borrow-checker issues with `lsp` being borrowed across awaits while `all_edges` is mutated, restructure to collect resolved edges into a separate `Vec` first and extend `all_edges` after the loop.

- [ ] **Step 4: Add graph access to the pipeline if missing**

If the pipeline doesn't already take a `&GraphClient`, thread it through `index_file_collect_edges`'s signature. The caller (`index_repo` or similar) already has it.

- [ ] **Step 5: Update the `refine`-vs-`resolve_refs` story**

Workstream A removes the callers of `MultilspyResolver::refine`. Keep the method (don't break the API) but stop calling it from the pipeline. Add a `#[deprecated(note = "...")]` attribute pointing at `resolve_refs` so future work has a clean path to delete it.

- [ ] **Step 6: Run the existing index-side tests**

Run: `cargo test -p mycel-index`
Expected: passes (some tests may need updates if they assert on edge counts that grew; review failures one by one).

- [ ] **Step 7: Commit**

```bash
git add crates/mycel-index
git commit -m "Pipeline: resolve cross-file edges via multilspy + graph location lookup"
```

---

## Chunk 5: End-to-end cross-file edge test

### Task 7: Cross-file fixture + integration test

**Files:**
- Create: `crates/mycel-extract/tests/fixtures/typescript/cross_file_caller.ts`
- Create: `crates/mycel-index/tests/cross_file_calls.rs`

- [ ] **Step 1: Create the fixture**

Write `crates/mycel-extract/tests/fixtures/typescript/cross_file_caller.ts`:

```typescript
import { add } from './simple_function';

export function useAdd(): number {
  return add(1, 2);
}
```

The existing fixture `simple_function.ts` already defines `export function add(a, b)`. Confirm with `cat crates/mycel-extract/tests/fixtures/typescript/simple_function.ts`.

- [ ] **Step 2: Write the failing integration test**

Create `crates/mycel-index/tests/cross_file_calls.rs`:

```rust
//! End-to-end: index a tiny TS project with a cross-file call and assert
//! the CALLS edge lands in the graph with both endpoints resolved to Symbol
//! qnames.

use mycel_graph::GraphClient;

#[tokio::test]
async fn cross_file_calls_land_in_graph() {
    if std::env::var("MYCEL_TEST_LSP").ok().as_deref() != Some("1") {
        eprintln!("skipping (set MYCEL_TEST_LSP=1; requires multilspy + tsserver + FalkorDB)");
        return;
    }

    let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("mycel-extract/tests/fixtures/typescript");
    let fixtures = camino::Utf8PathBuf::try_from(fixtures).unwrap();

    let graph_name = "cross_file_calls_land_in_graph";
    let client = GraphClient::connect("redis://127.0.0.1:16379", graph_name).await.unwrap();
    client.reset_graph().await.unwrap();

    // Use the pipeline's repo-indexer for an honest end-to-end. Adjust import
    // path to whatever `mycel_index::index_repo` is actually named.
    let cfg = mycel_index::IndexConfig {
        repo_root: fixtures.clone(),
        // ... include just enough config to spin up multilspy + tree-sitter for TS;
        // copy from an existing index test fixture if one exists
        ..Default::default()
    };
    mycel_index::index_repo(&client, &cfg).await.unwrap();

    // After indexing, the call to `add` in cross_file_caller.ts should be a CALLS
    // edge from `cross_file_caller.ts::useAdd` to `simple_function.ts::add`.
    let callers = client
        .query_callers("simple_function.ts::add")
        .await
        .unwrap();
    let names: Vec<_> = callers.iter().map(|s| s.qualified_name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.ends_with("::useAdd")),
        "expected cross_file_caller.ts::useAdd in callers of add; got {:?}",
        names
    );
}
```

The signature of `mycel_index::index_repo` and `mycel_index::IndexConfig` may differ from this sketch — adjust to match the actual API. The point is: index, then `query_callers("simple_function.ts::add")` returns the cross-file caller.

- [ ] **Step 3: Run the test to confirm it fails on a fresh worktree pre-Workstream-A**

Skip this if there's no clean pre-Workstream-A state to compare against. The test exists to lock the post-Workstream-A behavior; we accept "passes now" as success.

- [ ] **Step 4: Run the test with `MYCEL_TEST_LSP=1` and FalkorDB running**

```bash
docker ps | grep mycel-falkordb-test || \
  docker run -d --rm --name mycel-falkordb-test -p 127.0.0.1:16379:6379 falkordb/falkordb:v4.18.3
MYCEL_TEST_LSP=1 cargo test -p mycel-index --test cross_file_calls
```

Expected: test passes. If multilspy/tsserver setup is incomplete in the test environment, the test will skip — that's acceptable for the v1 ship, but verify manually that the test *would* pass when LSP is set up.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-extract/tests/fixtures crates/mycel-index/tests/cross_file_calls.rs
git commit -m "Add end-to-end test: cross-file CALLS edge lands in the graph"
```

---

## Chunk 6: Workspace verification + CLAUDE.md update

### Task 8: Verify the whole workspace + drop the "callers broken" note from CLAUDE.md

- [ ] **Step 1: Full workspace test (FalkorDB up)**

Ensure `mycel-falkordb-test` is running.

```bash
cargo build --workspace && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo test --workspace
```

Expected: zero failures, zero warnings.

- [ ] **Step 2: Manually verify a Tier-1 command works**

```bash
target/debug/mycel --repo . index .
target/debug/mycel --repo . callers content_hash
```

Expected: returns ≥1 result (per CLAUDE.md, today this returns empty; Workstream A should fix it).

- [ ] **Step 3: Update CLAUDE.md**

Edit `CLAUDE.md`:
- Remove `callers`, `uses`, `implements` from the "Broken" table rows.
- Add them as "Works" rows.
- Update the "**`callers`, `uses`, `implements` all return empty.**" bullet in the Phase 1/2 limitations section — either delete it (preferred) or qualify it ("Pre-Workstream-A: returned empty. Post-Workstream-A (this branch): cross-file edges land via LSP definition lookups").
- Delete the "**Cross-file CALLS edges drop silently.**" bullet too (Workstream A makes them land).

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "Mark callers/uses/implements as working post-Workstream-A"
```

---

## Success criteria

1. `cargo build --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test --workspace` clean (with `mycel-falkordb-test` running and `MYCEL_TEST_LSP=1` for LSP-gated tests).
4. `mycel callers content_hash` against this repo returns ≥3 callers (the success criterion from the design spec).
5. `mycel uses Symbol` returns ≥30 results.
6. `mycel implements Embedder` returns ≥1 (`OllamaEmbedder`).
7. The cross-file integration test (`cross_file_calls.rs`) passes.
8. CLAUDE.md no longer lists `callers` / `uses` / `implements` as broken.

## Out of scope (defer to follow-up work, do NOT bundle here)

- Cross-file CALLS edge resolution for languages without multilspy (Python, Go, etc.) — TS + Rust only here.
- Column-accurate call-site coords (currently we send `col: 0` and let LSP hover the line). v2 polish if eval-harness data shows accuracy wins.
- Deleting `MultilspyResolver::refine` — marked `#[deprecated]` here; delete when no callers remain after a sweep.
- Tier-3 / Tier-4 query side effects (e.g., does graph expansion now actually expand?) — that's Workstream C's domain.
- LSP-resolved edges from outside the indexed surface (stdlib, node_modules). Today they get dropped at `symbol_containing` returns None. Future work could choose to record them as "external" Symbols.
