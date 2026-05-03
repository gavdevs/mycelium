use mycel_core::*;

#[test]
fn symbol_round_trip_json() {
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::foo::Bar"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 10,
        end_line: 20,
        signature: Signature::new("fn bar() -> u32"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
    };
    let json = serde_json::to_string(&sym).unwrap();
    let back: Symbol = serde_json::from_str(&json).unwrap();
    assert_eq!(sym, back);
}

#[test]
fn qualified_name_defangs_backticks() {
    // Backticks are Cypher's identifier delimiter. The constructor must
    // defang them so qualified names can be safely interpolated into
    // single-quoted Cypher string literals downstream.
    let q = QualifiedName::new("foo`bar");
    assert_eq!(q.as_str(), "foo_bar");

    // No-op fast path for inputs without backticks
    let q = QualifiedName::new("crate::module::Symbol");
    assert_eq!(q.as_str(), "crate::module::Symbol");
}

#[test]
fn edge_kind_serializes_lowercase() {
    let e = Edge {
        from: "a".into(),
        to: "b".into(),
        kind: EdgeKind::Calls,
        source: EdgeSource::Lsp,
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"calls\""));
    assert!(json.contains("\"lsp\""));
}
