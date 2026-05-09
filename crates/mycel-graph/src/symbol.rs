use falkordb::FalkorValue;
use mycel_core::*;
use tracing::warn;

use crate::GraphClient;
use crate::cypher::escape;

/// A Symbol surfaced for the description-synthesis pipeline. Carries the
/// minimum the indexer needs to slice the body from disk and decide whether
/// to skip (already described).
#[derive(Debug, Clone)]
pub struct SymbolForSynthesis {
    pub qualified_name: String,
    pub signature: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub has_description: bool,
}

/// 1-hop graph neighborhood used to build a synthesis prompt.
#[derive(Debug, Clone, Default)]
pub struct SynthesisContext {
    pub callers: Vec<SymbolNeighbor>,
    pub callees: Vec<SymbolNeighbor>,
}

#[derive(Debug, Clone)]
pub struct SymbolNeighbor {
    pub qualified_name: String,
    pub signature: String,
}

/// A Symbol's description state plus the two hashes used to detect staleness.
/// Used by `mycel describe` and the workload-driven synthesis skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolDescriptionInfo {
    pub description: Option<String>,
    pub body_hash: Option<String>,
    pub description_source_hash: Option<String>,
}

fn parse_neighbor_rows(rows: Vec<Vec<FalkorValue>>) -> Vec<SymbolNeighbor> {
    rows.into_iter()
        .filter_map(|row| {
            let mut iter = row.into_iter();
            let qname = match iter.next() {
                Some(FalkorValue::String(s)) => s,
                _ => return None,
            };
            let signature = match iter.next() {
                Some(FalkorValue::String(s)) => s,
                _ => String::new(),
            };
            Some(SymbolNeighbor {
                qualified_name: qname,
                signature,
            })
        })
        .collect()
}

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
        body_hash: None,
        description_source_hash: None,
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
        // body_hash is computed by the indexing pipeline, not by the extractor;
        // an upsert from extracted-Symbol-only paths leaves it None and we must
        // NOT clobber a hash a previous pipeline pass already wrote.
        let body_hash_clause = match &sym.body_hash {
            Some(h) => format!(", s.body_hash = '{}'", escape(h)),
            None => String::new(),
        };
        let cypher = format!(
            r#"MERGE (s:Symbol {{qualified_name: '{qname}'}})
            SET s.kind = '{kind}',
                s.file_path = '{file}',
                s.start_line = {start},
                s.end_line = {end},
                s.signature = '{sig}',
                s.exported = {exported},
                s.name = '{name}'{body_hash_clause}"#,
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

    /// Returns every Symbol's qualified_name + signature in this graph,
    /// optionally filtered to those without a synthesized_description (the
    /// common case for Phase 2 synthesis: only describe what hasn't been
    /// described yet, so re-runs are idempotent and cheap).
    ///
    /// Result rows: (qualified_name, signature, file_path, start_line, end_line, has_description).
    pub async fn list_symbols_for_synthesis(
        &self,
        only_missing: bool,
    ) -> Result<Vec<SymbolForSynthesis>> {
        // FalkorDB's Cypher dialect lacks `IS NULL` predicates on missing
        // properties — non-existent properties evaluate to null, but the
        // standard equality test against null returns null, not false. The
        // safe pattern is `coalesce(s.synthesized_description, '') = ''`.
        let filter = if only_missing {
            "WHERE coalesce(s.synthesized_description, '') = ''"
        } else {
            ""
        };
        let cypher = format!(
            "MATCH (s:Symbol) {filter} \
             RETURN s.qualified_name, s.signature, s.file_path, s.start_line, s.end_line, \
                    coalesce(s.synthesized_description, '') AS desc",
        );
        let rows = self.query(&cypher).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut iter = row.into_iter();
            let qname = match iter.next() {
                Some(FalkorValue::String(s)) => s,
                _ => continue,
            };
            let signature = match iter.next() {
                Some(FalkorValue::String(s)) => s,
                _ => String::new(),
            };
            let file_path = match iter.next() {
                Some(FalkorValue::String(s)) => s,
                _ => continue,
            };
            let start_line = match iter.next() {
                Some(FalkorValue::I64(n)) => n as u32,
                _ => 0,
            };
            let end_line = match iter.next() {
                Some(FalkorValue::I64(n)) => n as u32,
                _ => 0,
            };
            let has_description = matches!(iter.next(), Some(FalkorValue::String(s)) if !s.is_empty());
            out.push(SymbolForSynthesis {
                qualified_name: qname,
                signature,
                file_path,
                start_line,
                end_line,
                has_description,
            });
        }
        Ok(out)
    }

    /// Returns 1-hop neighbors (callers + callees) for a symbol, with their
    /// signatures only (not bodies). Used to build description-synthesis
    /// prompts. Caps each side at `limit` to keep prompts under the small
    /// model's effective context.
    pub async fn query_synthesis_context(
        &self,
        qname: &str,
        limit: usize,
    ) -> Result<SynthesisContext> {
        let callers_cypher = format!(
            "MATCH (a:Symbol)-[:CALLS]->(b:Symbol {{qualified_name: '{q}'}}) \
             RETURN DISTINCT a.qualified_name, a.signature LIMIT {limit}",
            q = escape(qname),
        );
        let callees_cypher = format!(
            "MATCH (a:Symbol {{qualified_name: '{q}'}})-[:CALLS]->(b:Symbol) \
             RETURN DISTINCT b.qualified_name, b.signature LIMIT {limit}",
            q = escape(qname),
        );
        let callers = parse_neighbor_rows(self.query(&callers_cypher).await?);
        let callees = parse_neighbor_rows(self.query(&callees_cypher).await?);
        Ok(SynthesisContext { callers, callees })
    }

    /// Writes the synthesized description AND its embedding on a Symbol node
    /// in a single Cypher statement. Atomic at the query boundary — either
    /// both land or neither does, so a partially-described node can never
    /// slip past `list_symbols_for_synthesis(only_missing=true)` on re-runs.
    ///
    /// (An earlier draft split this into `set_symbol_description` followed
    /// by `set_symbol_embedding`; if the second call failed the node carried
    /// the new description but its old, signature-derived embedding, and
    /// `only_missing` would skip it forever absent `--force`. Bundling fixes
    /// that class of inconsistency at the source.)
    pub async fn set_symbol_description_and_embedding(
        &self,
        qname: &str,
        description: &str,
        embedding: &[f32],
    ) -> Result<()> {
        let vec_lit = embedding
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(",");
        // Atomic write: description, embedding, AND description_source_hash
        // (mirrored from the Symbol's current body_hash) all land in a single
        // statement so a stale hash can never be observed against a fresh
        // description. Legacy Symbols whose body_hash hasn't been backfilled
        // get NULL source_hash, which the daemon's staleness pass treats as
        // "do not invalidate."
        let cypher = format!(
            "MATCH (s:Symbol {{qualified_name: '{q}'}}) \
             SET s.synthesized_description = '{d}', \
                 s.embedding = vecf32([{v}]), \
                 s.description_source_hash = s.body_hash",
            q = escape(qname),
            d = escape(description),
            v = vec_lit,
        );
        self.query(&cypher).await?;
        Ok(())
    }

    /// Returns the Symbol's current description plus the two hashes used to
    /// detect staleness. `None` only when the qname doesn't match a Symbol
    /// node — distinguishes "missing" from "exists but no description."
    pub async fn get_symbol_description(
        &self,
        qname: &str,
    ) -> Result<Option<SymbolDescriptionInfo>> {
        let cypher = format!(
            "MATCH (s:Symbol {{qualified_name: '{q}'}}) \
             RETURN coalesce(s.synthesized_description, '') AS desc, \
                    coalesce(s.body_hash, '') AS bh, \
                    coalesce(s.description_source_hash, '') AS sh",
            q = escape(qname),
        );
        let rows = self.query(&cypher).await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let mut iter = row.into_iter();
        let desc = match iter.next() {
            Some(FalkorValue::String(s)) if !s.is_empty() => Some(s),
            _ => None,
        };
        let body_hash = match iter.next() {
            Some(FalkorValue::String(s)) if !s.is_empty() => Some(s),
            _ => None,
        };
        let source_hash = match iter.next() {
            Some(FalkorValue::String(s)) if !s.is_empty() => Some(s),
            _ => None,
        };
        Ok(Some(SymbolDescriptionInfo {
            description: desc,
            body_hash,
            description_source_hash: source_hash,
        }))
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
