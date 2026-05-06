# Workload-Driven Synthesis Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drop eager-at-index synthesis from Mycelium. Cold index becomes Cursor-style (signature + body slice embeddings, no LLM). Behavioral descriptions are written workload-driven by Claude Code via a new `mycel set-description` CLI and a shipped `mycel-graph-care` skill. Existing Synthesizer/Ollama infrastructure is preserved for opt-in bulk passes.

**Architecture:** Two new fields on the Symbol node (`body_hash`, `description_source_hash`) drive staleness invalidation. The indexing pipeline embeds `signature + body[:60 lines]` and records `body_hash`. Description writes go through `set_symbol_description_and_embedding`, which now also stamps `description_source_hash = body_hash`. The daemon's incremental path detects mismatches and reverts stale descriptions to fresh signature+body embeddings.

**Tech Stack:** Rust workspace (9 crates), FalkorDB (Cypher), tree-sitter, Ollama (existing Embedder/Synthesizer), tokio, blake3 (already a dep).

**Spec:** `docs/superpowers/specs/2026-05-05-workload-driven-synthesis-design.md`

**Pre-flight (before any task):**

- [ ] Confirm FalkorDB test container is up: `docker ps | grep mycel-falkordb-test`. If not: `docker run -d --rm --name mycel-falkordb-test -p 127.0.0.1:16379:6379 -p 127.0.0.1:13000:3000 falkordb/falkordb:v4.18.3`.
- [ ] Confirm baseline build green: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- [ ] Read the spec at `docs/superpowers/specs/2026-05-05-workload-driven-synthesis-design.md` end-to-end before starting.

**File structure overview:**

| File | Status | Responsibility |
|------|--------|----------------|
| `crates/mycel-core/src/symbol.rs` | Modify | Add `body_hash`, `description_source_hash` fields to `Symbol` struct |
| `crates/mycel-graph/src/symbol.rs` | Modify | Extend `upsert_symbol` to write `body_hash`; modify `set_symbol_description_and_embedding` to stamp `description_source_hash`; add `get_symbol_description`, `clear_symbol_description_and_reembed`, `list_stale_descriptions`, `refresh_description_source_hashes` |
| `crates/mycel-graph/tests/integration.rs` | Modify | Add tests for new graph methods |
| `crates/mycel-index/src/body_slice.rs` | Create | Reusable `body_slice_and_hash(path, start, end) -> (text, hash)` helper, sharing `BODY_LINE_CAP = 60` with synthesize.rs |
| `crates/mycel-index/src/synthesize.rs` | Modify | Move `BODY_LINE_CAP` import from local to shared; reuse new body_slice helper |
| `crates/mycel-index/src/pipeline.rs` | Modify | Drop `synthesizer` field on `Indexer`; embed `signature + body slice`; write `body_hash`; remove final synth pass from `index_repo`; add staleness pre-pass that clears descriptions whose source hash diverges from the new body hash |
| `crates/mycel-index/src/lib.rs` | Modify | Export `body_slice` module; remove now-defunct re-exports |
| `crates/mycel-cli/src/cli.rs` | Modify | Add `Describe`, `SetDescription`, `Skill` subcommands; add `--refresh-hashes-only` to `Synthesize`, `--force-cold-rebuild` to `Index`; remove `--no-descriptions` |
| `crates/mycel-cli/src/main.rs` | Modify | Wire new subcommands; remove synthesizer construction in `Index` arm |
| `crates/mycel-cli/src/skill_install.rs` | Create | Idempotent symlink helper for `mycel skill install` |
| `crates/mycel-cli/tests/cli_integration.rs` | Create | End-to-end CLI tests against ephemeral FalkorDB |
| `crates/mycel-daemon/src/main.rs` | Modify | Drop `synthesizer` from the `Indexer` it constructs (matches new struct shape) |
| `skills/mycel-graph-care/SKILL.md` | Create | The shipped skill instructing Claude when/how to write descriptions |
| `CLAUDE.md` | Modify | Document new commands; update phase status notes |
| `DESIGN.md` | Already updated | Phase 1/2 sections and schema row updated in commit `10095b8` |

---

## Chunk 1: Schema + graph methods

Foundation. Adds the two new Symbol fields, extends existing Cypher to read/write them, and introduces the four new graph methods that downstream chunks consume. Nothing in this chunk changes the indexing pipeline — `mycel index` continues to behave exactly as it does today (ignoring the new fields, which default to NULL).

### Task 1.1: Add `body_hash` and `description_source_hash` to the `Symbol` struct

**Files:**
- Modify: `crates/mycel-core/src/symbol.rs:51-66`
- Test: `crates/mycel-core/src/symbol.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-core/src/symbol.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_serializes_with_new_fields() {
        let sym = Symbol {
            qualified_name: QualifiedName::new("a::b"),
            kind: SymbolKind::Function,
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 3,
            signature: Signature::new("fn b()"),
            jsdoc: None,
            synthesized_description: None,
            exported: true,
            embedding: None,
            body_hash: Some("deadbeef".into()),
            description_source_hash: Some("cafebabe".into()),
        };
        let json = serde_json::to_string(&sym).unwrap();
        assert!(json.contains("body_hash"));
        assert!(json.contains("description_source_hash"));
        let round: Symbol = serde_json::from_str(&json).unwrap();
        assert_eq!(round.body_hash.as_deref(), Some("deadbeef"));
        assert_eq!(round.description_source_hash.as_deref(), Some("cafebabe"));
    }

    #[test]
    fn symbol_deserializes_legacy_shape_without_new_fields() {
        // Symbols stored on disk before this revision lack the two fields;
        // they must round-trip through serde without error and read back as None.
        let legacy = r#"{
            "qualified_name": "a::b",
            "kind": "function",
            "file_path": "src/a.rs",
            "start_line": 1,
            "end_line": 3,
            "signature": "fn b()",
            "exported": true
        }"#;
        let s: Symbol = serde_json::from_str(legacy).unwrap();
        assert!(s.body_hash.is_none());
        assert!(s.description_source_hash.is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mycel-core symbol_serializes_with_new_fields`
Expected: FAIL — `Symbol` has no `body_hash` field.

- [ ] **Step 3: Add the two fields to `Symbol`**

In `crates/mycel-core/src/symbol.rs`, modify the `Symbol` struct at line 51:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub qualified_name: QualifiedName,
    pub kind: SymbolKind,
    pub file_path: Utf8PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: Signature,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsdoc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesized_description: Option<String>,
    pub exported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// blake3 hex of the `signature + body slice` text used for the current
    /// embedding. Set on every cold-index/incremental pass. NULL only on
    /// legacy graphs predating the 2026-05-05 redirection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_hash: Option<String>,
    /// The `body_hash` value at the time `synthesized_description` was last
    /// written. NULL until a description is written. Compared against
    /// `body_hash` on incremental updates to detect description staleness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_source_hash: Option<String>,
}
```

- [ ] **Step 4: Update every site that constructs a `Symbol` literal**

Use compile-driven discovery: `cargo build --workspace 2>&1 | grep "missing field"` will list every E0063. Fix each site by adding `body_hash: None, description_source_hash: None,` to the struct literal.

Expected sites (verified at planning time; trust the compiler over this list):
- `crates/mycel-extract/src/languages/rust.rs` — extractor builds Symbol literals
- `crates/mycel-extract/src/languages/typescript.rs` — same
- `crates/mycel-core/tests/serde.rs` — Symbol round-trip fixture
- `crates/mycel-graph/src/symbol.rs:127` — `parse_symbol_row` constructor
- `crates/mycel-graph/tests/integration.rs` — test fixtures (multiple)

- [ ] **Step 5: Run tests**

Run: `cargo test -p mycel-core --lib`
Expected: PASS — both new tests pass; existing tests unaffected.

Run: `cargo build --workspace`
Expected: clean.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-core/src/symbol.rs crates/mycel-extract/src crates/mycel-graph/src/symbol.rs crates/mycel-graph/tests/integration.rs
git commit -m "Add body_hash and description_source_hash to Symbol"
```

### Task 1.2: Extend `upsert_symbol` to persist `body_hash`

