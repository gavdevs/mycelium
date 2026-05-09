//! These tests require a running FalkorDB at $MYCEL_TEST_FALKORDB_URL
//! (defaults to redis://127.0.0.1:16379, the sidecar test container).
//! Start it with `docker run -d --rm --name mycel-falkordb-test -p 127.0.0.1:16379:6379 -p 127.0.0.1:13000:3000 falkordb/falkordb:v4.18.3`.

use mycel_core::*;
use mycel_graph::*;

fn url() -> String {
    std::env::var("MYCEL_TEST_FALKORDB_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:16379".into())
}

/// Connect to a per-test graph and wipe its contents so assertions aren't
/// poisoned by state from prior `cargo test` invocations against the same
/// FalkorDB container. Indices/vectors persist; node and edge data is reset.
async fn fresh_client(graph_name: &str) -> GraphClient {
    let client = GraphClient::connect(&url(), graph_name).await.unwrap();
    client.query("MATCH (n) DETACH DELETE n").await.unwrap();
    client
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
    let client = fresh_client("mycel:test:body_hash").await;
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

#[tokio::test]
async fn set_description_stamps_source_hash_from_body_hash() {
    let client = fresh_client("mycel:test:source_hash").await;
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::stamp"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn stamp()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("body-v1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    let dummy_vec = vec![0.1f32; 768];
    client
        .set_symbol_description_and_embedding("crate::stamp", "Stamps a thing.", &dummy_vec)
        .await
        .unwrap();

    let rows = client
        .query(
            "MATCH (s:Symbol {qualified_name: 'crate::stamp'}) \
             RETURN s.synthesized_description, s.description_source_hash",
        )
        .await
        .unwrap();
    let row = &rows[0];
    match &row[0] {
        falkordb::FalkorValue::String(s) => assert_eq!(s, "Stamps a thing."),
        other => panic!("desc: {other:?}"),
    }
    match &row[1] {
        falkordb::FalkorValue::String(s) => assert_eq!(
            s, "body-v1",
            "description_source_hash must mirror body_hash at write time"
        ),
        other => panic!("source_hash: {other:?}"),
    }
}

#[tokio::test]
async fn get_symbol_description_returns_description_and_hashes() {
    let client = fresh_client("mycel:test:get_desc").await;
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::getter"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn getter()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("body-x".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    // Before write: description None, hashes (body=Some, source=None).
    let info = client
        .get_symbol_description("crate::getter")
        .await
        .unwrap()
        .expect("symbol exists");
    assert_eq!(info.description, None);
    assert_eq!(info.body_hash.as_deref(), Some("body-x"));
    assert_eq!(info.description_source_hash, None);

    client
        .set_symbol_description_and_embedding("crate::getter", "Gets stuff.", &vec![0.0f32; 768])
        .await
        .unwrap();

    let info = client
        .get_symbol_description("crate::getter")
        .await
        .unwrap()
        .expect("symbol exists");
    assert_eq!(info.description.as_deref(), Some("Gets stuff."));
    assert_eq!(info.body_hash.as_deref(), Some("body-x"));
    assert_eq!(info.description_source_hash.as_deref(), Some("body-x"));

    assert!(
        client
            .get_symbol_description("crate::nope")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn description_source_hashes_for_batch_returns_only_described_rows() {
    let client = fresh_client("mycel:test:hash_batch").await;

    let described = Symbol {
        qualified_name: QualifiedName::new("crate::described_batch"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn described_batch()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("hd".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&described).await.unwrap();
    client
        .set_symbol_description_and_embedding(
            "crate::described_batch",
            "Has desc.",
            &vec![0.0; 768],
        )
        .await
        .unwrap();

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
        .await
        .unwrap();

    assert_eq!(
        map.get("crate::described_batch").map(|s| s.as_str()),
        Some("hd")
    );
    assert!(!map.contains_key("crate::undescribed_batch"));
    assert!(!map.contains_key("crate::nonexistent"));
}

#[tokio::test]
async fn description_source_hashes_for_batch_empty_input_returns_empty() {
    let client = GraphClient::connect(&url(), "mycel:test:hash_batch_empty")
        .await
        .unwrap();
    let map = client.description_source_hashes_for_batch(&[]).await.unwrap();
    assert!(map.is_empty());
}

#[tokio::test]
async fn clear_description_and_reembed_resets_atomic() {
    let client = fresh_client("mycel:test:clear").await;
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::clearer"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn clearer()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("body-v1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    client
        .set_symbol_description_and_embedding("crate::clearer", "Clears state.", &vec![0.5f32; 768])
        .await
        .unwrap();

    let new_vec = vec![0.9f32; 768];
    client
        .clear_symbol_description_and_reembed("crate::clearer", "body-v2", &new_vec)
        .await
        .unwrap();

    let info = client
        .get_symbol_description("crate::clearer")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.description, None);
    assert_eq!(info.description_source_hash, None);
    assert_eq!(info.body_hash.as_deref(), Some("body-v2"));
}

#[tokio::test]
async fn list_stale_descriptions_finds_diverged_hashes() {
    let client = fresh_client("mycel:test:stale").await;

    let a = Symbol {
        qualified_name: QualifiedName::new("crate::fresh"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn fresh()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&a).await.unwrap();
    client
        .set_symbol_description_and_embedding("crate::fresh", "Fresh.", &vec![0.0; 768])
        .await
        .unwrap();

    let b = Symbol {
        qualified_name: QualifiedName::new("crate::stale"),
        body_hash: Some("h1".into()),
        description_source_hash: None,
        ..a.clone()
    };
    client.upsert_symbol(&b).await.unwrap();
    client
        .set_symbol_description_and_embedding("crate::stale", "Will go stale.", &vec![0.0; 768])
        .await
        .unwrap();
    client
        .query("MATCH (s:Symbol {qualified_name: 'crate::stale'}) SET s.body_hash = 'h2'")
        .await
        .unwrap();

    let stale = client.list_stale_descriptions().await.unwrap();
    let stale_qnames: std::collections::HashSet<_> =
        stale.iter().map(|s| s.qualified_name.clone()).collect();
    assert!(stale_qnames.contains("crate::stale"));
    assert!(!stale_qnames.contains("crate::fresh"));
}

#[tokio::test]
async fn refresh_description_source_hashes_only_touches_legacy_rows() {
    let client = fresh_client("mycel:test:refresh").await;

    let legacy = Symbol {
        qualified_name: QualifiedName::new("crate::legacy"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn legacy()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("hL".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&legacy).await.unwrap();
    client
        .query(
            "MATCH (s:Symbol {qualified_name: 'crate::legacy'}) \
             SET s.synthesized_description = 'old desc', \
                 s.embedding = vecf32([0.0])",
        )
        .await
        .unwrap();

    let modern = Symbol {
        qualified_name: QualifiedName::new("crate::modern"),
        body_hash: Some("hM".into()),
        ..legacy.clone()
    };
    client.upsert_symbol(&modern).await.unwrap();
    client
        .set_symbol_description_and_embedding("crate::modern", "modern desc", &vec![0.0; 768])
        .await
        .unwrap();

    let n = client.refresh_description_source_hashes().await.unwrap();
    assert_eq!(n, 1, "exactly one legacy row backfilled");

    let l = client
        .get_symbol_description("crate::legacy")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        l.description_source_hash.as_deref(),
        Some("hL"),
        "legacy row's source_hash backfilled from body_hash"
    );
    let m = client
        .get_symbol_description("crate::modern")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        m.description_source_hash.as_deref(),
        Some("hM"),
        "modern row untouched"
    );
}

#[tokio::test]
async fn cold_index_writes_embedding_and_body_hash() {
    let client = fresh_client("mycel:test:cold_index").await;

    // Stub embedder so we don't depend on a running Ollama.
    struct StubEmbedder;
    #[async_trait::async_trait]
    impl mycel_models::Embedder for StubEmbedder {
        fn identity(&self) -> &str {
            "stub/test"
        }
        fn dimension(&self) -> u32 {
            768
        }
        async fn embed(&self, texts: &[&str]) -> mycel_core::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.42_f32; 768]).collect())
        }
    }

    let indexer = mycel_index::Indexer {
        graph: client.clone(),
        lsp: None,
        embedder: std::sync::Arc::new(StubEmbedder),
    };

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("hello.rs");
    std::fs::write(&f, "fn hello() { println!(\"hi\"); }\n").unwrap();
    let f_utf8 = camino::Utf8PathBuf::from_path_buf(f).unwrap();
    indexer
        .index_file(&f_utf8, "fn hello() { println!(\"hi\"); }\n")
        .await
        .unwrap();

    let rows = client
        .query(
            "MATCH (s:Symbol) WHERE s.file_path ENDS WITH 'hello.rs' \
             RETURN s.qualified_name, s.body_hash, s.embedding",
        )
        .await
        .unwrap();
    assert!(!rows.is_empty(), "expected at least one symbol");
    for row in rows {
        let body_hash = match &row[1] {
            falkordb::FalkorValue::String(s) => s.clone(),
            other => panic!("body_hash: {other:?}"),
        };
        assert_eq!(body_hash.len(), 64, "blake3 hex");
    }
}

#[tokio::test]
async fn clear_all_descriptions_wipes_all_description_state() {
    let client = fresh_client("mycel:test:clear_all").await;

    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::with_desc"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn with_desc()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    client
        .set_symbol_description_and_embedding("crate::with_desc", "Has desc.", &vec![0.1; 768])
        .await
        .unwrap();

    let n = client.clear_all_descriptions().await.unwrap();
    assert!(n >= 1);

    let info = client
        .get_symbol_description("crate::with_desc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.description, None);
    assert_eq!(info.description_source_hash, None);
}

#[tokio::test]
async fn incremental_index_clears_stale_description() {
    let client = fresh_client("mycel:test:stale_clear").await;

    struct StubEmbedder;
    #[async_trait::async_trait]
    impl mycel_models::Embedder for StubEmbedder {
        fn identity(&self) -> &str {
            "stub/test"
        }
        fn dimension(&self) -> u32 {
            768
        }
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
        graph: client.clone(),
        lsp: None,
        embedder: std::sync::Arc::new(StubEmbedder),
    };
    indexer.index_file(&f_utf8, v1).await.unwrap();

    let definers = client.query_definers("staleable").await.unwrap();
    let qname_v1 = definers.first().expect("indexed").qualified_name.clone();

    client
        .set_symbol_description_and_embedding(qname_v1.as_str(), "Returns 1.", &vec![0.5; 768])
        .await
        .unwrap();

    let v2 = "fn staleable() {\n  let x = 99;\n  x * 2\n}\n";
    std::fs::write(&f, v2).unwrap();
    indexer.index_file(&f_utf8, v2).await.unwrap();

    let definers_after = client.query_definers("staleable").await.unwrap();
    let qname_v2 = &definers_after.first().expect("indexed v2").qualified_name;
    assert_eq!(qname_v2, &qname_v1, "qname stable across reindex");

    let info = client
        .get_symbol_description(qname_v1.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        info.description, None,
        "description must be cleared when body_hash diverges from description_source_hash"
    );
    assert_eq!(info.description_source_hash, None);
    assert!(info.body_hash.is_some(), "fresh body_hash written");
}

#[tokio::test]
async fn incremental_index_preserves_description_when_body_slice_unchanged() {
    let client = fresh_client("mycel:test:preserve").await;

    struct StubEmbedder;
    #[async_trait::async_trait]
    impl mycel_models::Embedder for StubEmbedder {
        fn identity(&self) -> &str {
            "stub/test"
        }
        fn dimension(&self) -> u32 {
            768
        }
        async fn embed(&self, texts: &[&str]) -> mycel_core::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.1; 768]).collect())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("stable.rs");
    let v1 = "fn stable() {\n    7\n}\n";
    std::fs::write(&f, v1).unwrap();
    let f_utf8 = camino::Utf8PathBuf::from_path_buf(f.clone()).unwrap();

    let indexer = mycel_index::Indexer {
        graph: client.clone(),
        lsp: None,
        embedder: std::sync::Arc::new(StubEmbedder),
    };
    indexer.index_file(&f_utf8, v1).await.unwrap();

    let definers = client.query_definers("stable").await.unwrap();
    let qname_v1 = definers.first().expect("indexed").qualified_name.clone();
    client
        .set_symbol_description_and_embedding(qname_v1.as_str(), "Returns 7.", &vec![0.5; 768])
        .await
        .unwrap();

    // v2: identical function body, but a comment line is appended below.
    // content_hash changes (so dedup does NOT skip), but the function's
    // signature+body slice is byte-identical, so body_hash is unchanged.
    let v2 = "fn stable() {\n    7\n}\n// added trailing comment\n";
    std::fs::write(&f, v2).unwrap();
    indexer.index_file(&f_utf8, v2).await.unwrap();

    let definers_after = client.query_definers("stable").await.unwrap();
    let qname_v2 = &definers_after.first().expect("indexed v2").qualified_name;
    assert_eq!(qname_v2, &qname_v1, "qname stable across reindex");

    let info = client
        .get_symbol_description(qname_v1.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        info.description.as_deref(),
        Some("Returns 7."),
        "description preserved when body slice unchanged"
    );
    assert!(
        info.description_source_hash.is_some(),
        "source_hash preserved (still matches body_hash)"
    );
}

