//! Requires multilspy installed and tsserver available. Set MYCEL_TEST_LSP=1 to run.

use camino::Utf8PathBuf;
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

#[tokio::test]
async fn lsp_resolve_refs_typescript() {
    if std::env::var("MYCEL_TEST_LSP").ok().as_deref() != Some("1") {
        eprintln!("skipping (set MYCEL_TEST_LSP=1; requires multilspy + tsserver)");
        return;
    }
    use mycel_lsp::protocol::RefSite;

    // Use the same fixture+repo-root pattern as `lsp_smoke_typescript`. The
    // call to `add(...)` lives in `tests/fixtures/typescript/imports_and_exports.ts`
    // at line 6, column 33 (0-indexed) — the `a` of `add(1, 2)` inside the
    // template literal:
    //   `    return \`Hi ${name}, sum is ${add(1, 2)}\`;`
    //                                       ^ col 33
    // It resolves to `tests/fixtures/typescript/simple_function.ts` line 1.
    let repo: Utf8PathBuf = std::env::current_dir().unwrap().try_into().unwrap();
    let resolver = MultilspyResolver::spawn(
        "python3 scripts/multilspy_bridge.py",
        repo.clone(),
    )
    .await
    .expect("bridge should spawn");

    let sites = vec![RefSite {
        line: 6,
        col: 33,
        kind: "calls".into(),
    }];
    let refs = resolver
        .resolve_refs(
            camino::Utf8Path::new("tests/fixtures/typescript/imports_and_exports.ts"),
            "typescript",
            sites,
        )
        .await
        .expect("resolve_refs should succeed");
    assert!(!refs.is_empty(), "expected at least one resolved ref; got {refs:?}");
    let first = &refs[0];
    assert!(
        first.to_path.contains("simple_function"),
        "expected cross-file resolution to simple_function.ts; got {first:?}",
    );
    assert!(first.to_line > 0);
}
