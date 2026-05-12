use camino::Utf8PathBuf;
use mycel_extract::*;

fn extract_fixture(name: &str) -> ExtractionOutput {
    // Read content from an absolute path, but pass a stable relative path
    // (`tests/fixtures/typescript/<name>`) into the extractor so snapshots
    // don't bake in a machine-specific absolute path.
    let abs: Utf8PathBuf = format!(
        "{}/../../tests/fixtures/typescript/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
    .into();
    let content = std::fs::read_to_string(&abs).unwrap_or_else(|e| panic!("{abs}: {e}"));
    let rel: Utf8PathBuf = format!("tests/fixtures/typescript/{name}").into();
    let extractor = for_language(&rel).expect("ts extractor");
    extractor.extract(&rel, &content).unwrap()
}

#[test]
fn simple_function_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("simple_function.ts"));
}

#[test]
fn class_with_methods_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("class_with_methods.ts"));
}

#[test]
fn imports_and_exports_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("imports_and_exports.ts"));
}

#[test]
fn typescript_call_edge_carries_from_line() {
    let out = extract_fixture("imports_and_exports.ts");
    let call_edges: Vec<_> = out
        .edges
        .iter()
        .filter(|e| matches!(e.kind, mycel_core::EdgeKind::Calls))
        .collect();
    assert!(!call_edges.is_empty(), "fixture should produce >=1 CALL edge");
    for e in &call_edges {
        assert!(e.from_line.is_some(), "CALL edge {e:?} missing from_line");
        assert!(e.from_line.unwrap() > 0, "from_line should be 1-indexed");
    }
}