#[tokio::test]
async fn set_description_round_trips_newlines_and_quotes() {
    // Regression: descriptions are user-/Claude-supplied text. Newlines and
    // single quotes inside the description must not corrupt Cypher parsing
    // or change the stored value.
    let client = fresh_client("mycel:test:desc_quote_newline").await;
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::tricky"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn tricky()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("h".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    let payload = "Line one with 'quotes'.\nLine two\twith tab.\nLine three.";
    client
        .set_symbol_description_and_embedding("crate::tricky", payload, &vec![0.1; 768])
        .await
        .unwrap();

    let info = client
        .get_symbol_description("crate::tricky")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.description.as_deref(), Some(payload));
}

#[tokio::test]
async fn upsert_symbol_preserves_description_source_hash_on_none() {
    // Sibling invariant to upsert_symbol_writes_body_hash_when_set: an
    // extractor-shaped upsert (no description_source_hash on the input)
    // must NOT clobber a value the description-write path stamped on a
    // prior pass. The pipeline depends on this for the staleness pre-pass
    // to observe the prior source_hash.
    let client = fresh_client("mycel:test:source_hash_preserve").await;
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::preserve_src"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn preserve_src()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("body-1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    client
        .set_symbol_description_and_embedding(
            "crate::preserve_src",
            "Stamped.",
            &vec![0.1; 768],
        )
        .await
        .unwrap();

    // Re-upsert the extractor-shape Symbol (description_source_hash: None).
    client.upsert_symbol(&sym).await.unwrap();

    let info = client
        .get_symbol_description("crate::preserve_src")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        info.description_source_hash.as_deref(),
        Some("body-1"),
        "upsert with None source_hash must preserve prior stamped value"
    );
}
