//! These tests require a running FalkorDB at $MYCEL_TEST_FALKORDB_URL
//! (defaults to redis://127.0.0.1:16379, the sidecar test container).
//! Start it with `docker run -d --rm --name mycel-falkordb-test -p 127.0.0.1:16379:6379 -p 127.0.0.1:13000:3000 falkordb/falkordb:v4.18.3`.

use mycel_core::*;
use mycel_graph::*;

fn url() -> String {
    std::env::var("MYCEL_TEST_FALKORDB_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:16379".into())
}

#[tokio::test]
async fn connect_runs_migrations_idempotently() {
    let client = GraphClient::connect(&url(), "mycel:test:connect").await.unwrap();
    // Should be safe to connect twice in a row.
    let client2 = GraphClient::connect(&url(), "mycel:test:connect").await.unwrap();
    drop((client, client2));
}

#[tokio::test]
async fn migrations_record_in_meta_graph() {
    let client = GraphClient::connect(&url(), "mycel:test:meta").await.unwrap();
    let applied = client.applied_migrations().await.unwrap();
    assert!(applied.contains(&"v1_symbol_indices".to_string()));
    assert!(applied.contains(&"v1_vector_index".to_string()));
    assert!(applied.contains(&"v2_name_index".to_string()));
}

#[tokio::test]
async fn symbol_upsert_and_read() {
    let client = GraphClient::connect(&url(), "mycel:test:upsert").await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::foo::bar"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 10,
        signature: Signature::new("fn bar() -> u32"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    // Round-trip via a Tier 1-style read
    let definers = client.query_definers("bar").await.unwrap();
    assert!(
        definers
            .iter()
            .any(|s| s.qualified_name.as_str() == "crate::foo::bar"),
        "expected crate::foo::bar in definers, got {definers:?}"
    );
}

#[tokio::test]
async fn edge_upsert_callers_query() {
    let client = GraphClient::connect(&url(), "mycel:test:edges").await.unwrap();
    let foo = Symbol {
        qualified_name: QualifiedName::new("crate::foo"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 5,
        signature: Signature::new("fn foo()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
    };
    let bar = Symbol {
        qualified_name: QualifiedName::new("crate::bar"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 7,
        end_line: 12,
        signature: Signature::new("fn bar()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
    };
    client.upsert_symbol(&foo).await.unwrap();
    client.upsert_symbol(&bar).await.unwrap();
    client
        .upsert_edge_batch(&[Edge {
            from: "crate::foo".into(),
            to: "crate::bar".into(),
            kind: EdgeKind::Calls,
            source: EdgeSource::Lsp,
        }])
        .await
        .unwrap();
    let callers = client.query_callers("crate::bar").await.unwrap();
    assert!(
        callers
            .iter()
            .any(|s| s.qualified_name.as_str() == "crate::foo"),
        "expected crate::foo in callers, got {callers:?}"
    );
}
