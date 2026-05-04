use crate::GraphClient;
use crate::cypher::escape;
use crate::symbol::parse_symbol_row;
use mycel_core::*;

impl GraphClient {
    /// "Who imports from this file?"
    ///
    /// Per DESIGN.md schema, `IMPORTS` edges can be either `Symbol→Symbol`
    /// (e.g., a TS function imports another exported function) or `File→File`
    /// (one file imports another). Phase 1 covers the Symbol→Symbol case,
    /// which is the dominant pattern in TS/Rust. The File→File case is added
    /// in Chunk 3 once the extractor produces those edges; revisit this query
    /// then to UNION in the file-level edges.
    pub async fn query_imports(&self, file_path: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:IMPORTS]->(b:Symbol) WHERE b.file_path = '{p}' \
             RETURN DISTINCT a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            p = escape(file_path),
        );
        Ok(self
            .query(&cypher)
            .await?
            .into_iter()
            .filter_map(parse_symbol_row)
            .collect())
    }

    pub async fn query_uses(&self, type_qname: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:USES_TYPE]->(t:Symbol {{qualified_name: '{q}'}}) \
             RETURN DISTINCT a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            q = escape(type_qname),
        );
        Ok(self
            .query(&cypher)
            .await?
            .into_iter()
            .filter_map(parse_symbol_row)
            .collect())
    }

    pub async fn query_implements(&self, interface_qname: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:IMPLEMENTS]->(b:Symbol {{qualified_name: '{q}'}}) \
             RETURN a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            q = escape(interface_qname),
        );
        Ok(self
            .query(&cypher)
            .await?
            .into_iter()
            .filter_map(parse_symbol_row)
            .collect())
    }
}
