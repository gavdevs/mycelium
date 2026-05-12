use camino::Utf8PathBuf;
use mycel_extract::*;

fn extract_fixture(name: &str) -> ExtractionOutput {
    // Read content from an absolute path, but pass a stable relative path
    // (`tests/fixtures/rust/<name>`) into the extractor so snapshots
    // don't bake in a machine-specific absolute path.
    let abs: Utf8PathBuf = format!(
        "{}/../../tests/fixtures/rust/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
    .into();
    let content = std::fs::read_to_string(&abs).unwrap_or_else(|e| panic!("{abs}: {e}"));
    let rel: Utf8PathBuf = format!("tests/fixtures/rust/{name}").into();
    let extractor = for_language(&rel).expect("rust extractor");
    extractor.extract(&rel, &content).unwrap()
}

#[test]
fn simple_module_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("simple_module.rs"));
}

#[test]
fn trait_and_impl_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("trait_and_impl.rs"));
}

#[test]
fn rust_call_edge_carries_from_line() {
    let out = extract_fixture("simple_module.rs");
    let call_edges: Vec<_> = out
        .edges
        .iter()
        .filter(|e| matches!(e.kind, mycel_core::EdgeKind::Calls))
        .collect();
    assert!(!call_edges.is_empty(), "fixture should produce >=1 CALL edge");
    // Pinning the exact line locks the `start_position().row + 1` conversion.
    // A `+ 0` regression would be caught here, where `> 0` alone would not.
    // The only call site in the fixture is `add(x, x)` inside `double` on line 6.
    for e in &call_edges {
        assert_eq!(e.from_line, Some(6), "edge {e:?}");
    }
}

#[test]
fn rust_implements_edge_carries_from_line() {
    let out = extract_fixture("trait_and_impl.rs");
    let impl_edges: Vec<_> = out
        .edges
        .iter()
        .filter(|e| matches!(e.kind, mycel_core::EdgeKind::Implements))
        .collect();
    assert!(
        !impl_edges.is_empty(),
        "fixture should produce >=1 IMPLEMENTS edge"
    );
    // Pinning the exact line locks the `start_position().row + 1` conversion.
    // A `+ 0` regression would be caught here, where `> 0` alone would not.
    // The only impl in the fixture is `impl Greeter for FormalGreeter` on line 9.
    for e in &impl_edges {
        assert_eq!(e.from_line, Some(9), "edge {e:?}");
    }
}
