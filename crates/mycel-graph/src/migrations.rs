use crate::GraphClient;
use mycel_core::Result;
use tracing::info;

const MIGRATIONS: &[(&str, &[&str])] = &[
    (
        "v1_symbol_indices",
        &[
            "CREATE INDEX FOR (s:Symbol) ON (s.qualified_name)",
            "CREATE INDEX FOR (s:Symbol) ON (s.file_path)",
            "CREATE INDEX FOR (f:File) ON (f.path)",
            "CREATE INDEX FOR (c:Commit) ON (c.sha)",
        ],
    ),
    (
        "v1_vector_index",
        &[
            // FalkorDB vector index. Dimension matches EmbeddingGemma 768d.
            "CREATE VECTOR INDEX FOR (s:Symbol) ON (s.embedding) OPTIONS {dimension: 768, similarityFunction: 'cosine'}",
        ],
    ),
    (
        "v2_name_index",
        &[
            // Indexes the short name (last `::` or `.` segment) used by query_definers.
            "CREATE INDEX FOR (s:Symbol) ON (s.name)",
        ],
    ),
];

pub async fn run_all(client: &GraphClient) -> Result<()> {
    // FalkorDB indices are per-graph, so migration tracking must also be per-graph.
    // We store records in the meta graph for centralized inspection, but each record
    // is scoped to a specific graph_name to ensure each repo's graph gets its own
    // index creation pass.
    let applied = client.applied_migrations().await?;
    for (id, statements) in MIGRATIONS {
        if applied.iter().any(|a| a == id) {
            continue;
        }
        info!(graph = %client.graph_name, migration = id, "applying migration");
        for stmt in *statements {
            // CREATE INDEX is idempotent at the FalkorDB layer (errors if exists);
            // we swallow such errors. The per-graph MigrationRecord guard prevents
            // reapplication on subsequent runs.
            let _ = client.query(stmt).await;
        }
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mark = format!(
            "CREATE (:MigrationRecord {{graph_name: '{g}', migration_id: '{id}', applied_at: {ts}}})",
            g = crate::cypher::escape(&client.graph_name),
            id = id,
            ts = now
        );
        client.meta_query(&mark).await?;
    }
    Ok(())
}