**Files:**
- Modify: `crates/mycel-graph/src/symbol.rs:146-169`
- Modify: `crates/mycel-graph/src/symbol.rs:127-139` (parse_symbol_row — extend to read body_hash if present, but the existing 7-column shape stays; new fields go through dedicated read methods).
- Test: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-graph/tests/integration.rs`:

```rust
#[tokio::test]
async fn upsert_symbol_writes_body_hash_when_set() {
    let client = GraphClient::connect(&url(), "mycel:test:body_hash").await.unwrap();
    let mut sym = Symbol {
        qualified_name: QualifiedName::new("crate::tests::with_hash"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn with_hash()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("abc123".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    // Read back via raw cypher — there is no public reader for body_hash yet
    let rows = client
        .query(
            "MATCH (s:Symbol {qualified_name: 'crate::tests::with_hash'}) \
             RETURN s.body_hash"
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let value = rows[0].first().unwrap();
    match value {
        falkordb::FalkorValue::String(s) => assert_eq!(s, "abc123"),
        other => panic!("expected String, got {other:?}"),
    }

    // Setting body_hash to None should NOT clobber the stored value on a
    // subsequent upsert (extractor doesn't compute hashes; pipeline does).
    sym.body_hash = None;
    client.upsert_symbol(&sym).await.unwrap();
    let rows = client
        .query(
            "MATCH (s:Symbol {qualified_name: 'crate::tests::with_hash'}) \
             RETURN s.body_hash"
        )
        .await
        .unwrap();
    match &rows[0][0] {
        falkordb::FalkorValue::String(s) => assert_eq!(s, "abc123",
            "None body_hash on upsert must not clobber existing value"),
        other => panic!("expected String, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mycel-graph --test integration upsert_symbol_writes_body_hash_when_set`
Expected: FAIL — `body_hash` property is never written.

- [ ] **Step 3: Modify `upsert_symbol` to write `body_hash` only when `Some`**

In `crates/mycel-graph/src/symbol.rs:146-169`, extend the Cypher to conditionally include the hash:

```rust
pub async fn upsert_symbol(&self, sym: &Symbol) -> Result<()> {
    let kind = serde_json::to_value(sym.kind).expect("SymbolKind serializes infallibly");
    let kind_str = kind.as_str().expect("SymbolKind serializes as JSON string");
    // body_hash is computed by the indexing pipeline, not by the extractor;
    // an upsert from extracted-Symbol-only paths leaves it None and we must
    // NOT clobber a hash a previous pipeline pass already wrote.
    let body_hash_clause = match &sym.body_hash {
        Some(h) => format!(", s.body_hash = '{}'", escape(h)),
        None => String::new(),
    };
    let cypher = format!(
        r#"MERGE (s:Symbol {{qualified_name: '{qname}'}})
        SET s.kind = '{kind}',
            s.file_path = '{file}',
            s.start_line = {start},
            s.end_line = {end},
            s.signature = '{sig}',
            s.exported = {exported},
            s.name = '{name}'{body_hash_clause}"#,
        qname = escape(sym.qualified_name.as_str()),
        kind = kind_str,
        file = escape(sym.file_path.as_str()),
        start = sym.start_line,
        end = sym.end_line,
        sig = escape(sym.signature.as_str()),
        exported = sym.exported,
        name = escape(short_name(sym.qualified_name.as_str())),
    );
    self.query(&cypher).await?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p mycel-graph --test integration upsert_symbol_writes_body_hash_when_set`
Expected: PASS.

Run: `cargo test -p mycel-graph --test integration` (full suite)
Expected: PASS — no regressions.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-graph/src/symbol.rs crates/mycel-graph/tests/integration.rs
git commit -m "Persist body_hash on Symbol upsert (preserve existing on None)"
```

### Task 1.3: `set_symbol_description_and_embedding` stamps `description_source_hash`

**Files:**
- Modify: `crates/mycel-graph/src/symbol.rs:312-332`
- Test: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-graph/tests/integration.rs`:

```rust
#[tokio::test]
async fn set_description_stamps_source_hash_from_body_hash() {
    let client = GraphClient::connect(&url(), "mycel:test:source_hash").await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::stamp"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn stamp()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("body-v1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    let dummy_vec = vec![0.1f32; 768];
    client.set_symbol_description_and_embedding(
        "crate::stamp", "Stamps a thing.", &dummy_vec,
    ).await.unwrap();

    let rows = client.query(
        "MATCH (s:Symbol {qualified_name: 'crate::stamp'}) \
         RETURN s.synthesized_description, s.description_source_hash"
    ).await.unwrap();
    let row = &rows[0];
    match &row[0] {
        falkordb::FalkorValue::String(s) => assert_eq!(s, "Stamps a thing."),
        other => panic!("desc: {other:?}"),
    }
    match &row[1] {
        falkordb::FalkorValue::String(s) => assert_eq!(s, "body-v1",
            "description_source_hash must mirror body_hash at write time"),
        other => panic!("source_hash: {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mycel-graph --test integration set_description_stamps_source_hash_from_body_hash`
Expected: FAIL — `description_source_hash` is NULL.

- [ ] **Step 3: Modify the Cypher**

Replace `set_symbol_description_and_embedding` in `crates/mycel-graph/src/symbol.rs:312-332` with:

```rust
pub async fn set_symbol_description_and_embedding(
    &self,
    qname: &str,
    description: &str,
    embedding: &[f32],
) -> Result<()> {
    let vec_lit = embedding
        .iter()
        .map(|f| f.to_string())
        .collect::<Vec<_>>()
        .join(",");
    // Atomic write: description, embedding, AND description_source_hash
    // (mirrored from the Symbol's current body_hash) all land in a single
    // statement so a stale hash can never be observed against a fresh
    // description. The `coalesce` defends against legacy Symbols whose
    // body_hash hasn't been backfilled yet — they get NULL source_hash,
    // which the daemon's staleness pass treats as "do not invalidate."
    let cypher = format!(
        "MATCH (s:Symbol {{qualified_name: '{q}'}}) \
         SET s.synthesized_description = '{d}', \
             s.embedding = vecf32([{v}]), \
             s.description_source_hash = s.body_hash",
        q = escape(qname),
        d = escape(description),
        v = vec_lit,
    );
    self.query(&cypher).await?;
    Ok(())
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p mycel-graph --test integration set_description_stamps_source_hash_from_body_hash`
Expected: PASS.

Run: `cargo test -p mycel-graph --test integration` (full suite)
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-graph/src/symbol.rs crates/mycel-graph/tests/integration.rs
git commit -m "Stamp description_source_hash from body_hash on description write"
```

### Task 1.4: Add `get_symbol_description` graph method

**Files:**
- Modify: `crates/mycel-graph/src/symbol.rs` (add method on `GraphClient`)
- Test: `crates/mycel-graph/tests/integration.rs`

The skill calls `mycel describe <qname>` to check current state before deciding to overwrite. The CLI command needs a way to read description + hashes in one call.

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-graph/tests/integration.rs`:

```rust
#[tokio::test]
async fn get_symbol_description_returns_description_and_hashes() {
    let client = GraphClient::connect(&url(), "mycel:test:get_desc").await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::getter"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn getter()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("body-x".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    // Before write: description None, hashes (body=Some, source=None).
    let info = client.get_symbol_description("crate::getter").await.unwrap();
    let info = info.expect("symbol exists");
    assert_eq!(info.description, None);
    assert_eq!(info.body_hash.as_deref(), Some("body-x"));
    assert_eq!(info.description_source_hash, None);

    client.set_symbol_description_and_embedding(
        "crate::getter", "Gets stuff.", &vec![0.0f32; 768],
    ).await.unwrap();

    // After write: description Some, source_hash mirrors body_hash.
    let info = client.get_symbol_description("crate::getter").await.unwrap();
    let info = info.expect("symbol exists");
    assert_eq!(info.description.as_deref(), Some("Gets stuff."));
    assert_eq!(info.body_hash.as_deref(), Some("body-x"));
    assert_eq!(info.description_source_hash.as_deref(), Some("body-x"));

    // Non-existent symbol returns None
    assert!(client.get_symbol_description("crate::nope").await.unwrap().is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mycel-graph --test integration get_symbol_description_returns_description_and_hashes`
Expected: FAIL — method does not exist.

- [ ] **Step 3: Add a struct + method**

In `crates/mycel-graph/src/symbol.rs`, near the other description-related types:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolDescriptionInfo {
    pub description: Option<String>,
    pub body_hash: Option<String>,
    pub description_source_hash: Option<String>,
}
```

Inside `impl GraphClient`, add the method:

```rust
/// Returns the Symbol's current description plus the two hashes used to
/// detect staleness. `None` only when the qname doesn't match a Symbol
/// node — distinguishes "missing" from "exists but no description."
pub async fn get_symbol_description(&self, qname: &str) -> Result<Option<SymbolDescriptionInfo>> {
    let cypher = format!(
        "MATCH (s:Symbol {{qualified_name: '{q}'}}) \
         RETURN coalesce(s.synthesized_description, '') AS desc, \
                coalesce(s.body_hash, '') AS bh, \
                coalesce(s.description_source_hash, '') AS sh",
        q = escape(qname),
    );
    let rows = self.query(&cypher).await?;
    let Some(row) = rows.into_iter().next() else { return Ok(None); };
    let mut iter = row.into_iter();
    let desc = match iter.next() {
        Some(FalkorValue::String(s)) if !s.is_empty() => Some(s),
        _ => None,
    };
    let body_hash = match iter.next() {
        Some(FalkorValue::String(s)) if !s.is_empty() => Some(s),
        _ => None,
    };
    let source_hash = match iter.next() {
        Some(FalkorValue::String(s)) if !s.is_empty() => Some(s),
        _ => None,
    };
    Ok(Some(SymbolDescriptionInfo {
        description: desc,
        body_hash,
        description_source_hash: source_hash,
    }))
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p mycel-graph --test integration get_symbol_description_returns_description_and_hashes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-graph/src/symbol.rs crates/mycel-graph/tests/integration.rs
git commit -m "Add get_symbol_description graph method"
```

### Task 1.4b: Add batched `description_source_hashes_for_batch` graph method

The daemon's incremental staleness pre-pass (Task 4.1) needs to know each Symbol's stored `description_source_hash` so it can compare against the freshly-computed `body_hash`. Doing one round-trip per Symbol kills cold-rebuild throughput (≥50k Cypher reads on a typical work codebase). This task adds a single-round-trip batched reader.

**Files:**
- Modify: `crates/mycel-graph/src/symbol.rs`
- Test: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-graph/tests/integration.rs`:

```rust
#[tokio::test]
async fn description_source_hashes_for_batch_returns_only_described_rows() {
    let client = GraphClient::connect(&url(), "mycel:test:hash_batch").await.unwrap();

    // Symbol with description: source_hash should appear
    let described = Symbol {
        qualified_name: QualifiedName::new("crate::described_batch"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn described_batch()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("hd".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&described).await.unwrap();
    client.set_symbol_description_and_embedding(
        "crate::described_batch", "Has desc.", &vec![0.0; 768],
    ).await.unwrap();

    // Symbol without description: should be absent from the map
    let undescribed = Symbol {
        qualified_name: QualifiedName::new("crate::undescribed_batch"),
        body_hash: Some("hu".into()),
        ..described.clone()
    };
    client.upsert_symbol(&undescribed).await.unwrap();

    let map = client
        .description_source_hashes_for_batch(&[
            "crate::described_batch",
            "crate::undescribed_batch",
            "crate::nonexistent",
        ])
        .await.unwrap();

    assert_eq!(map.get("crate::described_batch").map(|s| s.as_str()), Some("hd"));
    assert!(!map.contains_key("crate::undescribed_batch"));
    assert!(!map.contains_key("crate::nonexistent"));
}

#[tokio::test]
async fn description_source_hashes_for_batch_empty_input_returns_empty() {
    let client = GraphClient::connect(&url(), "mycel:test:hash_batch_empty").await.unwrap();
    let map = client.description_source_hashes_for_batch(&[]).await.unwrap();
    assert!(map.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mycel-graph --test integration description_source_hashes_for_batch`
Expected: FAIL — method does not exist.

- [ ] **Step 3: Implement the batched reader**

In `crates/mycel-graph/src/symbol.rs`, inside `impl GraphClient`:

```rust
/// Single-round-trip read of `description_source_hash` for a list of
/// qualified names. Symbols with no description (or whose source_hash
/// is unset) are absent from the returned map. Used by the daemon's
/// incremental staleness pre-pass to avoid N round-trips per file.
///
/// Empty input returns an empty map without issuing a Cypher query.
pub async fn description_source_hashes_for_batch(
    &self,
    qnames: &[&str],
) -> Result<std::collections::HashMap<String, String>> {
    if qnames.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let in_list = qnames
        .iter()
        .map(|q| format!("'{}'", escape(q)))
        .collect::<Vec<_>>()
        .join(", ");
    let cypher = format!(
        "MATCH (s:Symbol) \
         WHERE s.qualified_name IN [{in_list}] \
           AND coalesce(s.description_source_hash, '') <> '' \
         RETURN s.qualified_name, s.description_source_hash"
    );
    let rows = self.query(&cypher).await?;
    let mut out = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        let mut iter = row.into_iter();
        let qname = match iter.next() {
            Some(FalkorValue::String(s)) => s,
            _ => continue,
        };
        let hash = match iter.next() {
            Some(FalkorValue::String(s)) if !s.is_empty() => s,
            _ => continue,
        };
        out.insert(qname, hash);
    }
    Ok(out)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mycel-graph --test integration description_source_hashes_for_batch`
Expected: PASS for both cases.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-graph/src/symbol.rs crates/mycel-graph/tests/integration.rs
git commit -m "Add description_source_hashes_for_batch graph method"
```

### Task 1.5: Add `clear_symbol_description_and_reembed` graph method

The daemon's staleness path calls this when an edit changes the body that a description was written against. Atomic: clear description, replace embedding with a fresh signature+body embedding, clear `description_source_hash` — all in one Cypher statement.

**Files:**
- Modify: `crates/mycel-graph/src/symbol.rs`
- Test: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-graph/tests/integration.rs`:

```rust
#[tokio::test]
async fn clear_description_and_reembed_resets_atomic() {
    let client = GraphClient::connect(&url(), "mycel:test:clear").await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::clearer"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn clearer()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("body-v1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    client.set_symbol_description_and_embedding(
        "crate::clearer", "Clears state.", &vec![0.5f32; 768],
    ).await.unwrap();

    // After clear: description NULL, source_hash NULL, embedding replaced,
    // body_hash UPDATED to the new value (the body slice changed; the
    // caller passes the new hash explicitly).
    let new_vec = vec![0.9f32; 768];
    client.clear_symbol_description_and_reembed(
        "crate::clearer", "body-v2", &new_vec,
    ).await.unwrap();

    let info = client.get_symbol_description("crate::clearer").await.unwrap().unwrap();
    assert_eq!(info.description, None);
    assert_eq!(info.description_source_hash, None);
    assert_eq!(info.body_hash.as_deref(), Some("body-v2"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mycel-graph --test integration clear_description_and_reembed_resets_atomic`
Expected: FAIL — method does not exist.

- [ ] **Step 3: Implement the method**

In `crates/mycel-graph/src/symbol.rs`, inside `impl GraphClient`:

```rust
/// Clears `synthesized_description` and `description_source_hash`, replaces
/// `embedding` with the fresh signature+body embedding, and updates
/// `body_hash` — all atomically. Used by the daemon's incremental path
/// when a Symbol's body has changed since its description was written.
///
/// We do NOT preserve the old description. A description written against
/// a function body that no longer exists is worse than no description at
/// all — `find` clusters around behavior that's been removed/refactored.
pub async fn clear_symbol_description_and_reembed(
    &self,
    qname: &str,
    new_body_hash: &str,
    new_embedding: &[f32],
) -> Result<()> {
    let vec_lit = new_embedding
        .iter()
        .map(|f| f.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let cypher = format!(
        "MATCH (s:Symbol {{qualified_name: '{q}'}}) \
         SET s.synthesized_description = NULL, \
             s.description_source_hash = NULL, \
             s.body_hash = '{h}', \
             s.embedding = vecf32([{v}])",
        q = escape(qname),
        h = escape(new_body_hash),
        v = vec_lit,
    );
    self.query(&cypher).await?;
    Ok(())
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p mycel-graph --test integration clear_description_and_reembed_resets_atomic`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-graph/src/symbol.rs crates/mycel-graph/tests/integration.rs
git commit -m "Add clear_symbol_description_and_reembed graph method"
```

### Task 1.6: Add `list_stale_descriptions` and `refresh_description_source_hashes`

Two batch methods:
- `list_stale_descriptions()` — returns Symbols where description exists AND `description_source_hash != body_hash`. The daemon does NOT use this method (it operates per-file); CLI maintenance commands and tests use it.
- `refresh_description_source_hashes()` — for legacy graphs (descriptions written before this revision, no source_hash). Sets `description_source_hash = body_hash` for every Symbol with a description but NULL source_hash. Backs `mycel synthesize --refresh-hashes-only`.

**Files:**
- Modify: `crates/mycel-graph/src/symbol.rs`
- Test: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/mycel-graph/tests/integration.rs`:

```rust
#[tokio::test]
async fn list_stale_descriptions_finds_diverged_hashes() {
    let client = GraphClient::connect(&url(), "mycel:test:stale").await.unwrap();

    // Symbol A: description matches body_hash → fresh
    let a = Symbol {
        qualified_name: QualifiedName::new("crate::fresh"),
        kind: SymbolKind::Function, file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn fresh()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&a).await.unwrap();
    client.set_symbol_description_and_embedding(
        "crate::fresh", "Fresh.", &vec![0.0; 768],
    ).await.unwrap();

    // Symbol B: description set, then body_hash changed via raw update
    let b = Symbol {
        qualified_name: QualifiedName::new("crate::stale"),
        body_hash: Some("h1".into()), description_source_hash: None,
        ..a.clone()
    };
    client.upsert_symbol(&b).await.unwrap();
    client.set_symbol_description_and_embedding(
        "crate::stale", "Will go stale.", &vec![0.0; 768],
    ).await.unwrap();
    // Simulate a body edit by manually advancing body_hash
    client.query(
        "MATCH (s:Symbol {qualified_name: 'crate::stale'}) SET s.body_hash = 'h2'"
    ).await.unwrap();

    let stale = client.list_stale_descriptions().await.unwrap();
    let stale_qnames: std::collections::HashSet<_> =
        stale.iter().map(|s| s.qualified_name.clone()).collect();
    assert!(stale_qnames.contains("crate::stale"));
    assert!(!stale_qnames.contains("crate::fresh"));
}

#[tokio::test]
async fn refresh_description_source_hashes_only_touches_legacy_rows() {
    let client = GraphClient::connect(&url(), "mycel:test:refresh").await.unwrap();

    // Legacy: description present, source_hash NULL, body_hash present.
    let legacy = Symbol {
        qualified_name: QualifiedName::new("crate::legacy"),
        kind: SymbolKind::Function, file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn legacy()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("hL".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&legacy).await.unwrap();
    // Write description directly via raw cypher to skip the source_hash stamp,
    // simulating a 2026-05-04-shape graph.
    client.query(
        "MATCH (s:Symbol {qualified_name: 'crate::legacy'}) \
         SET s.synthesized_description = 'old desc', \
             s.embedding = vecf32([0.0])"
    ).await.unwrap();

    // Modern: description AND source_hash present (must NOT be touched).
    let modern = Symbol {
        qualified_name: QualifiedName::new("crate::modern"),
        body_hash: Some("hM".into()),
        ..legacy.clone()
    };
    client.upsert_symbol(&modern).await.unwrap();
    client.set_symbol_description_and_embedding(
        "crate::modern", "modern desc", &vec![0.0; 768],
    ).await.unwrap();

    let n = client.refresh_description_source_hashes().await.unwrap();
    assert_eq!(n, 1, "exactly one legacy row backfilled");

    let l = client.get_symbol_description("crate::legacy").await.unwrap().unwrap();
    assert_eq!(l.description_source_hash.as_deref(), Some("hL"),
        "legacy row's source_hash backfilled from body_hash");
    let m = client.get_symbol_description("crate::modern").await.unwrap().unwrap();
    assert_eq!(m.description_source_hash.as_deref(), Some("hM"),
        "modern row untouched");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mycel-graph --test integration list_stale_descriptions_finds_diverged_hashes refresh_description_source_hashes_only_touches_legacy_rows`
Expected: FAIL — methods don't exist.

- [ ] **Step 3: Implement both methods**

In `crates/mycel-graph/src/symbol.rs`, inside `impl GraphClient`:

```rust
/// Returns Symbols whose `description_source_hash` no longer matches
/// `body_hash`. These are descriptions written against a body that has
/// since been edited — stale by definition.
///
/// Legacy graphs (Symbols with a description but NULL source_hash)
/// are NOT returned here. Run `refresh_description_source_hashes` once
/// to bring them under the staleness regime, then they participate
/// normally on subsequent edits.
///
/// Uses `coalesce(...) <> ''` for "is set" checks because FalkorDB's
/// Cypher dialect treats equality against NULL as NULL (not false), so
/// `IS NOT NULL` predicates filter unreliably; see the doc-comment on
/// `list_symbols_for_synthesis` for the full reasoning.
pub async fn list_stale_descriptions(&self) -> Result<Vec<SymbolForSynthesis>> {
    let cypher =
        "MATCH (s:Symbol) \
         WHERE coalesce(s.synthesized_description, '') <> '' \
           AND coalesce(s.description_source_hash, '') <> '' \
           AND s.description_source_hash <> s.body_hash \
         RETURN s.qualified_name, s.signature, s.file_path, s.start_line, s.end_line, \
                coalesce(s.synthesized_description, '') AS desc";
    let rows = self.query(cypher).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut iter = row.into_iter();
        let qname = match iter.next() { Some(FalkorValue::String(s)) => s, _ => continue };
        let signature = match iter.next() { Some(FalkorValue::String(s)) => s, _ => String::new() };
        let file_path = match iter.next() { Some(FalkorValue::String(s)) => s, _ => continue };
        let start_line = match iter.next() { Some(FalkorValue::I64(n)) => n as u32, _ => 0 };
        let end_line = match iter.next() { Some(FalkorValue::I64(n)) => n as u32, _ => 0 };
        let has_description = matches!(iter.next(), Some(FalkorValue::String(s)) if !s.is_empty());
        out.push(SymbolForSynthesis {
            qualified_name: qname, signature, file_path,
            start_line, end_line, has_description,
        });
    }
    Ok(out)
}

/// Backfills `description_source_hash` from `body_hash` on Symbols that
/// already have a description but whose source_hash is NULL — i.e.,
/// descriptions written before the 2026-05-05 redirection. Returns the
/// number of rows updated.
///
/// `coalesce(...) <> ''` for "is set" and `coalesce(..., '') = ''` for
/// "is unset" — FalkorDB's `IS NULL`/`IS NOT NULL` is unreliable, see
/// the doc-comment on `list_symbols_for_synthesis`.
pub async fn refresh_description_source_hashes(&self) -> Result<usize> {
    let cypher =
        "MATCH (s:Symbol) \
         WHERE coalesce(s.synthesized_description, '') <> '' \
           AND coalesce(s.description_source_hash, '') = '' \
           AND coalesce(s.body_hash, '') <> '' \
         SET s.description_source_hash = s.body_hash \
         RETURN count(s) AS updated";
    let rows = self.query(cypher).await?;
    let updated = rows
        .into_iter().next()
        .and_then(|r| r.into_iter().next())
        .and_then(|v| match v { FalkorValue::I64(n) => Some(n as usize), _ => None })
        .unwrap_or(0);
    Ok(updated)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mycel-graph --test integration list_stale_descriptions_finds_diverged_hashes refresh_description_source_hashes_only_touches_legacy_rows`
Expected: PASS.

Run full graph integration suite: `cargo test -p mycel-graph --test integration`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-graph/src/symbol.rs crates/mycel-graph/tests/integration.rs
git commit -m "Add list_stale_descriptions and refresh_description_source_hashes"
```

### Task 1.7: Chunk 1 verification

- [ ] Run the full workspace test suite once: `cargo test --workspace`. Expected: all green, including the four new graph integration tests.
- [ ] Run clippy: `cargo clippy --workspace --all-targets -- -D warnings`. Expected: clean.
- [ ] Sanity-check: open the FalkorDB browser at `http://127.0.0.1:13000`, run `MATCH (s:Symbol) RETURN s LIMIT 1` — confirm the new properties are accepted by the schema (FalkorDB will accept any property; this is a sanity check, not a gate).

---

## Chunk 2: Cold-index pipeline change

This chunk is the behavioral pivot: `mycel index` stops calling the Synthesizer, embeds `signature + body slice` instead of just signature, and writes `body_hash` for every Symbol. The daemon's per-file path inherits the change for free because it shares `index_file_collect_edges`.

### Task 2.1: Extract a shared body-slicing helper into `mycel-index`

`crates/mycel-index/src/synthesize.rs:30` defines `BODY_LINE_CAP = 60` privately, and `read_body_slice` at `crates/mycel-index/src/synthesize.rs:192-216` reads + caches body text. The pipeline needs the same capability for embedding. Extract to a new module so both call sites share one definition.

**Files:**
- Create: `crates/mycel-index/src/body_slice.rs`
- Modify: `crates/mycel-index/src/lib.rs` (add `pub mod body_slice`)
- Modify: `crates/mycel-index/src/synthesize.rs` (use the shared helper, drop the local copy)

- [ ] **Step 1: Write the failing test**

Create `crates/mycel-index/src/body_slice.rs` with:

```rust
//! Shared helper for slicing a Symbol's body out of a source file and
//! computing a stable hash of the slice. Used by both:
//! - `pipeline.rs` — to compute the embedding input for cold index
//! - `synthesize.rs` — to build a Synthesizer prompt
//!
//! Same input → same output → same hash, so the pipeline's `body_hash`
//! and the synthesizer's prompt body agree byte-for-byte.

use std::collections::HashMap;

/// Cap on body lines included in the slice. Keeps embed input under
/// EmbeddingGemma's 2048-token context with margin and prevents a 5000-line
/// legacy file from dominating the embedding signal.
pub const BODY_LINE_CAP: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodySlice {
    pub text: String,
    pub hash: String,
}

/// Lazily read and cache file contents. `None` for files that don't exist
/// or aren't UTF-8 — both treated as "no body available."
pub type FileCache = HashMap<String, Option<Vec<String>>>;

/// Build the (signature + body slice) text used as the embed input AND
/// hash that text with blake3. Returns the same string and hash regardless
/// of which call site invokes it.
///
/// `start_line` and `end_line` are 1-indexed and inclusive (matching how
/// extractors report them).
pub fn signature_plus_body_slice(
    cache: &mut FileCache,
    signature: &str,
    file_path: &str,
    start_line: u32,
    end_line: u32,
) -> BodySlice {
    let body = read_body_slice(cache, file_path, start_line, end_line);
    let mut text = String::with_capacity(signature.len() + body.as_deref().map(str::len).unwrap_or(0) + 2);
    text.push_str(signature);
    if let Some(b) = body {
        text.push_str("\n\n");
        text.push_str(&b);
    }
    let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
    BodySlice { text, hash }
}

/// Read up to `BODY_LINE_CAP` lines of the body and return them joined.
/// Truncation appends a marker so the embedder doesn't see an abrupt cut.
pub fn read_body_slice(
    cache: &mut FileCache,
    path: &str,
    start: u32,
    end: u32,
) -> Option<String> {
    let entry = cache.entry(path.to_string()).or_insert_with(|| {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.lines().map(|l| l.to_string()).collect())
    });
    let lines = entry.as_ref()?;
    let start_idx = (start.saturating_sub(1)) as usize;
    let end_idx = (end as usize).min(lines.len());
    if start_idx >= end_idx { return None; }
    let slice = &lines[start_idx..end_idx];
    let cap = BODY_LINE_CAP.min(slice.len());
    let mut body = slice[..cap].join("\n");
    if slice.len() > cap {
        body.push_str("\n// ... (body truncated)");
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_plus_body_slice_is_deterministic() {
        let dir = std::env::temp_dir().join(format!(
            "mycel-bs-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.rs");
        std::fs::write(&p, "fn a() {\n  let x = 1;\n  x + 1\n}\n").unwrap();
        let mut cache = FileCache::new();
        let s1 = signature_plus_body_slice(&mut cache, "fn a()", p.to_str().unwrap(), 1, 4);
        let s2 = signature_plus_body_slice(&mut cache, "fn a()", p.to_str().unwrap(), 1, 4);
        assert_eq!(s1, s2);
        assert!(s1.text.contains("fn a()"));
        assert!(s1.text.contains("let x = 1"));
        assert_eq!(s1.hash.len(), 64);  // blake3 hex
    }

    #[test]
    fn missing_file_returns_signature_only() {
        let mut cache = FileCache::new();
        let s = signature_plus_body_slice(&mut cache, "fn missing()", "/no/such/file.rs", 1, 5);
        assert_eq!(s.text, "fn missing()");
        assert_eq!(s.hash.len(), 64);
    }

    #[test]
    fn truncation_marker_appended_when_over_cap() {
        let dir = std::env::temp_dir().join(format!(
            "mycel-bs-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.rs");
        let content: String = (1..=200).map(|i| format!("line{i}\n")).collect();
        std::fs::write(&p, &content).unwrap();
        let mut cache = FileCache::new();
        let s = signature_plus_body_slice(&mut cache, "fn big()", p.to_str().unwrap(), 1, 200);
        assert!(s.text.contains("(body truncated)"));
        assert!(s.text.starts_with("fn big()\n\nline1"));
    }
}
```

- [ ] **Step 2: Wire the new module**

In `crates/mycel-index/src/lib.rs`, add:

```rust
pub mod body_slice;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p mycel-index body_slice`
Expected: PASS — the three new unit tests.

- [ ] **Step 4: Refactor `synthesize.rs` to use the shared helper**

In `crates/mycel-index/src/synthesize.rs`:
- Delete the local `BODY_LINE_CAP` constant at line 30 and the local `read_body_slice` at lines 192-216.
- Replace internal references with imports from `crate::body_slice`:
  ```rust
  use crate::body_slice::{read_body_slice, BODY_LINE_CAP};
  ```
- Move (delete from `synthesize.rs`, paste into `body_slice.rs` under the existing `#[cfg(test)] mod tests`) the existing `synthesize.rs` tests `read_body_slice_caps_long_bodies` and `read_body_slice_handles_missing_file_and_caches_negative`. Tests follow the function they exercise. Run the full suite after the move to catch missed import adjustments.

- [ ] **Step 5: Run tests**

Run: `cargo test -p mycel-index`
Expected: PASS — both `body_slice` and `synthesize` modules compile and test.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-index/src/body_slice.rs crates/mycel-index/src/lib.rs crates/mycel-index/src/synthesize.rs
git commit -m "Extract shared body_slice helper for pipeline + synthesizer"
```

### Task 2.2: Embed `signature + body slice` and write `body_hash` in the index pipeline

**Files:**
- Modify: `crates/mycel-index/src/pipeline.rs:96-120` (the embed loop in `index_file_collect_edges`)
- Test: `crates/mycel-graph/tests/integration.rs` (an integration test exercising the new behavior end-to-end is light and stays close to the schema layer)

- [ ] **Step 1: Write the failing test (small unit-level test against the embed path)**

Add a unit test inside `crates/mycel-index/src/pipeline.rs` (under a new `#[cfg(test)]` module if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::body_slice::{signature_plus_body_slice, FileCache};

    /// Sanity: the helper used by the pipeline produces a deterministic
    /// (text, hash) pair so the same Symbol-and-body always yields the
    /// same hash that we'll persist on the node.
    #[test]
    fn embed_input_helper_is_stable() {
        let dir = std::env::temp_dir().join(format!(
            "mycel-pipe-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.rs");
        std::fs::write(&p, "fn s() { 1 + 2 }\n").unwrap();

        let mut cache = FileCache::new();
        let bs = signature_plus_body_slice(&mut cache, "fn s()", p.to_str().unwrap(), 1, 1);
        assert!(bs.text.contains("fn s()"));
        assert!(bs.text.contains("fn s() { 1 + 2 }"));
        assert_eq!(bs.hash.len(), 64);
    }
}
```

(This test exists mainly to document the expectation; the real guarantee comes from the integration test added in Step 6 below.)

- [ ] **Step 2: Modify the embed loop**

In `crates/mycel-index/src/pipeline.rs:96-120`, replace the existing block:

```rust
// 5. Embed signature + body slice (Phase 2 revision: cold index no longer
//    runs the Synthesizer; the embedding source is the same string the
//    body_hash is computed over, so embedding and hash are always paired).
//
//    CRITICAL: pair each symbol with its OWN embedding. The chunk-of-32
//    batching means we must zip the symbol-chunk with the text-chunk,
//    not zip `extraction.symbols.iter()` (which always restarts at 0)
//    with the most recent batch's vectors.
if !extraction.symbols.is_empty() {
    use crate::body_slice::{signature_plus_body_slice, FileCache};
    let mut cache = FileCache::new();
    let prepared: Vec<(String, String)> = extraction
        .symbols
        .iter()
        .map(|s| {
            let bs = signature_plus_body_slice(
                &mut cache,
                s.signature.as_str(),
                s.file_path.as_str(),
                s.start_line,
                s.end_line,
            );
            (bs.text, bs.hash)
        })
        .collect();
    for (sym_chunk, prep_chunk) in extraction.symbols.chunks(32).zip(prepared.chunks(32)) {
        let text_chunk: Vec<&str> = prep_chunk.iter().map(|(t, _)| t.as_str()).collect();
        let vecs = self.embedder.embed(&text_chunk).await?;
        if vecs.len() != sym_chunk.len() {
            return Err(MycelError::Model {
                provider: self.embedder.identity().into(),
                message: format!(
                    "expected {} embeddings, got {}",
                    sym_chunk.len(),
                    vecs.len()
                ),
            });
        }
        for ((sym, vec), (_, hash)) in sym_chunk.iter().zip(vecs.iter()).zip(prep_chunk.iter()) {
            self.graph
                .set_symbol_embedding_and_body_hash(
                    sym.qualified_name.as_str(), vec, hash.as_str(),
                )
                .await?;
        }
    }
}
```

Each iteration zips the symbol-chunk, the embedded-vector-chunk, and the prepared (text, hash)-chunk together — every symbol writes its OWN embedding and OWN body_hash in one atomic call. No per-batch Vec allocation beyond the standard chunked-borrow pattern.

- [ ] **Step 3: Add `set_symbol_embedding_and_body_hash` and delete `set_symbol_embedding`**

The existing `set_symbol_embedding` lives at **`crates/mycel-graph/src/file.rs:75`** (not `symbol.rs` — verified at planning time). The new method belongs alongside it, in the same file. Replace `set_symbol_embedding` with `set_symbol_embedding_and_body_hash`:

```rust
/// Writes embedding and body_hash atomically. The two fields must move
/// together — body_hash describes which `signature + body slice` the
/// embedding was computed against. Splitting the write would let the
/// daemon observe a stale embedding paired with a fresh hash (or vice
/// versa) and falsely conclude staleness was/wasn't present.
pub async fn set_symbol_embedding_and_body_hash(
    &self,
    qname: &str,
    embedding: &[f32],
    body_hash: &str,
) -> Result<()> {
    let vec_lit = embedding.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(",");
    let cypher = format!(
        "MATCH (s:Symbol {{qualified_name: '{q}'}}) \
         SET s.embedding = vecf32([{v}]), s.body_hash = '{h}'",
        q = escape(qname),
        v = vec_lit,
        h = escape(body_hash),
    );
    self.query(&cypher).await?;
    Ok(())
}
```

**Delete `set_symbol_embedding`.** Audit with `rg -n 'set_symbol_embedding\b' crates/`. The only in-tree caller is the line in `pipeline.rs:96-120` that Step 2 already replaced — after that edit, `set_symbol_embedding` has zero callers. `cargo clippy -D warnings` does not flag dead public functions by default, so leaving it would silently rot. Remove the function and any doc-comments referencing it.

Note: `crates/mycel-graph/src/symbol.rs:308` has a comment that mentions `set_symbol_embedding` historically — that comment is descriptive of the *bug class* the atomic write avoids and remains accurate after this change. Leave it.

- [ ] **Step 4: Update the pipeline test fixture in `mycel-graph/tests/integration.rs`**

Add an integration test that runs an `Indexer` over a tiny synthetic file and verifies the resulting Symbol carries embedding + body_hash:

```rust
#[tokio::test]
async fn cold_index_writes_embedding_and_body_hash() {
    let client = GraphClient::connect(&url(), "mycel:test:cold_index").await.unwrap();

    // Use a stub embedder so we don't depend on a running Ollama.
    struct StubEmbedder;
    #[async_trait::async_trait]
    impl mycel_models::Embedder for StubEmbedder {
        fn identity(&self) -> &str { "stub/test" }
        fn dimension(&self) -> u32 { 768 }
        async fn embed(&self, texts: &[&str]) -> mycel_core::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.42_f32; 768]).collect())
        }
    }

    // Include synthesizer: None here. Task 2.3 removes the field from
    // the Indexer struct; updating this literal happens as part of that
    // task's compile-driven sweep. Keeping it now means the suite stays
    // green between Task 2.2 and Task 2.3.
    let indexer = mycel_index::Indexer {
        graph: client.clone(),
        lsp: None,
        embedder: std::sync::Arc::new(StubEmbedder),
        synthesizer: None,
    };

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("hello.rs");
    std::fs::write(&f, "fn hello() { println!(\"hi\"); }\n").unwrap();
    let f_utf8 = camino::Utf8PathBuf::from_path_buf(f).unwrap();
    indexer.index_file(&f_utf8, "fn hello() { println!(\"hi\"); }\n").await.unwrap();

    let rows = client.query(
        "MATCH (s:Symbol) WHERE s.file_path ENDS WITH 'hello.rs' \
         RETURN s.qualified_name, s.body_hash, s.embedding"
    ).await.unwrap();
    assert!(!rows.is_empty(), "expected at least one symbol");
    for row in rows {
        let body_hash = match &row[1] {
            falkordb::FalkorValue::String(s) => s.clone(),
            other => panic!("body_hash: {other:?}"),
        };
        assert_eq!(body_hash.len(), 64, "blake3 hex");
    }
}
```

This requires both `tempfile` (for the temp directory) and `async-trait` (for the `#[async_trait::async_trait]` impl on the stub embedder) as dev-dependencies in `crates/mycel-graph/Cargo.toml`. Verify neither is already present (`grep -E 'tempfile|async-trait' crates/mycel-graph/Cargo.toml`); add under `[dev-dependencies]`:

```toml
[dev-dependencies]
tempfile = "3"
async-trait = { workspace = true }
mycel-models = { path = "../mycel-models" }
```

`mycel-models` is needed because the test impls `mycel_models::Embedder` for the stub. `async-trait` is already a workspace dep so the workspace-true variant works.

- [ ] **Step 5: Run the integration test**

Run: `cargo test -p mycel-graph --test integration cold_index_writes_embedding_and_body_hash`
Expected: PASS.

Run: `cargo test --workspace`
Expected: full green.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-index/src/pipeline.rs crates/mycel-graph/src/file.rs crates/mycel-graph/tests/integration.rs crates/mycel-graph/Cargo.toml
git commit -m "Cold index: embed signature+body slice and write body_hash"
```

### Task 2.3: Drop the synthesizer field from `Indexer` and remove the trailing synth pass

**Files:**
- Modify: `crates/mycel-index/src/pipeline.rs:10-19` (struct), `:214-234` (trailing pass)
- Modify: `crates/mycel-cli/src/main.rs:51-56` (Index arm — stops constructing a synthesizer)
- Modify: `crates/mycel-daemon/src/main.rs` (drop synthesizer if it constructs an Indexer)

- [ ] **Step 1: Remove the field and the call site**

In `crates/mycel-index/src/pipeline.rs:10-19`, replace the `Indexer` struct:

```rust
pub struct Indexer {
    pub graph: GraphClient,
    pub lsp: Option<Arc<MultilspyResolver>>,
    pub embedder: Arc<dyn Embedder>,
}
```

Remove the `use mycel_models::{Embedder, Synthesizer};` and replace with `use mycel_models::Embedder;`.

In `crates/mycel-index/src/pipeline.rs:214-234`, delete the entire trailing block:

```rust
// Phase 2: description synthesis as a final pass...
if let Some(synth) = &self.synthesizer { ... }
```

The `index_repo` method ends after the manifest write.

- [ ] **Step 2: Update CLI Index arm**

In `crates/mycel-cli/src/main.rs:51-56`, remove:

```rust
let synthesizer = if no_descriptions {
    None
} else {
    config::synthesizer_from_cfg(&cfg)
};
let indexer = Indexer { graph: g, lsp, embedder, synthesizer };
```

Replace with:

```rust
let indexer = Indexer { graph: g, lsp, embedder };
```

(The `--no-descriptions` flag is removed entirely in Task 3.6.)

- [ ] **Step 3: Update daemon if it constructs an `Indexer`**

Check: `rg 'Indexer \{' crates/mycel-daemon`. If the daemon builds an Indexer with `synthesizer:`, drop that field. If it doesn't (the field was Optional and the daemon passed `None`), it still won't compile — drop the field per Step 1.

- [ ] **Step 4: Build, test, clippy**

Run: `cargo build --workspace`
Expected: clean.

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

Manual sanity: `cargo run --release -p mycel-cli -- --repo . index . 2>&1 | head -20`. Expected: indexing runs without invoking Ollama for synthesis. Should complete in seconds (vs. minutes previously) because no per-symbol LLM call.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-index/src/pipeline.rs crates/mycel-cli/src/main.rs crates/mycel-daemon/src/main.rs
git commit -m "Remove eager synthesis pass from index_repo"
```

### Task 2.4: Chunk 2 verification

- [ ] Manually re-index this repo: `target/release/mycel --repo . index .`. Expected wall-clock under 5 minutes (vs ~30 minutes pre-revision).
- [ ] Verify Symbols carry `body_hash`: `target/release/mycel --repo . definers Indexer --json` returns the symbol; raw FalkorDB query against `s.body_hash` shows a 64-char hex string.
- [ ] Verify no descriptions were written: `MATCH (s:Symbol) WHERE s.synthesized_description IS NOT NULL RETURN count(s)` returns 0 (on a fresh graph).
- [ ] `mycel find` still returns sensible results — try a few queries against well-known symbols like "embedder identity" and verify the right hits appear.

---

## Chunk 3: New CLI commands

`mycel describe`, `mycel set-description`, `mycel skill install`, plus the migration flags `mycel synthesize --refresh-hashes-only` and `mycel index --force-cold-rebuild`. The `--no-descriptions` flag on `mycel index` is removed (no longer meaningful — synthesis is no longer part of indexing).

### Task 3.1: Add `mycel describe <qname>` command

**Files:**
- Modify: `crates/mycel-cli/src/cli.rs` (add `Describe` variant)
- Modify: `crates/mycel-cli/src/main.rs` (handle `Describe`)
- Test: `crates/mycel-cli/tests/cli_integration.rs` (new file)

- [ ] **Step 1: Create the test directory and write the failing test**

Create `crates/mycel-cli/tests/cli_integration.rs`:

```rust
//! End-to-end CLI tests against an ephemeral FalkorDB. Each test isolates
//! to its own graph name. Requires the test container at
//! redis://127.0.0.1:16379 (same as mycel-graph integration tests).

use assert_cmd::Command;
use mycel_core::*;
use mycel_graph::GraphClient;
use predicates::str::contains;

fn url() -> String {
    std::env::var("MYCEL_TEST_FALKORDB_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:16379".into())
}

#[tokio::test]
async fn describe_prints_none_for_undescribed_symbol() {
    let graph_name = "mycel:cli_test:desc_none";
    let client = GraphClient::connect(&url(), graph_name).await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::described"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn described()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    Command::cargo_bin("mycel").unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name)
        .args(["describe", "crate::described"])
        .assert()
        .success()
        .stdout(contains("(none)"));
}

#[tokio::test]
async fn describe_prints_text_when_set() {
    let graph_name = "mycel:cli_test:desc_text";
    let client = GraphClient::connect(&url(), graph_name).await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::with_desc"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn with_desc()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    client.set_symbol_description_and_embedding(
        "crate::with_desc", "Does a thing.", &vec![0.0; 768],
    ).await.unwrap();

    Command::cargo_bin("mycel").unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name)
        .args(["describe", "crate::with_desc"])
        .assert()
        .success()
        .stdout(contains("Does a thing."));
}

#[tokio::test]
async fn describe_json_includes_hashes() {
    let graph_name = "mycel:cli_test:desc_json";
    let client = GraphClient::connect(&url(), graph_name).await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::with_json"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn with_json()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    client.set_symbol_description_and_embedding(
        "crate::with_json", "Json description.", &vec![0.0; 768],
    ).await.unwrap();

    let out = Command::cargo_bin("mycel").unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name)
        .args(["--json", "describe", "crate::with_json"])
        .output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"description\""));
    assert!(stdout.contains("\"body_hash\""));
    assert!(stdout.contains("\"description_source_hash\""));
    assert!(stdout.contains("Json description."));
}
```

Add dev-dependencies to `crates/mycel-cli/Cargo.toml`:

```toml
[dev-dependencies]
assert_cmd = "2"
predicates = "3"
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
mycel-core = { path = "../mycel-core" }
mycel-graph = { path = "../mycel-graph" }
```

Note: a `MYCEL_TEST_GRAPH` env var override is added in this task to let tests target deterministic graph names. Implementation: in `main.rs`, the `open()` helper consults `MYCEL_TEST_GRAPH` if set, falling back to `mycel:<repo_id>` otherwise. Document this is **test-only** in a code comment.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mycel-cli --test cli_integration describe_prints_none_for_undescribed_symbol`
Expected: FAIL — `describe` subcommand doesn't exist.

- [ ] **Step 3: Add the `Describe` variant to the CLI**

In `crates/mycel-cli/src/cli.rs`, add to the `Cmd` enum:

```rust
/// Print the current synthesized description for a Symbol, or "(none)".
/// With --json, emits {description, body_hash, description_source_hash}.
Describe { qname: String },
```

- [ ] **Step 4: Wire the `Describe` arm**

In `crates/mycel-cli/src/main.rs`, add a match arm:

```rust
Cmd::Describe { qname } => {
    let g = open(&cfg, &cli.repo).await?;
    let info = g.get_symbol_description(&qname).await?
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
```

Add the `MYCEL_TEST_GRAPH` override in `open()`:

```rust
async fn open(cfg: &config::Config, repo: &Option<Utf8PathBuf>) -> anyhow::Result<GraphClient> {
    // MYCEL_TEST_GRAPH overrides the derived graph name. Test-only — production
    // callers should leave it unset; documented for cli_integration tests.
    let graph_name = std::env::var("MYCEL_TEST_GRAPH")
        .unwrap_or_else(|_| format!("mycel:{}", repo.as_deref()
            .map(repo_id_from_path)
            .unwrap_or_else(|| "default".into())));
    GraphClient::connect(&cfg.storage.falkordb_url, &graph_name).await
        .context("connect to FalkorDB")
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p mycel-cli --test cli_integration`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-cli
git commit -m "Add mycel describe <qname> command"
```

### Task 3.2: Add `mycel set-description --qname --description`

**Files:**
- Modify: `crates/mycel-cli/src/cli.rs`
- Modify: `crates/mycel-cli/src/main.rs`
- Test: `crates/mycel-cli/tests/cli_integration.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-cli/tests/cli_integration.rs`:

```rust
#[tokio::test]
async fn set_description_writes_and_describe_reads_back() {
    let graph_name = "mycel:cli_test:set_desc";
    let client = GraphClient::connect(&url(), graph_name).await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::settable"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn settable()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("hb".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    // NOTE: this test requires Ollama to be running on the configured endpoint
    // for the embedding step. Tests that need a stub embedder should test the
    // underlying graph methods directly; this test is the end-to-end shape
    // verification. `MYCEL_SKIP_OLLAMA_TESTS` is a NEW convention introduced
    // here — document it in CLAUDE.md when this lands so future CI scripts
    // can opt out of Ollama-dependent tests cleanly.
    if std::env::var("MYCEL_SKIP_OLLAMA_TESTS").is_ok() {
        eprintln!("skipping (MYCEL_SKIP_OLLAMA_TESTS set)");
        return;
    }

    Command::cargo_bin("mycel").unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name)
        .args([
            "set-description",
            "--qname", "crate::settable",
            "--description", "Sets a thing.",
        ])
        .assert()
        .success();

    let info = client.get_symbol_description("crate::settable").await.unwrap().unwrap();
    assert_eq!(info.description.as_deref(), Some("Sets a thing."));
    assert_eq!(info.description_source_hash.as_deref(), Some("hb"));
}

#[tokio::test]
async fn set_description_unknown_qname_exits_nonzero_with_helpful_error() {
    let graph_name = "mycel:cli_test:set_desc_unknown";
    let _ = GraphClient::connect(&url(), graph_name).await.unwrap();

    Command::cargo_bin("mycel").unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name)
        .args([
            "set-description",
            "--qname", "crate::not::a::real::symbol",
            "--description", "Whatever.",
        ])
        .assert()
        .failure()
        .stderr(contains("no Symbol"))
        .stderr(contains("definers"));  // hint Claude at the recovery path
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mycel-cli --test cli_integration set_description_unknown_qname_exits_nonzero_with_helpful_error`
Expected: FAIL — subcommand absent.

- [ ] **Step 3: Add `SetDescription` to the CLI**

In `crates/mycel-cli/src/cli.rs`:

```rust
/// Write a behavioral description for a Symbol. Embeds the description
/// text, atomically updates the Symbol's description + embedding +
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
```

- [ ] **Step 4: Wire the arm**

In `crates/mycel-cli/src/main.rs`:

```rust
Cmd::SetDescription { qname, description } => {
    let g = open(&cfg, &cli.repo).await?;
    // Verify the qname resolves before paying for an embedding.
    if g.get_symbol_description(&qname).await?.is_none() {
        anyhow::bail!(
            "no Symbol with qualified_name '{qname}' — run `mycel definers <name>` to find the canonical qname"
        );
    }
    let embedder = config::embedder_from_cfg(&cfg);
    let mut vecs = embedder.embed(&[description.as_str()]).await
        .context("embed description")?;
    let vec = vecs.pop().ok_or_else(|| anyhow::anyhow!("embedder returned no vectors"))?;
    g.set_symbol_description_and_embedding(&qname, &description, &vec).await
        .context("write description + embedding")?;
    if json {
        println!("{}", serde_json::json!({"qualified_name": qname, "ok": true}));
    } else {
        println!("ok");
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p mycel-cli --test cli_integration`
Expected: PASS for `set_description_unknown_qname_exits_nonzero_with_helpful_error` unconditionally; PASS for `set_description_writes_and_describe_reads_back` when Ollama is available.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-cli/src/cli.rs crates/mycel-cli/src/main.rs crates/mycel-cli/tests/cli_integration.rs
git commit -m "Add mycel set-description command"
```

### Task 3.3: Add `mycel synthesize --refresh-hashes-only`

**Files:**
- Modify: `crates/mycel-cli/src/cli.rs:43-48` (Synthesize variant)
- Modify: `crates/mycel-cli/src/main.rs:60-77` (Synthesize arm)
- Test: `crates/mycel-cli/tests/cli_integration.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-cli/tests/cli_integration.rs`:

```rust
#[tokio::test]
async fn synthesize_refresh_hashes_only_backfills_legacy_rows() {
    let graph_name = "mycel:cli_test:refresh";
    let client = GraphClient::connect(&url(), graph_name).await.unwrap();

    // Legacy: description present but source_hash NULL
    let legacy = Symbol {
        qualified_name: QualifiedName::new("crate::legacy_sym"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn legacy_sym()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("legacy_h".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&legacy).await.unwrap();
    client.query(
        "MATCH (s:Symbol {qualified_name: 'crate::legacy_sym'}) \
         SET s.synthesized_description = 'old', s.embedding = vecf32([0.0])"
    ).await.unwrap();

    Command::cargo_bin("mycel").unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name)
        .args(["synthesize", "--refresh-hashes-only"])
        .assert()
        .success()
        .stdout(contains("backfilled 1"));

    let info = client.get_symbol_description("crate::legacy_sym").await.unwrap().unwrap();
    assert_eq!(info.description_source_hash.as_deref(), Some("legacy_h"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mycel-cli --test cli_integration synthesize_refresh_hashes_only_backfills_legacy_rows`
Expected: FAIL — flag does not exist.

- [ ] **Step 3: Add the flag**

In `crates/mycel-cli/src/cli.rs:43-48`, modify `Synthesize`:

```rust
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
```

- [ ] **Step 4: Wire the arm**

In `crates/mycel-cli/src/main.rs:60-77`, add a `--refresh-hashes-only` branch:

```rust
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
        &g, synthesizer, embedder,
        mycel_index::SynthesisOptions { force, limit, ..Default::default() },
    ).await?;
    println!(
        "synthesized {} of {} (skipped {}, failed {})",
        outcome.synthesized, outcome.considered, outcome.skipped, outcome.failed
    );
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p mycel-cli --test cli_integration synthesize_refresh_hashes_only_backfills_legacy_rows`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-cli/src/cli.rs crates/mycel-cli/src/main.rs crates/mycel-cli/tests/cli_integration.rs
git commit -m "Add mycel synthesize --refresh-hashes-only flag"
```

### Task 3.4: Add `mycel index --force-cold-rebuild`

**Files:**
- Modify: `crates/mycel-cli/src/cli.rs` (Index variant)
- Modify: `crates/mycel-cli/src/main.rs` (Index arm — pre-clears descriptions)
- Modify: `crates/mycel-graph/src/symbol.rs` (new `clear_all_descriptions` helper)
- Test: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Add the graph helper test**

Append to `crates/mycel-graph/tests/integration.rs`:

```rust
#[tokio::test]
async fn clear_all_descriptions_wipes_all_description_state() {
    let client = GraphClient::connect(&url(), "mycel:test:clear_all").await.unwrap();

    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::with_desc"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1, end_line: 2,
        signature: Signature::new("fn with_desc()"),
        jsdoc: None, synthesized_description: None, exported: true, embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    client.set_symbol_description_and_embedding(
        "crate::with_desc", "Has desc.", &vec![0.0; 768],
    ).await.unwrap();

    let n = client.clear_all_descriptions().await.unwrap();
    assert!(n >= 1);

    let info = client.get_symbol_description("crate::with_desc").await.unwrap().unwrap();
    assert_eq!(info.description, None);
    assert_eq!(info.description_source_hash, None);
}
```

- [ ] **Step 2: Implement `clear_all_descriptions`**

In `crates/mycel-graph/src/symbol.rs`:

```rust
/// Wipes synthesized_description and description_source_hash on every
/// Symbol in the graph. Embeddings are NOT cleared — the next index pass
/// will overwrite them with fresh signature+body embeddings via
/// `set_symbol_embedding_and_body_hash`. Returns the number of rows
/// affected.
pub async fn clear_all_descriptions(&self) -> Result<usize> {
    let cypher =
        "MATCH (s:Symbol) WHERE s.synthesized_description IS NOT NULL \
         SET s.synthesized_description = NULL, s.description_source_hash = NULL \
         RETURN count(s) AS cleared";
    let rows = self.query(cypher).await?;
    Ok(rows
        .into_iter().next()
        .and_then(|r| r.into_iter().next())
        .and_then(|v| match v { FalkorValue::I64(n) => Some(n as usize), _ => None })
        .unwrap_or(0))
}
```

- [ ] **Step 3: Run graph test**

Run: `cargo test -p mycel-graph --test integration clear_all_descriptions_wipes_all_description_state`
Expected: PASS.

- [ ] **Step 4: Add the CLI flag and wire it**

In `crates/mycel-cli/src/cli.rs`, modify the `Index` variant:

```rust
Index {
    path: Utf8PathBuf,
    /// Wipe all existing descriptions before indexing — useful when
    /// abandoning eager-synth descriptions in favor of workload-driven
    /// fill via the mycel-graph-care skill.
    #[arg(long)]
    force_cold_rebuild: bool,
},
```

(Note the removal of `no_descriptions` here — that flag is gone in Task 3.6.)

In `crates/mycel-cli/src/main.rs`, the `Index` arm:

```rust
Cmd::Index { path, force_cold_rebuild } => {
    let graph_name = std::env::var("MYCEL_TEST_GRAPH")
        .unwrap_or_else(|_| format!("mycel:{}", repo_id_from_path(&path)));
    let g = GraphClient::connect(&cfg.storage.falkordb_url, &graph_name).await?;
    if force_cold_rebuild {
        let n = g.clear_all_descriptions().await?;
        println!("cleared {n} description(s) before re-index");
    }
    let embedder = config::embedder_from_cfg(&cfg);
    // ... LSP setup unchanged ...
    let indexer = Indexer { graph: g, lsp, embedder };
    let n = indexer.index_repo(&path).await?;
    println!("indexed {n} files");
}
```

- [ ] **Step 5: Build, test, clippy**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-cli/src/cli.rs crates/mycel-cli/src/main.rs crates/mycel-graph/src/symbol.rs crates/mycel-graph/tests/integration.rs
git commit -m "Add mycel index --force-cold-rebuild flag"
```

### Task 3.5: Add `mycel skill install`

**Files:**
- Modify: `crates/mycel-cli/src/cli.rs` (add `Skill` subcommand group)
- Create: `crates/mycel-cli/src/skill_install.rs`
- Modify: `crates/mycel-cli/src/main.rs` (handle `Skill`)
- Test: `crates/mycel-cli/src/skill_install.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing unit test**

Create `crates/mycel-cli/src/skill_install.rs`:

```rust
//! `mycel skill install` — symlink <repo>/skills/mycel-graph-care into
//! ~/.claude/skills/mycel-graph-care. Idempotent. Refuses to overwrite
//! a non-symlink at the target.

use anyhow::{bail, Context, Result};
use camino::{Utf8Path, Utf8PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub source: Utf8PathBuf,
    pub target: Utf8PathBuf,
    /// True if a symlink was created or already existed pointing at `source`.
    /// False is impossible — errors are surfaced as `Err`.
    pub linked: bool,
}

/// Compute the source skill directory inside a repo.
pub fn source_for_repo(repo: &Utf8Path) -> Utf8PathBuf {
    repo.join("skills").join("mycel-graph-care")
}

/// Compute the user-skills target directory under HOME.
pub fn default_target() -> Result<Utf8PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let home: Utf8PathBuf = home.try_into().context("HOME not utf-8")?;
    Ok(home.join(".claude").join("skills").join("mycel-graph-care"))
}

/// Install the skill: create the symlink target's parent if needed, then
/// create a symlink from `target` to `source`. Idempotent.
pub fn install(source: &Utf8Path, target: &Utf8Path) -> Result<InstallOutcome> {
    if !source.exists() {
        bail!("source does not exist: {source}");
    }
    let target_path = target.as_std_path();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent.as_std_path())
            .with_context(|| format!("create parent dir {parent}"))?;
    }
    if target_path.exists() || target_path.is_symlink() {
        // Idempotent path: already a symlink pointing at the right source?
        if target_path.is_symlink() {
            let dest = std::fs::read_link(target_path)
                .with_context(|| format!("read symlink {target}"))?;
            let dest_canon = dest.canonicalize().unwrap_or(dest);
            let source_canon = source.as_std_path().canonicalize()
                .with_context(|| format!("canonicalize source {source}"))?;
            if dest_canon == source_canon {
                return Ok(InstallOutcome {
                    source: source.to_path_buf(),
                    target: target.to_path_buf(),
                    linked: true,
                });
            }
            bail!(
                "{target} is already a symlink, but points elsewhere ({dest:?}). \
                 Remove it manually and retry."
            );
        }
        bail!(
            "{target} exists and is not a symlink. \
             Remove it manually and retry."
        );
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(source.as_std_path(), target_path)
        .with_context(|| format!("symlink {source} -> {target}"))?;

    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(source.as_std_path(), target_path)
        .with_context(|| format!("symlink {source} -> {target}"))?;

    Ok(InstallOutcome {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        linked: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> Utf8PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mycel-skill-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Utf8PathBuf::from_path_buf(p).unwrap()
    }

    #[test]
    fn install_creates_symlink() {
        let dir = tmp();
        let source = dir.join("src/skill");
        std::fs::create_dir_all(source.as_std_path()).unwrap();
        let target = dir.join("home/.claude/skills/x");

        let outcome = install(&source, &target).unwrap();
        assert!(outcome.linked);
        assert!(target.as_std_path().is_symlink());
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tmp();
        let source = dir.join("src/skill");
        std::fs::create_dir_all(source.as_std_path()).unwrap();
        let target = dir.join("home/.claude/skills/x");

        install(&source, &target).unwrap();
        install(&source, &target).unwrap();  // second call: no-op success
    }

    #[test]
    fn install_refuses_to_overwrite_existing_file() {
        let dir = tmp();
        let source = dir.join("src/skill");
        std::fs::create_dir_all(source.as_std_path()).unwrap();
        let target = dir.join("home/.claude/skills/x");
        std::fs::create_dir_all(target.parent().unwrap().as_std_path()).unwrap();
        std::fs::write(target.as_std_path(), "user data").unwrap();

        let err = install(&source, &target).unwrap_err();
        assert!(err.to_string().contains("not a symlink"));
    }

    #[test]
    fn install_refuses_to_overwrite_wrong_symlink() {
        let dir = tmp();
        let real_source = dir.join("src/skill");
        std::fs::create_dir_all(real_source.as_std_path()).unwrap();
        let other = dir.join("src/other");
        std::fs::create_dir_all(other.as_std_path()).unwrap();
        let target = dir.join("home/.claude/skills/x");
        std::fs::create_dir_all(target.parent().unwrap().as_std_path()).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(other.as_std_path(), target.as_std_path()).unwrap();

        let err = install(&real_source, &target).unwrap_err();
        assert!(err.to_string().contains("points elsewhere"));
    }
}
```

- [ ] **Step 2: Wire the module and run tests**

(The TDD "verify the test fails first" phase isn't meaningful here — the module file doesn't compile until it's wired into the crate, so the only state where tests "fail" is "doesn't compile." Wire the module first, then watch the tests pass.)

Add `mod skill_install;` to `crates/mycel-cli/src/main.rs`.

Run: `cargo test -p mycel-cli --lib skill_install`
Expected: PASS — the four unit tests use only filesystem ops, no FalkorDB.

- [ ] **Step 3: Add the `Skill` subcommand**

In `crates/mycel-cli/src/cli.rs`:

```rust
/// Manage Claude Code skills shipped with Mycelium.
Skill { #[command(subcommand)] action: SkillAction },
```

```rust
#[derive(Subcommand)]
pub enum SkillAction {
    /// Symlink <repo>/skills/mycel-graph-care into ~/.claude/skills/.
    /// Idempotent. Run from a Mycelium repo checkout.
    Install,
}
```

In `crates/mycel-cli/src/main.rs`:

```rust
Cmd::Skill { action } => match action {
    cli::SkillAction::Install => {
        let repo = cli.repo.clone().unwrap_or_else(|| ".".into());
        let source = skill_install::source_for_repo(&repo);
        let target = skill_install::default_target()?;
        let outcome = skill_install::install(&source, &target)?;
        println!("installed: {} -> {}", outcome.target, outcome.source);
    }
},
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mycel-cli`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-cli
git commit -m "Add mycel skill install command"
```

### Task 3.6: Remove the `--no-descriptions` flag

The flag is no longer meaningful — synthesis is no longer part of indexing. Removing rather than deprecating because Mycelium is pre-1.0 and we explicitly avoid backwards-compat shims (per CLAUDE.md).

**Files:**
- Modify: `crates/mycel-cli/src/cli.rs` (Index variant — already touched in Task 3.4; this confirms the flag is gone)
- Verify: `rg -n 'no_descriptions|no-descriptions' crates/` returns no live references

- [ ] **Step 1: Confirm removal**

Run: `rg -n 'no_descriptions|no-descriptions' crates/`
Expected: no matches (Task 3.4 already removed the field; this step is verification).

- [ ] **Step 2: Update CLAUDE.md if it references the flag**

Run: `rg -n 'no-descriptions' CLAUDE.md`
- If it appears, edit to either remove the line or replace with the new `--force-cold-rebuild` semantics. (The full CLAUDE.md update lands in Chunk 5; only fix obvious contradictions here.)

- [ ] **Step 3: Commit (only if anything changed)**

```bash
git add CLAUDE.md
git commit -m "Drop --no-descriptions references (flag removed)"
```

### Task 3.7: Chunk 3 verification

- [ ] `cargo test --workspace` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Manual smoke: from this repo, run `target/release/mycel --repo . definers Indexer --json | head -5`. Take a qname. Run `target/release/mycel --repo . describe <qname>` — expect `(none)` (since cold index doesn't write descriptions). Run `target/release/mycel --repo . set-description --qname <qname> --description "Test desc."`. Re-run `describe` — expect "Test desc.".
- [ ] Run `target/release/mycel --repo . skill install` — expect `~/.claude/skills/mycel-graph-care` to become a symlink at the repo's `skills/mycel-graph-care`. (Skill file itself doesn't exist yet — that's Chunk 5.) **Note**: this leaves a persistent symlink under `~/.claude/skills/`. If you're testing in a sandbox, clean up afterwards: `rm ~/.claude/skills/mycel-graph-care`. If you want this skill installed for real, leave it.

---

## Chunk 4: Daemon staleness handling

The daemon's incremental path needs to detect when a Symbol's body has changed since its description was written and clear the stale description. Because `index_file_collect_edges` is shared between `mycel index` and the daemon, the staleness check belongs there.

### Task 4.1: Pre-pass that clears descriptions whose source_hash diverges

**Files:**
- Modify: `crates/mycel-index/src/pipeline.rs` (in `index_file_collect_edges`, between symbol upsert and embedding)
- Test: `crates/mycel-graph/tests/integration.rs`

The flow becomes:
1. Parse and extract Symbols with their new body slices.
2. Upsert Symbols (sets new file_path/start_line/end_line, but does NOT touch body_hash since extractor-built Symbols carry None).
3. **NEW**: For each updated Symbol, compute the new body_hash from the new body slice. If the stored `description_source_hash != new_body_hash`, call `clear_symbol_description_and_reembed` — which atomically clears description, source_hash, and updates embedding + body_hash.
4. Otherwise, fall through to the existing embed loop and write the new embedding + body_hash via `set_symbol_embedding_and_body_hash`.

This keeps the staleness check colocated with the data that triggers it (the new body slice).

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-graph/tests/integration.rs`:

```rust
#[tokio::test]
async fn incremental_index_clears_stale_description() {
    let client = GraphClient::connect(&url(), "mycel:test:stale_clear").await.unwrap();

    struct StubEmbedder;
    #[async_trait::async_trait]
    impl mycel_models::Embedder for StubEmbedder {
        fn identity(&self) -> &str { "stub/test" }
        fn dimension(&self) -> u32 { 768 }
        async fn embed(&self, texts: &[&str]) -> mycel_core::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.1; 768]).collect())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("staleable.rs");
    let v1 = "fn staleable() { 1 }\n";
    std::fs::write(&f, v1).unwrap();
    let f_utf8 = camino::Utf8PathBuf::from_path_buf(f.clone()).unwrap();

    let indexer = mycel_index::Indexer {
        graph: client.clone(), lsp: None,
        embedder: std::sync::Arc::new(StubEmbedder),
    };
    indexer.index_file(&f_utf8, v1).await.unwrap();

    // Find the symbol's qname
    let definers = client.query_definers("staleable").await.unwrap();
    let qname_v1 = definers.first().expect("indexed").qualified_name.clone();

    // Write a description against v1 body
    client.set_symbol_description_and_embedding(
        qname_v1.as_str(), "Returns 1.", &vec![0.5; 768],
    ).await.unwrap();

    // Edit the file: body changes substantially
    let v2 = "fn staleable() {\n  let x = 99;\n  x * 2\n}\n";
    std::fs::write(&f, v2).unwrap();
    indexer.index_file(&f_utf8, v2).await.unwrap();

    // Defensive: qname should be stable across the reindex even though the
    // body shape changed. If a future extractor change mangles qnames on
    // body shape this assertion will catch it before the staleness check.
    let definers_after = client.query_definers("staleable").await.unwrap();
    let qname_v2 = &definers_after.first().expect("indexed v2").qualified_name;
    assert_eq!(qname_v2, &qname_v1, "qname stable across reindex");

    // Description should be cleared by the staleness pre-pass
    let info = client.get_symbol_description(qname_v1.as_str()).await.unwrap().unwrap();
    assert_eq!(info.description, None,
        "description must be cleared when body_hash diverges from description_source_hash");
    assert_eq!(info.description_source_hash, None);
    assert!(info.body_hash.is_some(), "fresh body_hash written");
}

#[tokio::test]
async fn incremental_index_preserves_description_when_body_slice_unchanged() {
    // Verify the staleness pre-pass DOES NOT clear a description when the
    // file content_hash differs but the Symbol's body slice is byte-identical
    // (e.g. a comment edit at the top of the file). This forces the dedup
    // early-return to NOT short-circuit, so the embed/staleness loop actually
    // runs — without that, the test would pass trivially.
    let client = GraphClient::connect(&url(), "mycel:test:preserve").await.unwrap();

    struct StubEmbedder;
    #[async_trait::async_trait]
    impl mycel_models::Embedder for StubEmbedder {
        fn identity(&self) -> &str { "stub/test" }
        fn dimension(&self) -> u32 { 768 }
        async fn embed(&self, texts: &[&str]) -> mycel_core::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.1; 768]).collect())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("stable.rs");
    // v1: function on lines 1-3
    let v1 = "fn stable() {\n    7\n}\n";
    std::fs::write(&f, v1).unwrap();
    let f_utf8 = camino::Utf8PathBuf::from_path_buf(f.clone()).unwrap();

    let indexer = mycel_index::Indexer {
        graph: client.clone(), lsp: None,
        embedder: std::sync::Arc::new(StubEmbedder),
    };
    indexer.index_file(&f_utf8, v1).await.unwrap();

    let definers = client.query_definers("stable").await.unwrap();
    let qname_v1 = definers.first().expect("indexed").qualified_name.clone();
    client.set_symbol_description_and_embedding(
        qname_v1.as_str(), "Returns 7.", &vec![0.5; 768],
    ).await.unwrap();

    // v2: identical function body, but the file content differs — a comment
    // line is appended below the function. content_hash changes (so dedup
    // does NOT skip), but the function's signature+body slice (lines 1-3)
    // is byte-identical, so body_hash is unchanged and the description is
    // not stale.
    let v2 = "fn stable() {\n    7\n}\n// added trailing comment\n";
    std::fs::write(&f, v2).unwrap();
    indexer.index_file(&f_utf8, v2).await.unwrap();

    // The function's qname should be stable across the reindex (defensive
    // assertion against future extractor changes that mangle qnames on
    // surrounding-content shape).
    let definers_after = client.query_definers("stable").await.unwrap();
    let qname_v2 = &definers_after.first().expect("indexed v2").qualified_name;
    assert_eq!(qname_v2, &qname_v1, "qname stable across reindex");

    let info = client.get_symbol_description(qname_v1.as_str()).await.unwrap().unwrap();
    assert_eq!(info.description.as_deref(), Some("Returns 7."),
        "description preserved when body slice unchanged");
    assert!(info.description_source_hash.is_some(),
        "source_hash preserved (still matches body_hash)");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p mycel-graph --test integration incremental_index_clears_stale_description incremental_index_preserves_description_when_body_unchanged`
Expected: FAIL — `incremental_index_clears_stale_description` fails because no staleness check exists; the second may pass already.

- [ ] **Step 3: Replace the embed loop with a staleness-aware variant**

> **Anchor**: this fully replaces the embed loop introduced in Chunk 2 Task 2.2 Step 2. Apply this diff against the version of `pipeline.rs` produced by Chunk 2, not against the original Phase 1/2-shipped pipeline. The shape is the same (chunk-of-32 embed batch); the behavior change is interleaving a single batched staleness read in front of the per-Symbol write.
>
> Why batched: the read fetches `description_source_hash` for *every* Symbol in the file in a single Cypher round-trip via `description_source_hashes_for_batch` (Task 1.4b). A naïve per-Symbol read would issue ≥1 round-trip per Symbol, which on a 50k-symbol cold rebuild would dominate the indexer's wall-clock — exactly the regression this whole revision is designed to avoid.

In `crates/mycel-index/src/pipeline.rs`, replace the embed loop (the `if !extraction.symbols.is_empty() { ... }` block from Chunk 2 Task 2.2) with:

```rust
// 5. Embed signature + body slice; handle staleness via a single batched
//    pre-read.
//
//    Sequence:
//      a) Compute (text, hash) for every Symbol's body slice.
//      b) Single Cypher round-trip: fetch description_source_hash for
//         this file's Symbols.
//      c) Batched embed (chunk-of-32, mirroring the cold-index shape).
//      d) Per-Symbol write: if (b) returned a hash AND it diverges from
//         the new body_hash, clear+reembed; otherwise just write
//         embedding+body_hash.
if !extraction.symbols.is_empty() {
    use crate::body_slice::{signature_plus_body_slice, FileCache};
    let mut cache = FileCache::new();
    // (a) Per-Symbol (embed text, body_hash) pairs.
    let prepared: Vec<(String, String)> = extraction
        .symbols.iter()
        .map(|s| {
            let bs = signature_plus_body_slice(
                &mut cache, s.signature.as_str(),
                s.file_path.as_str(), s.start_line, s.end_line,
            );
            (bs.text, bs.hash)
        })
        .collect();

    // (b) One Cypher read for the whole file's Symbols.
    let qnames: Vec<&str> = extraction.symbols.iter()
        .map(|s| s.qualified_name.as_str())
        .collect();
    let stored_source_hashes = self.graph.description_source_hashes_for_batch(&qnames).await?;

    // (c) Batched embed.
    let mut all_vecs: Vec<Vec<f32>> = Vec::with_capacity(prepared.len());
    for (sym_chunk, prep_chunk) in extraction.symbols.chunks(32).zip(prepared.chunks(32)) {
        let text_chunk: Vec<&str> = prep_chunk.iter().map(|(t, _)| t.as_str()).collect();
        let vecs = self.embedder.embed(&text_chunk).await?;
        if vecs.len() != sym_chunk.len() {
            return Err(MycelError::Model {
                provider: self.embedder.identity().into(),
                message: format!("expected {} embeddings, got {}", sym_chunk.len(), vecs.len()),
            });
        }
        all_vecs.extend(vecs);
    }
    debug_assert_eq!(all_vecs.len(), prepared.len());

    // (d) Per-Symbol write — clear-and-reembed on staleness, otherwise plain write.
    for ((sym, (_, new_hash)), vec) in extraction.symbols.iter().zip(prepared.iter()).zip(all_vecs.iter()) {
        let stale = stored_source_hashes
            .get(sym.qualified_name.as_str())
            .is_some_and(|src| src.as_str() != new_hash.as_str());
        if stale {
            self.graph.clear_symbol_description_and_reembed(
                sym.qualified_name.as_str(), new_hash, vec,
            ).await?;
        } else {
            self.graph.set_symbol_embedding_and_body_hash(
                sym.qualified_name.as_str(), vec, new_hash,
            ).await?;
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mycel-graph --test integration incremental_index_clears_stale_description incremental_index_preserves_description_when_body_unchanged`
Expected: both PASS.

Run full suite: `cargo test --workspace`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-index/src/pipeline.rs crates/mycel-graph/tests/integration.rs
git commit -m "Clear stale descriptions when body_hash diverges on incremental index"
```

### Task 4.2: Daemon path verification

The daemon calls `index_file` on each notify event, which calls `index_file_collect_edges`, which now contains the staleness logic. No daemon-side code change is needed.

- [ ] **Step 1: Verify the daemon picks up the change**

Run: `cargo build --release -p mycel-daemon`
Expected: clean.

- [ ] **Step 2: End-to-end manual verification (optional but recommended)**

Requires Task 3.2 (`mycel set-description`) to be implemented. If you're executing chunks in order this is true by the time Chunk 4 runs.

Restart the daemon to pick up the rebuilt binary: `target/release/mycel daemon stop && target/release/mycel daemon start`. Tail logs: `tail -f ~/.cache/mycel/daemon.log` in another shell.

- Pick a function in this repo (e.g. `mycel-index`'s `Indexer::index_file`). Don't hardcode the qname — get it from `mycel definers`.
- Get its qname: `target/release/mycel --repo . definers index_file --json` and copy the `qualified_name` field from the result.
- Write a description: `target/release/mycel --repo . set-description --qname <qname> --description "Test."`.
- Edit the function body (add a comment line, save).
- Wait 2-5 seconds for the watcher debounce + reindex.
- Run: `target/release/mycel --repo . describe <qname>`.
- Expected: `(none)` (description was cleared by the staleness pre-pass).

This is a manual verification step — it's documented for confidence but isn't part of the automated test suite.

- [ ] **Step 3: Commit (no code change; this task is verification only)**

No commit if no files changed.

### Task 4.3: Chunk 4 verification

- [ ] All Chunk 4 integration tests pass.
- [ ] `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] Optional manual end-to-end verification per Task 4.2.

---

## Chunk 5: Skill + CLAUDE.md updates

Ship the actual skill file Claude reads, and update CLAUDE.md so the new commands are discoverable.

### Task 5.1: Create `skills/mycel-graph-care/SKILL.md`

**Files:**
- Create: `skills/mycel-graph-care/SKILL.md`

- [ ] **Step 1: Create the directory and write the skill**

```bash
mkdir -p skills/mycel-graph-care
```

The body below is an expansion of the spec's §"Skill body draft" (in `docs/superpowers/specs/2026-05-05-workload-driven-synthesis-design.md`). Two sections are new vs the spec — *Examples* and *What to do if a command fails* — both consistent with the spec's note that "final wording lives in the skill file." A reviewer comparing spec-to-plan will notice this expansion; that's intentional.

Write `skills/mycel-graph-care/SKILL.md`:

```markdown
---
name: mycel-graph-care
description: Use after reading and reasoning about a function's behavior to answer the user's question — write a 1-3 sentence behavioral description back to the Mycelium graph so future searches cluster on behavior. Conservative trigger; do not synthesize while skimming, do not synthesize trivial code.
---

# Tend the Mycelium graph as you work

Mycelium watches this repo and indexes every Symbol with a fast embedding
of its signature + body. That gets you Cursor-grade semantic search for
free. The behavioral-search advantage — clustering on what code DOES rather
than what it's NAMED — only shows up once Symbols have synthesized
descriptions. Cold indexing doesn't write descriptions to keep indexing
fast. You write them as a side-effect of normal work.

## When to write a description (conservative trigger)

Write only when ALL of these are true:

- You read the symbol's body via `Read` or via `mycel` output
- You used your understanding to make a decision, an explanation, or a code change
- The symbol is non-trivial: more than ~10 lines, or genuinely complex single-line logic

## When NOT to write

- You skimmed a file looking for an unrelated bug — don't synthesize what you didn't think about
- The symbol is a getter, setter, constant, simple struct field, or generated code
- The symbol is a `mod foo;` declaration (Rust module decls hallucinate badly)
- The symbol is in `tests/`, `examples/`, or a `*_test.*` file
- A description already exists AND your understanding matches it (call `mycel describe` first)

## How to write

1. Get the canonical qualified name:

   ```sh
   target/release/mycel --repo . definers <name> --json
   ```

   Take the `qualified_name` field from the result.

2. Check current state:

   ```sh
   target/release/mycel --repo . describe <qname>
   ```

   If it prints something and your understanding matches, skip. If it differs
   materially, overwrite. If it prints `(none)`, write a fresh description.

3. Write the description:

   ```sh
   target/release/mycel --repo . set-description \
     --qname <qname> \
     --description "<1-3 sentences>"
   ```

## Description format

- 1-3 sentences. Behavioral, not syntactic.
- Don't restate the signature ("takes X, returns Y") — the signature is already there.
- Don't start with "This function..." — get to the verb.
- Focus on what the symbol does, why it exists, what role it plays.
- No code blocks, no markdown.

## Examples

Good:

> Atomically writes a Symbol's synthesized description and the embedding
> derived from it, stamping the description's source body hash so the
> daemon can detect staleness on later edits.

Good:

> Picks the default synthesizer model from the configured tier — gemma4:e2b
> on minimal, gemma4:e4b on balanced, qwen3.6:35b-a3b on max. Auto-detects
> tier from /proc/meminfo when not pinned.

Bad ("restates the signature"):

> Takes a qname, description, and embedding, writes them to the graph, returns Result<()>.

Bad ("vague"):

> Handles description writes for the indexer.

## What to do if a command fails

- `mycel definers <name>` returns nothing — the symbol may not be indexed
  (file just edited and daemon hasn't caught up); wait 5 seconds and retry,
  or fall back to grepping the source.
- `mycel set-description` exits non-zero with "no Symbol with qualified_name" —
  the qname is wrong. Re-fetch it from `mycel definers <name> --json`.
- `mycel set-description` exits non-zero with an embedder error — Ollama
  likely isn't running. Confirm with `curl http://localhost:11434/api/tags`
  and retry. Don't ask the user to restart Ollama unless they explicitly
  asked you to handle errors.
```

- [ ] **Step 2: Commit**

```bash
git add skills/mycel-graph-care/SKILL.md
git commit -m "Ship mycel-graph-care skill"
```

### Task 5.2: Update CLAUDE.md to reflect the new commands and Phase 2 policy

**Files:**
- Modify: `CLAUDE.md`

CLAUDE.md has four sections that need touching, each at a specific line range as of planning time. Open the file and confirm line numbers haven't drifted (`rg -n '^##' CLAUDE.md` shows section anchors); if they have, find by content.

- [ ] **Step 1: Update the commands table (lines 13–20)**

The table currently has six rows. Three rows need *modification* and four new rows need *insertion*. Concretely:

- **Modify line 16** (the "Semantic exploration" row). Currently reads:
  > `| Semantic exploration | ...mycel find... | **Improving** — Phase 2 lands description-based embeddings; quality jumps as 'mycel synthesize' runs across the graph |`
  Replace the Status cell with:
  > `**Works baseline** — embeds signature+body slice on cold index; quality lifts further as the mycel-graph-care skill writes descriptions for symbols you read.`

- **Modify line 17** (the "Run Phase 2 description synthesis" row). Currently reads:
  > `| Run Phase 2 description synthesis | mycel synthesize [--force] [--limit N] | **Works** — synthesizes a behavioral description per Symbol via Ollama, then re-embeds. Idempotent. |`
  Replace with:
  > `| Run bulk description synthesis (manual / non-Claude path) | target/release/mycel --repo . synthesize [--force] [--limit N] | **Works** — opt-in bulk pass via Ollama. Not run by mycel index after the 2026-05-05 redirection. |`

- **Insert** the following five rows immediately after the modified line 17, in this order (they slot logically between the manual-synthesis row and the broken-edge rows):

  | Task | Command | Status |
  |------|---------|--------|
  | Read a Symbol's current description | `target/release/mycel --repo . describe <qname>` | **Works** |
  | Write a behavioral description (workload-driven) | `target/release/mycel --repo . set-description --qname <qname> --description "<text>"` | **Works** — invoked by the `mycel-graph-care` skill |
  | Install the graph-care skill into Claude Code | `target/release/mycel --repo . skill install` | **Works** |
  | Backfill legacy description hashes | `target/release/mycel --repo . synthesize --refresh-hashes-only` | **Works** — one-shot for graphs predating 2026-05-05 |
  | Re-index from scratch (drop legacy descriptions) | `target/release/mycel --repo . index . --force-cold-rebuild` | **Works** |

- [ ] **Step 2: Replace the Phase 2 policy paragraph (line 24)**

Line 24 currently reads (single paragraph):

> `mycel index runs the description-synthesis pass automatically at the end on a configured synthesizer (MYCEL_SYNTHESIZER=off to disable, --no-descriptions flag for a fast cold reindex). The daemon's incremental path always skips synth — per-file edits don't pay an LLM call apiece. After daemon-driven reindexes, run mycel synthesize to refresh descriptions across the graph.`

Replace the entire paragraph (line 24) with:

```markdown
`mycel index` does NOT run description synthesis (post-2026-05-05 redirection). Cold-index embeddings are computed on `signature + body[:60 lines]` — fast, no LLM round-trips. Behavioral descriptions are written by Claude Code itself when you've read and reasoned about a function, via `mycel set-description`, instructed by the shipped `mycel-graph-care` skill. The bulk `mycel synthesize` command remains as a manual fallback for non-Claude workflows. When the daemon's watcher detects an edit that changes a Symbol's body slice, the corresponding stored description (if any) is cleared automatically and the symbol is re-embedded on the new signature+body — Claude refills the description next time it reads and understands the function.
```

- [ ] **Step 3: Update the Phase 1/2 limitations section (lines 26–43)**

Two bullets need adjustment:

- **The first bullet** (line ~30, starting `**find quality depends on description coverage.**`) currently says "a freshly indexed graph has signature-embedded symbols until `mycel synthesize` runs". Replace that sentence with: "a freshly indexed graph has signature+body-slice-embedded symbols until Claude (via the `mycel-graph-care` skill) writes behavioral descriptions for the symbols you actually work with — coverage grows with use, not with a one-shot bulk command."

- **The second bullet** (line ~31, "Module-declaration hallucinations") still applies — `mycel synthesize` still has the same hallucination behavior on module declarations when run as a bulk pass. Add a leading parenthetical: "(Applies to the bulk `mycel synthesize` path; the workload-driven skill instructs Claude to skip module decls.)"

The other three bullets (HNSW low-k, broken Tier-1 edges, cross-file CALLS) are independent of this revision; leave them as-is.

- [ ] **Step 4: Update the Phase plan section (lines ~70–77)**

The Phase 2 line currently reads:

> `**Phase 2** — synthesizer wired (gemma4:e2b/e4b/qwen3.6:35b-a3b), description generation with 1-hop graph context, re-embed on descriptions. **Shipped 2026-05-04.** Run mycel synthesize to upgrade an existing graph from signature- to description-embeddings.`

Replace with:

> `**Phase 2** — workload-driven description synthesis. Cold index embeds signature+body slice; behavioral descriptions are written by Claude via the mycel-graph-care skill (set-description CLI). Bulk mycel synthesize remains as opt-in fallback. **Shipped 2026-05-04**, redirected 2026-05-05 (see docs/superpowers/specs/2026-05-05-workload-driven-synthesis-design.md).`

- [ ] **Step 5: Final verification**

Run: `rg -n 'no-descriptions' CLAUDE.md`
Expected: zero matches.

Run: `rg -n 'mycel synthesize' CLAUDE.md`
Expected: only the table row mentioning the manual / non-Claude path, the limitations parenthetical, and the Phase 2 entry — no remaining "run synthesize to upgrade" framing.

Run: `cargo build --workspace`
Expected: clean.

Run: `target/release/mycel --help`
Expected: subcommand list includes `describe`, `set-description`, `skill`; `index` shows `--force-cold-rebuild` but not `--no-descriptions`.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md
git commit -m "Update CLAUDE.md for workload-driven synthesis"
```

### Task 5.3: Final integration smoke test

- [ ] **Step 1: Cold index this repo from scratch**

```bash
target/release/mycel --repo . index . --force-cold-rebuild
```

Expected: completes in under 5 minutes; output reports cleared descriptions and indexed file count.

- [ ] **Step 2: Verify cold-index state**

```bash
target/release/mycel --repo . describe <some_qname>
```

Expected: `(none)` for any Symbol — cold index writes no descriptions.

- [ ] **Step 3: Install the skill and write a test description**

```bash
target/release/mycel --repo . skill install
ls -la ~/.claude/skills/mycel-graph-care
```

Expected: symlink pointing into this repo.

```bash
target/release/mycel --repo . definers Indexer --json | head
# Take the qualified_name from the result. Don't hardcode it — the
# extractor's qname format may include module paths or generic
# parameters in ways that vary across releases. Substitute below.
QNAME=$(target/release/mycel --repo . definers Indexer --json | jq -r '.[0].qualified_name')
target/release/mycel --repo . set-description \
  --qname "$QNAME" \
  --description "Holds the graph, embedder, and optional LSP for the indexing pipeline. Owned by the daemon for incremental updates and by the CLI for cold builds."
target/release/mycel --repo . describe "$QNAME"
```

Expected: the description prints back.

- [ ] **Step 4: Verify staleness invalidation**

Edit `crates/mycel-index/src/pipeline.rs` — add a comment line inside the `Indexer` struct definition. Save.

Wait ~5 seconds (daemon debounce + indexing). Then re-derive the qname (don't reuse `$QNAME` from Step 3 — shell variables won't carry across if you ran Step 3 in a different shell, and the qname could shift if the comment edit moved the struct's line numbers in a way the extractor cares about):

```bash
QNAME=$(target/release/mycel --repo . definers Indexer --json | jq -r '.[0].qualified_name')
target/release/mycel --repo . describe "$QNAME"
```

Expected: `(none)` — the description was cleared because the body_hash diverged.

(If the daemon isn't running, run `target/release/mycel --repo . index .` to reproduce the same effect via cold index.)

- [ ] **Step 5: Final test sweep**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green.

---

## Wrap-up

- All five chunks committed in order on the working branch.
- Spec at `docs/superpowers/specs/2026-05-05-workload-driven-synthesis-design.md` is the source of truth for design decisions; this plan is the implementation contract.
- DESIGN.md and CLAUDE.md both reflect the new policy.
- The skill ships at `skills/mycel-graph-care/SKILL.md` and is installable via `mycel skill install`.
- Cold-index time on this repo drops from ~30 minutes to under 5; descriptions accrue over time as Claude works in the codebase.
