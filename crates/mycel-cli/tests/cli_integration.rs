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

async fn fresh_client(graph_name: &str) -> GraphClient {
    let client = GraphClient::connect(&url(), graph_name).await.unwrap();
    client.query("MATCH (n) DETACH DELETE n").await.unwrap();
    client
}

#[tokio::test]
async fn describe_prints_none_for_undescribed_symbol() {
    let graph_name = "mycel:cli_test:desc_none";
    let client = fresh_client(graph_name).await;
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::described"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn described()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();

    Command::cargo_bin("mycel")
        .unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name).env("MYCEL_FALKORDB_URL", url())
        .args(["describe", "crate::described"])
        .assert()
        .success()
        .stdout(contains("(none)"));
}

#[tokio::test]
async fn describe_prints_text_when_set() {
    let graph_name = "mycel:cli_test:desc_text";
    let client = fresh_client(graph_name).await;
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
        .set_symbol_description_and_embedding(
            "crate::with_desc",
            "Does a thing.",
            &vec![0.1; 768],
        )
        .await
        .unwrap();

    Command::cargo_bin("mycel")
        .unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name).env("MYCEL_FALKORDB_URL", url())
        .args(["describe", "crate::with_desc"])
        .assert()
        .success()
        .stdout(contains("Does a thing."));
}

#[tokio::test]
async fn describe_json_includes_hashes() {
    let graph_name = "mycel:cli_test:desc_json";
    let client = fresh_client(graph_name).await;
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::with_json"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn with_json()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("h1".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    client
        .set_symbol_description_and_embedding(
            "crate::with_json",
            "Json description.",
            &vec![0.1; 768],
        )
        .await
        .unwrap();

    let out = Command::cargo_bin("mycel")
        .unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name).env("MYCEL_FALKORDB_URL", url())
        .args(["--json", "describe", "crate::with_json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"description\""), "stdout: {stdout}");
    assert!(stdout.contains("\"body_hash\""));
    assert!(stdout.contains("\"description_source_hash\""));
    assert!(stdout.contains("Json description."));
}

#[tokio::test]
async fn set_description_unknown_qname_exits_nonzero_with_helpful_error() {
    let graph_name = "mycel:cli_test:set_desc_unknown";
    let _ = fresh_client(graph_name).await;

    Command::cargo_bin("mycel")
        .unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name)
        .env("MYCEL_FALKORDB_URL", url())
        .args([
            "set-description",
            "--qname",
            "crate::not::a::real::symbol",
            "--description",
            "Whatever.",
        ])
        .assert()
        .failure()
        .stderr(contains("no Symbol"))
        .stderr(contains("definers")); // hint Claude at the recovery path
}

#[tokio::test]
async fn synthesize_refresh_hashes_only_backfills_legacy_rows() {
    let graph_name = "mycel:cli_test:refresh";
    let client = fresh_client(graph_name).await;

    let legacy = Symbol {
        qualified_name: QualifiedName::new("crate::legacy_sym"),
        kind: SymbolKind::Function,
        file_path: "x.rs".into(),
        start_line: 1,
        end_line: 2,
        signature: Signature::new("fn legacy_sym()"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
        body_hash: Some("legacy_h".into()),
        description_source_hash: None,
    };
    client.upsert_symbol(&legacy).await.unwrap();
    client
        .query(
            "MATCH (s:Symbol {qualified_name: 'crate::legacy_sym'}) \
             SET s.synthesized_description = 'old', s.embedding = vecf32([0.1])",
        )
        .await
        .unwrap();

    Command::cargo_bin("mycel")
        .unwrap()
        .env("MYCEL_TEST_GRAPH", graph_name)
        .env("MYCEL_FALKORDB_URL", url())
        .args(["synthesize", "--refresh-hashes-only"])
        .assert()
        .success()
        .stdout(contains("backfilled 1"));

    let info = client
        .get_symbol_description("crate::legacy_sym")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(info.description_source_hash.as_deref(), Some("legacy_h"));
}
