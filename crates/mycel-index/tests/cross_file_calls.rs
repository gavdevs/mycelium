//! End-to-end: index a tiny TS project with a cross-file call and assert
//! the CALLS edge lands in the graph with both endpoints resolved to Symbol
//! qnames.
//!
//! This locks the post-Workstream-A behavior: tree-sitter emits a tentative
//! `<file>::greet -> add` edge (bare `add` callee), the LSP bridge resolves
//! the call site to `simple_function.ts:1`, and `symbol_containing` rewrites
//! the edge target to the full qname `simple_function.ts::add`. Without
//! Workstream A wiring this test fails because the dangling `add` callee
//! never matches a Symbol at upsert time.
//!
//! Requires:
//!   - MYCEL_TEST_LSP=1 (gates the network-y multilspy spawn)
//!   - FalkorDB on redis://127.0.0.1:16379
//!   - python3 with `multilspy` and a tsserver discoverable on PATH

use camino::{Utf8Path, Utf8PathBuf};
use mycel_core::{EdgeKind, Result};
use mycel_graph::GraphClient;
use mycel_lsp::MultilspyResolver;
use mycel_models::Embedder;
use std::sync::Arc;

const FIXTURE_SIMPLE: &str = "simple_function.ts";
const FIXTURE_CALLER: &str = "imports_and_exports.ts";
// imports_and_exports.ts also imports from this file; copy it too so tsserver
// can resolve every import in the project root and not bail out on missing
// modules.
const FIXTURE_TYPES: &str = "class_with_methods.ts";

/// Stub embedder so the test doesn't depend on a running Ollama. Mirrors the
/// pattern used in `crates/mycel-graph/tests/integration.rs`.
struct StubEmbedder;

#[async_trait::async_trait]
impl Embedder for StubEmbedder {
    fn identity(&self) -> &str {
        "stub/cross-file-test"
    }
    fn dimension(&self) -> u32 {
        768
    }
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0_f32; 768]).collect())
    }
}

#[tokio::test]
async fn cross_file_calls_land_in_graph() {
    if std::env::var("MYCEL_TEST_LSP").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping cross_file_calls_land_in_graph \
             (set MYCEL_TEST_LSP=1; requires multilspy + tsserver + FalkorDB)"
        );
        return;
    }

    // Source fixtures live at <workspace_root>/tests/fixtures/typescript/.
    // CARGO_MANIFEST_DIR is crates/mycel-index; ../.. lands at the workspace root.
    let workspace_root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .expect("workspace root resolvable from CARGO_MANIFEST_DIR");
    let src_dir = workspace_root.join("tests/fixtures/typescript");
    let bridge_script = workspace_root.join("scripts/multilspy_bridge.py");
    assert!(
        bridge_script.exists(),
        "multilspy bridge missing at {bridge_script}"
    );

    // Copy fixtures into a tempdir so the indexer's "repo" is hermetic — no
    // unrelated files for walkdir to crawl into and no risk of cross-test
    // contamination.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf())
        .expect("tempdir is utf8");
    for name in [FIXTURE_SIMPLE, FIXTURE_CALLER, FIXTURE_TYPES] {
        let src = src_dir.join(name);
        let dst = repo_root.join(name);
        std::fs::copy(&src, &dst).unwrap_or_else(|e| panic!("copy {src} -> {dst}: {e}"));
    }

    // Per-test graph, wiped before use so a prior run can't poison assertions.
    let graph_url = std::env::var("MYCEL_TEST_FALKORDB_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:16379".into());
    let client = GraphClient::connect(&graph_url, "mycel:test:cross_file_calls")
        .await
        .expect("connect FalkorDB");
    client
        .query("MATCH (n) DETACH DELETE n")
        .await
        .expect("wipe graph");

    let lsp = MultilspyResolver::spawn(
        &format!("python3 {bridge_script}"),
        repo_root.clone(),
    )
    .await
    .expect("multilspy bridge spawn");

    let indexer = mycel_index::Indexer {
        graph: client.clone(),
        lsp: Some(Arc::new(lsp)),
        embedder: Arc::new(StubEmbedder),
    };

    // Index files with repo-relative paths so symbol qnames stay short
    // (e.g. `simple_function.ts::add`) and `to_path` from the LSP bridge
    // (also repo-relative) lines up with stored `file_path`s — required for
    // `symbol_containing` to find the callee at edge-resolution time.
    //
    // We collect edges from both files and upsert them in a single batch at
    // the end, matching `index_repo`'s deferred-edge ordering — without this,
    // the caller file's CALLS edge would be written before `add` exists as a
    // Symbol and would silently drop.
    let mut all_edges = Vec::new();
    for name in [FIXTURE_SIMPLE, FIXTURE_CALLER, FIXTURE_TYPES] {
        let rel: Utf8PathBuf = name.into();
        let abs = repo_root.join(name);
        let content = std::fs::read_to_string(&abs).expect("read fixture");
        let edges = indexer
            .index_file_collect_edges(Utf8Path::new(&rel), &content)
            .await
            .expect("index_file_collect_edges")
            .expect("file was indexed (not dedup-skipped)");
        all_edges.extend(edges);
    }
    client
        .upsert_edge_batch(&all_edges)
        .await
        .expect("upsert deferred edges");

    // Sanity: at least one of the edges we just upserted is the cross-file
    // CALLS we care about — caller in imports_and_exports.ts, callee == add
    // qname in simple_function.ts.
    let cross_file_call_present = all_edges.iter().any(|e| {
        matches!(e.kind, EdgeKind::Calls)
            && e.to == "simple_function.ts::add"
            && e.from.starts_with("imports_and_exports.ts::")
    });
    assert!(
        cross_file_call_present,
        "expected a Calls edge from imports_and_exports.ts::* -> simple_function.ts::add; \
         all_edges = {all_edges:?}"
    );

    // The real Tier-1 contract: `query_callers` over the callee returns the
    // cross-file caller.
    let callers = client
        .query_callers("simple_function.ts::add")
        .await
        .expect("query_callers");
    let names: Vec<&str> = callers
        .iter()
        .map(|s| s.qualified_name.as_str())
        .collect();
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("imports_and_exports.ts::")),
        "expected a caller in imports_and_exports.ts for simple_function.ts::add; got {names:?}"
    );
}
