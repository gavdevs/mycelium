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
                serde_json::to_value(e.source).expect("EdgeSource serializes infallibly");
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

    /// "Who calls this?" Matches the callee on either the full qualified name
    /// (e.g., `tests/fixtures/typescript/simple_function.ts::add`) or the short
    /// `name` property (e.g., `add`), so users don't have to type fully-qualified
    /// names. Mirrors `query_definers`'s name-or-qname behavior.
    pub async fn query_callers(&self, name_or_qname: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:CALLS]->(b:Symbol) \
             WHERE b.name = '{q}' OR b.qualified_name = '{q}' \
             RETURN DISTINCT a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            q = escape(name_or_qname),
        );
        Ok(self
            .query(&cypher)
            .await?
            .into_iter()
            .filter_map(parse_symbol_row)
            .collect())
    }

    /// "What does this call?" Matches the caller on short `name` or full
    /// qualified name. See `query_callers` for rationale.
    pub async fn query_callees(&self, name_or_qname: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:CALLS]->(b:Symbol) \
             WHERE a.name = '{q}' OR a.qualified_name = '{q}' \
             RETURN DISTINCT b.qualified_name, b.kind, b.file_path, b.start_line, b.end_line, \
                    b.signature, b.exported",
            q = escape(name_or_qname),
        );
        Ok(self
            .query(&cypher)
            .await?
            .into_iter()
            .filter_map(parse_symbol_row)
            .collect())
    }
}
