use crate::GraphClient;
use falkordb::FalkorValue;
use mycel_core::*;

impl GraphClient {
    /// Vector search the top-K symbols by cosine similarity to the given query vector.
    /// Returns (Symbol, score) pairs.
    ///
    /// If a manifest exists for this graph (keyed by `self.graph_name`),
    /// errors if `expected_embedder` / `expected_dim` don't match.
    /// Without a manifest, the safety check is skipped (Task 6.1's indexer
    /// is responsible for writing one).
    pub async fn vector_search_top_k(
        &self,
        query: &[f32],
        k: usize,
        expected_embedder: &str,
        expected_dim: u32,
    ) -> Result<Vec<(Symbol, f32)>> {
        if let Some(m) = self.read_manifest(&self.graph_name).await? {
            if m.embedder_identity != expected_embedder || m.embedder_dimension != expected_dim {
                return Err(MycelError::EmbedderMismatch {
                    graph: format!("{}@{}", m.embedder_identity, m.embedder_dimension),
                    runtime: format!("{expected_embedder}@{expected_dim}"),
                });
            }
        }
        let vec_lit = format!(
            "vecf32([{}])",
            query
                .iter()
                .map(|f| format!("{f}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let cypher = format!(
            "CALL db.idx.vector.queryNodes('Symbol', 'embedding', {k}, {vec_lit}) \
             YIELD node, score \
             RETURN node.qualified_name, node.kind, node.file_path, node.start_line, node.end_line, \
                    node.signature, node.exported, score",
        );
        let rows = self.query(&cypher).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut iter = row.into_iter();
            let symbol = crate::symbol::parse_symbol_row(iter.by_ref().take(7).collect());
            if let Some(s) = symbol {
                let score = match iter.next() {
                    Some(FalkorValue::F64(f)) => f as f32,
                    _ => 0.0,
                };
                out.push((s, score));
            }
        }
        Ok(out)
    }
}
