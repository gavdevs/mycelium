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
        body_hash: None,
        description_source_hash: None,
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
        body_hash: None,
        description_source_hash: None,
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
        body_hash: None,
        description_source_hash: None,
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

#[tokio::test]
async fn imports_uses_implements_queries() {
    let client = GraphClient::connect(&url(), "mycel:test:tier1").await.unwrap();
    let foo = Symbol {
        qualified_name: QualifiedName::new("ts::Foo"),
        kind: SymbolKind::Class,
        file_path: "src/foo.ts".into(),
        start_line: 1,
        end_line: 3,
        signature: Signature::new("class Foo"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: None,
        description_source_hash: None,
    };
    let i = Symbol {
        qualified_name: QualifiedName::new("ts::IFoo"),
        kind: SymbolKind::Interface,
        file_path: "src/foo.ts".into(),
        start_line: 5,
        end_line: 6,
        signature: Signature::new("interface IFoo"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: None,
        description_source_hash: None,
    };
    client.upsert_symbol(&foo).await.unwrap();
    client.upsert_symbol(&i).await.unwrap();
    client
        .upsert_edge_batch(&[Edge {
            from: "ts::Foo".into(),
            to: "ts::IFoo".into(),
            kind: EdgeKind::Implements,
            source: EdgeSource::Lsp,
        }])
        .await
        .unwrap();
    let impls = client.query_implements("ts::IFoo").await.unwrap();
    assert!(impls.iter().any(|s| s.qualified_name.as_str() == "ts::Foo"));
}

#[tokio::test]
async fn manifest_round_trip() {
    let client = GraphClient::connect(&url(), "mycel:test:manifest").await.unwrap();
    let m = IndexManifest {
        repo_id: RepoId::new("test-repo"),
        embedder_identity: "ollama/embeddinggemma".into(),
        embedder_dimension: 768,
        schema_version: SCHEMA_VERSION,
        last_indexed_at: time::OffsetDateTime::now_utc(),
    };
    client.write_manifest(&m).await.unwrap();
    let read = client.read_manifest("test-repo").await.unwrap().unwrap();
    assert_eq!(read.embedder_identity, m.embedder_identity);
    assert_eq!(read.embedder_dimension, 768);
}

#[tokio::test]
async fn upsert_symbol_writes_body_hash_when_set() {
    let client = GraphClient::connect(&url(), "mycel:test:body_hash").await.unwrap();
    let mut sym = Symbol {
        qualified_name: QualifiedName::new("crate::tests::with_hash"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn with_hash()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("abc123".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    let rows = client
        .query(
            "MATCH (s:Symbol {qualified_name: 'crate::tests::with_hash'}) \
             RETURN s.body_hash",
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
             RETURN s.body_hash",
        )
        .await
        .unwrap();
    match &rows[0][0] {
        falkordb::FalkorValue::String(s) => assert_eq!(
            s, "abc123",
            "None body_hash on upsert must not clobber existing value"
        ),
        other => panic!("expected String, got {other:?}"),
    }
}
