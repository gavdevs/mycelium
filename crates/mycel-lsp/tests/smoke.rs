//! Requires multilspy installed and tsserver available. Set MYCEL_TEST_LSP=1 to run.

use camino::Utf8PathBuf;
use mycel_core::*;
use mycel_lsp::*;

#[tokio::test]
async fn lsp_smoke_typescript() {
    if std::env::var("MYCEL_TEST_LSP").ok().as_deref() != Some("1") {
        eprintln!("skipping (set MYCEL_TEST_LSP=1 to run)");
        return;
    }
    let repo: Utf8PathBuf = std::env::current_dir().unwrap().try_into().unwrap();
    let resolver = MultilspyResolver::spawn(
        "python3 scripts/multilspy_bridge.py",
        repo.clone(),
    ).await.expect("spawn multilspy");
    let path: Utf8PathBuf = "tests/fixtures/typescript/imports_and_exports.ts".into();
    let extraction = mycel_extract::ExtractionOutput::default();
    let edges = resolver.refine(&path, "typescript", &extraction).await
        .expect("refine returns Ok even when partial");
    eprintln!("got {} edges", edges.len());
    // imports_and_exports.ts has at least one definition (`add` from
    // ./simple_function) that LSP should resolve. If we get zero, the bridge
    // or multilspy is broken and the test should fail loudly.
    assert!(!edges.is_empty(), "expected at least one LSP edge from a TS fixture with cross-file references");
}
