use mycel_core::*;

use crate::GraphClient;
use crate::cypher::escape;
use crate::symbol::parse_symbol_row;

fn edge_kind_to_cypher(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Defines => "DEFINES",
        EdgeKind::Calls => "CALLS",
        EdgeKind::Imports => "IMPORTS",
        EdgeKind::Extends => "EXTENDS",
        EdgeKind::Implements => "IMPLEMENTS",
        EdgeKind::UsesType => "USES_TYPE",
        EdgeKind::References => "REFERENCES",
        EdgeKind::ReExports => "RE_EXPORTS",
        EdgeKind::CoChanged => "CO_CHANGED",
        EdgeKind::TestedBy => "TESTED_BY",
        EdgeKind::ModifiedIn => "MODIFIED_IN",
    }
}

impl GraphClient {
    /// Upsert relationships between two existing Symbol nodes. Both endpoints
    /// MUST already exist (call `upsert_symbol_batch` first) — edges
    /// referencing missing endpoints are silently dropped because the MATCH
    /// yields no rows. The Indexer (Task 6.1) is responsible for ordering.
    pub async fn upsert_edge_batch(&self, edges: &[Edge]) -> Result<()> {
        for e in edges {
            let src_value =
                serde_json::to_value(&e.source).expect("EdgeSource serializes infallibly");
            let src_str = src_value
                .as_str()
                .expect("EdgeSource serializes as JSON string");
            let cypher = format!(
                r#"MATCH (a:Symbol {{qualified_name: '{from}'}}), (b:Symbol {{qualified_name: '{to}'}})
                   MERGE (a)-[r:{rel}]->(b)
                   SET r.source = '{src}'"#,
                from = escape(&e.from),
                to = escape(&e.to),
                rel = edge_kind_to_cypher(e.kind),
                src = src_str,
            );
            self.query(&cypher).await?;
        }
        Ok(())
    }

    pub async fn query_callers(&self, qualified: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:CALLS]->(b:Symbol {{qualified_name: '{q}'}}) \
             RETURN a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            q = escape(qualified),
        );
        Ok(self
            .query(&cypher)
            .await?
            .into_iter()
            .filter_map(parse_symbol_row)
            .collect())
    }

    pub async fn query_callees(&self, qualified: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol {{qualified_name: '{q}'}})-[:CALLS]->(b:Symbol) \
             RETURN b.qualified_name, b.kind, b.file_path, b.start_line, b.end_line, \
                    b.signature, b.exported",
            q = escape(qualified),
        );
        Ok(self
            .query(&cypher)
            .await?
            .into_iter()
            .filter_map(parse_symbol_row)
            .collect())
    }
}
