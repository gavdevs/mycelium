use falkordb::FalkorValue;
use mycel_core::*;
use tracing::warn;

use crate::GraphClient;
use crate::cypher::escape;

pub(crate) fn short_name(qualified: &str) -> &str {
    qualified
        .rsplit_once("::")
        .map(|(_, n)| n)
        .or_else(|| qualified.rsplit_once('.').map(|(_, n)| n))
        .unwrap_or(qualified)
}

/// Parses a Symbol from a FalkorDB result row matching the order:
/// (qualified_name, kind, file_path, start_line, end_line, signature, exported).
///
/// Returns `None` on any shape mismatch and logs a `tracing::warn!` so future
/// schema drift surfaces in production logs rather than silently truncating
/// query results.
pub(crate) fn parse_symbol_row(row: Vec<FalkorValue>) -> Option<Symbol> {
    let mut iter = row.into_iter();
    let qname = match iter.next() {
        Some(FalkorValue::String(s)) => s,
        other => {
            warn!(?other, "parse_symbol_row: expected String at qualified_name");
            return None;
        }
    };
    let kind_s = match iter.next() {
        Some(FalkorValue::String(s)) => s,
        other => {
            warn!(?other, "parse_symbol_row: expected String at kind");
            return None;
        }
    };
    let kind: SymbolKind =
        match serde_json::from_value::<SymbolKind>(serde_json::Value::String(kind_s.clone())) {
            Ok(k) => k,
            Err(e) => {
                warn!(error = %e, value = %kind_s, "parse_symbol_row: failed to decode SymbolKind");
                return None;
            }
        };
    let file = match iter.next() {
        Some(FalkorValue::String(s)) => s,
        other => {
            warn!(?other, "parse_symbol_row: expected String at file_path");
            return None;
        }
    };
    let start = match iter.next() {
        Some(FalkorValue::I64(n)) => n as u32,
        other => {
            warn!(?other, "parse_symbol_row: expected I64 at start_line");
            return None;
        }
    };
    let end = match iter.next() {
        Some(FalkorValue::I64(n)) => n as u32,
        other => {
            warn!(?other, "parse_symbol_row: expected I64 at end_line");
            return None;
        }
    };
    let sig = match iter.next() {
        Some(FalkorValue::String(s)) => s,
        other => {
            warn!(?other, "parse_symbol_row: expected String at signature");
            return None;
        }
    };
    let exported = match iter.next() {
        Some(FalkorValue::Bool(b)) => b,
        other => {
            warn!(?other, "parse_symbol_row: expected Bool at exported");
            return None;
        }
    };
    Some(Symbol {
        qualified_name: QualifiedName::new(qname),
        kind,
        file_path: file.into(),
        start_line: start,
        end_line: end,
        signature: Signature::new(sig),
        jsdoc: None,
        synthesized_description: None,
        exported,
        embedding: None,
    })
}

impl GraphClient {
    /// Upsert (MERGE) a Symbol node. Sets all scalar fields but does NOT set
    /// `s.embedding` — that's populated separately by the indexer (Task 5.2/6.1).
    /// The `v1_vector_index` migration handles missing-embedding nodes gracefully
    /// (they're just not indexed for vector search until embedded).
    pub async fn upsert_symbol(&self, sym: &Symbol) -> Result<()> {
        let kind = serde_json::to_value(sym.kind).expect("SymbolKind serializes infallibly");
        let kind_str = kind.as_str().expect("SymbolKind serializes as JSON string");
        let cypher = format!(
            r#"MERGE (s:Symbol {{qualified_name: '{qname}'}})
            SET s.kind = '{kind}',
                s.file_path = '{file}',
                s.start_line = {start},
                s.end_line = {end},
                s.signature = '{sig}',
                s.exported = {exported},
                s.name = '{name}'"#,
            qname = escape(sym.qualified_name.as_str()),
            kind = kind_str,
            file = escape(sym.file_path.as_str()),
            start = sym.start_line,
            end = sym.end_line,
            sig = escape(sym.signature.as_str()),
            exported = sym.exported,
            name = escape(short_name(sym.qualified_name.as_str())),
        );
        self.query(&cypher).await?;
        Ok(())
    }

    /// Sequential MERGE per symbol. Acceptable for v0; revisit when the
    /// `Arc<Mutex>` in `client.rs` is replaced (see TODO(perf) there) so
    /// upserts can dispatch concurrently across the FalkorDB connection pool.
    pub async fn upsert_symbol_batch(&self, syms: &[Symbol]) -> Result<()> {
        for s in syms {
            self.upsert_symbol(s).await?;
        }
        Ok(())
    }

    /// Detach-delete all Symbol nodes owned by `file_path` whose qualified
    /// names are NOT in `keep`. Used by the indexer to prune symbols that
    /// were removed from a file between reindex passes — without this,
    /// deleting a function from a file leaves a ghost Symbol in the graph
    /// that still answers `definers`/`find` queries.
    ///
    /// `DETACH DELETE` cascades through relationships, so any incoming or
    /// outgoing edges are also removed. Cross-file edges to the deleted
    /// symbol disappear correctly because the symbol no longer exists.
    pub async fn prune_stale_symbols(&self, file_path: &str, keep: &[&str]) -> Result<usize> {
        // Build the IN-list of qnames to keep. Empty list means "delete all
        // symbols for this file" — valid when a file becomes empty/non-source.
        let kept = if keep.is_empty() {
            "[]".to_string()
        } else {
            let parts: Vec<String> = keep.iter().map(|q| format!("'{}'", escape(q))).collect();
            format!("[{}]", parts.join(", "))
        };
        let cypher = format!(
            "MATCH (s:Symbol) WHERE s.file_path = '{p}' AND NOT s.qualified_name IN {kept} \
             DETACH DELETE s RETURN count(s) AS deleted",
            p = escape(file_path),
        );
        let rows = self.query(&cypher).await?;
        let deleted = rows
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next())
            .and_then(|v| match v {
                FalkorValue::I64(n) => Some(n as usize),
                _ => None,
            })
            .unwrap_or(0);
        Ok(deleted)
    }

    pub async fn query_definers(&self, name: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (s:Symbol) WHERE s.name = '{name}' OR s.qualified_name = '{name}' \
             RETURN s.qualified_name, s.kind, s.file_path, s.start_line, s.end_line, \
                    s.signature, s.exported",
            name = escape(name),
        );
        let rows = self.query(&cypher).await?;
        Ok(rows.into_iter().filter_map(parse_symbol_row).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_double_colon() {
        assert_eq!(short_name("crate::foo::bar"), "bar");
    }

    #[test]
    fn short_name_dot() {
        assert_eq!(short_name("module.foo.bar"), "bar");
    }

    #[test]
    fn short_name_double_colon_wins_over_dot() {
        assert_eq!(short_name("foo.bar::Baz"), "Baz");
    }

    #[test]
    fn short_name_no_separator() {
        assert_eq!(short_name("Anonymous"), "Anonymous");
    }

    #[test]
    fn short_name_empty() {
        assert_eq!(short_name(""), "");
    }
}
