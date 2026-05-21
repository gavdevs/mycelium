use crate::GraphClient;
use crate::cypher::escape;
use crate::symbol::parse_symbol_row;
use falkordb::FalkorValue;
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

    /// Returns the qname of the Symbol whose `start_line..=end_line` range
    /// contains `line` in `file_path`. Used by Workstream A's pipeline to map
    /// LSP-returned definition locations back to Symbol qnames.
    ///
    /// Returns `Ok(None)` when no Symbol spans that location — common case
    /// for definitions outside the indexed surface (stdlib, node_modules) or
    /// for references into module-decl symbols whose range we don't model.
    /// For nested symbols (e.g., a method inside a class — both could match
    /// line N), `LIMIT 1` arbitrarily picks one; a future iteration may
    /// prefer the smallest enclosing range.
    pub async fn symbol_containing(
        &self,
        file_path: &str,
        line: u32,
    ) -> Result<Option<String>> {
        let cypher = format!(
            "MATCH (s:Symbol) WHERE s.file_path = '{p}' \
               AND s.start_line <= {l} AND s.end_line >= {l} \
             RETURN s.qualified_name LIMIT 1",
            p = escape(file_path),
            l = line,
        );
        let rows = self.query(&cypher).await?;
        Ok(rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .and_then(|v| match v {
                FalkorValue::String(s) => Some(s),
                _ => None,
            }))
    }
}
