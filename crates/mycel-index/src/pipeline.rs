use camino::Utf8Path;
use mycel_core::*;
use mycel_extract::for_language;
use mycel_graph::GraphClient;
use mycel_lsp::MultilspyResolver;
use mycel_models::Embedder;
use std::sync::Arc;
use tracing::{debug, info, warn};

pub struct Indexer {
    pub graph: GraphClient,
    pub lsp: Option<Arc<MultilspyResolver>>,
    pub embedder: Arc<dyn Embedder>,
}

impl Indexer {
    /// Like `index_file` but returns the edges to be upserted by the caller
    /// instead of writing them. Used by `index_repo` so all symbols across the
    /// repo are present before any edges are written (eliminates the
    /// cross-file-endpoint silent-drop in upsert_edge_batch).
    ///
    /// Returns `Ok(None)` if the file was skipped (dedup or no extractor),
    /// `Ok(Some(edges))` if the file was indexed.
    pub async fn index_file_collect_edges(
        &self,
        path: &Utf8Path,
        content: &str,
    ) -> Result<Option<Vec<Edge>>> {
        // 1. Content-hash dedup
        let hash = crate::dedup::content_hash(content);
        if crate::dedup::is_unchanged(&self.graph, path, &hash).await? {
            debug!(file=%path, "unchanged, skipping");
            return Ok(None);
        }

        // 2. Pick extractor
        let Some(extractor) = for_language(path) else {
            debug!(file=%path, "no extractor for extension");
            return Ok(None);
        };
        let mut extraction = extractor.extract(path, content)?;
        // Tree-sitter has no symbol table; same-file calls come out with bare
        // callee names. Resolve them to full qnames so edges actually land at
        // upsert time. (Cross-file resolution is LSP territory.)
        mycel_extract::resolve_same_file_edges(path, &mut extraction.edges, &extraction.symbols);
        info!(
            file=%path,
            n_symbols = extraction.symbols.len(),
            n_edges = extraction.edges.len(),
            "extracted"
        );

        // 3. Cross-file resolution via LSP (if configured).
        //
        //    Tree-sitter emits Calls / UsesType / Implements edges with bare
        //    callee names and a `from_line` coord. `resolve_same_file_edges`
        //    above handles the same-file case; what remains is cross-file —
        //    LSP territory. We:
        //      a) Collect a RefSite per qualifying tree-sitter edge.
        //      b) Build a `from_line -> from_qname` map (the LSP response
        //         carries only file/line coords, not qnames).
        //      c) Call `resolve_refs`. On error, log WARN and continue with
        //         tree-sitter edges only.
        //      d) For each resolved ref, look up the to-side qname via the
        //         graph (`symbol_containing`). If the definition is outside
        //         the indexed surface, drop the edge.
        let mut all_edges = extraction.edges.clone();
        if let Some(lsp) = &self.lsp {
            use mycel_lsp::protocol::RefSite;
            use std::collections::HashMap;

            let mut from_line_to_qname: HashMap<u32, String> = HashMap::new();
            let mut sites: Vec<RefSite> = Vec::new();
            for e in &all_edges {
                let kind_str = match e.kind {
                    EdgeKind::Calls => "calls",
                    EdgeKind::UsesType => "uses_type",
                    EdgeKind::Implements => "implements",
                    _ => continue,
                };
                let Some(line) = e.from_line else { continue };
                // Skip edges whose target already resolved to a qname
                // (`<file>::<symbol>`) — these are same-file resolutions
                // produced by `resolve_same_file_edges`. Sending them to LSP
                // wastes a round-trip and column 0 returns nothing anyway.
                if e.to.contains("::") {
                    continue;
                }
                // Assumes one enclosing-symbol per from_line: a single source line in a single
                // file is owned by one tree-sitter parent (e.g., `foo().bar().baz()` on one
                // line all share an enclosing fn). If a future extractor emits multiple
                // distinct `from` qnames for the same line — possible with closures or
                // inline lambdas — promote this to HashMap<u32, Vec<String>> and disambiguate
                // at apply time. For today's TS+Rust extractors this is safe.
                from_line_to_qname
                    .entry(line)
                    .or_insert_with(|| e.from.clone());
                // Column accuracy matters: tsserver's request_definition returns
                // empty when the cursor lands on whitespace. We locate the bare
                // callee name in the source line to seed an identifier-bearing
                // column. Falls back to 0 if not found (same as the no-column
                // baseline; the LSP call will likely return empty).
                let col = locate_token_col(content, line, &e.to);
                sites.push(RefSite {
                    line,
                    col,
                    kind: kind_str.into(),
                });
            }

            if !sites.is_empty() {
                match lsp
                    .resolve_refs(path, extractor.language_name(), sites)
                    .await
                {
                    Ok(refs) => {
                        let mut resolved = 0usize;
                        for r in refs {
                            if r.from_path != path.as_str() {
                                tracing::warn!(
                                    file = %path,
                                    bridge_from_path = %r.from_path,
                                    "resolve_refs response carries a from_path that doesn't match the request; skipping",
                                );
                                continue;
                            }
                            let Some(from_qname) = from_line_to_qname.get(&r.from_line) else {
                                // LSP returned a site we didn't seed — shouldn't
                                // happen, but skip rather than fabricate a from.
                                continue;
                            };
                            let to_qname = match self
                                .graph
                                .symbol_containing(&r.to_path, r.to_line)
                                .await
                            {
                                Ok(Some(q)) => q,
                                Ok(None) => continue, // definition outside indexed surface
                                Err(e) => {
                                    warn!(
                                        file=%path,
                                        to_path = %r.to_path,
                                        to_line = r.to_line,
                                        error=%e,
                                        "symbol_containing failed; dropping resolved ref",
                                    );
                                    continue;
                                }
                            };
                            let kind = match r.kind.as_str() {
                                "calls" => EdgeKind::Calls,
                                "uses_type" => EdgeKind::UsesType,
                                "implements" => EdgeKind::Implements,
                                _ => continue,
                            };
                            all_edges.push(Edge {
                                from: from_qname.clone(),
                                to: to_qname,
                                kind,
                                source: EdgeSource::Lsp,
                                from_line: Some(r.from_line),
                            });
                            resolved += 1;
                        }
                        debug!(file=%path, n_resolved = resolved, "lsp resolve_refs");
                    }
                    Err(e) => warn!(
                        file=%path,
                        error=%e,
                        "lsp resolve_refs failed; proceeding with tree-sitter edges only",
                    ),
                }
            }
        }

        // 4a. Graph upsert — SYMBOLS ONLY. Edges deferred to caller.
        //     Prune symbols that were owned by this file in a prior index pass
        //     but are no longer present in the new extraction (function deleted,
        //     renamed, or file shrunk). Without this, ghost symbols accumulate
        //     and surface in `definers`/`find` queries forever.
        let kept_qnames: Vec<&str> = extraction
            .symbols
            .iter()
            .map(|s| s.qualified_name.as_str())
            .collect();
        let pruned = self
            .graph
            .prune_stale_symbols(path.as_str(), &kept_qnames)
            .await?;
        if pruned > 0 {
            debug!(file=%path, pruned, "pruned stale symbols");
        }
        self.graph.upsert_symbol_batch(&extraction.symbols).await?;

        // 5. Embed signature + body slice; handle description staleness via
        //    a single batched pre-read.
        //
        //    Sequence:
        //      a) Compute (text, hash) for every Symbol's body slice.
        //      b) Single Cypher round-trip: fetch description_source_hash for
        //         this file's Symbols.
        //      c) Batched embed (chunk-of-32, mirroring the cold-index shape).
        //      d) Per-Symbol write: if (b) returned a hash AND it diverges
        //         from the new body_hash, clear+reembed; otherwise just
        //         write embedding+body_hash.
        if !extraction.symbols.is_empty() {
            use crate::body_slice::{FileCache, signature_plus_body_slice};
            let mut cache = FileCache::new();
            let prepared: Vec<(String, String)> = extraction
                .symbols
                .iter()
                .map(|s| {
                    let bs = signature_plus_body_slice(
                        &mut cache,
                        s.signature.as_str(),
                        s.file_path.as_str(),
                        s.start_line,
                        s.end_line,
                    );
                    (bs.text, bs.hash)
                })
                .collect();

            let qnames: Vec<&str> = extraction
                .symbols
                .iter()
                .map(|s| s.qualified_name.as_str())
                .collect();
            let stored_source_hashes = self
                .graph
                .description_source_hashes_for_batch(&qnames)
                .await?;

            let mut all_vecs: Vec<Vec<f32>> = Vec::with_capacity(prepared.len());
            for (sym_chunk, prep_chunk) in
                extraction.symbols.chunks(32).zip(prepared.chunks(32))
            {
                let text_chunk: Vec<&str> = prep_chunk.iter().map(|(t, _)| t.as_str()).collect();
                let vecs = self.embedder.embed(&text_chunk).await?;
                if vecs.len() != sym_chunk.len() {
                    return Err(MycelError::Model {
                        provider: self.embedder.identity().into(),
                        message: format!(
                            "expected {} embeddings, got {}",
                            sym_chunk.len(),
                            vecs.len()
                        ),
                    });
                }
                all_vecs.extend(vecs);
            }
            debug_assert_eq!(all_vecs.len(), prepared.len());

            for ((sym, (_, new_hash)), vec) in extraction
                .symbols
                .iter()
                .zip(prepared.iter())
                .zip(all_vecs.iter())
            {
                let stale = stored_source_hashes
                    .get(sym.qualified_name.as_str())
                    .is_some_and(|src| src.as_str() != new_hash.as_str());
                if stale {
                    self.graph
                        .clear_symbol_description_and_reembed(
                            sym.qualified_name.as_str(),
                            new_hash,
                            vec,
                        )
                        .await?;
                } else {
                    self.graph
                        .set_symbol_embedding_and_body_hash(
                            sym.qualified_name.as_str(),
                            vec,
                            new_hash,
                        )
                        .await?;
                }
            }
        }

        // 6. Update File record
        crate::dedup::upsert_file_record(&self.graph, path, extractor.language_name(), &hash)
            .await?;

        Ok(Some(all_edges))
    }

    /// Index a single file, including writing its edges. Used by the daemon's
    /// per-file change events. For repo-wide indexing prefer `index_repo`,
    /// which defers all edges to a final batch so cross-file endpoints resolve.
    pub async fn index_file(&self, path: &Utf8Path, content: &str) -> Result<()> {
        if let Some(edges) = self.index_file_collect_edges(path, content).await? {
            self.graph.upsert_edge_batch(&edges).await?;
        }
        Ok(())
    }

    /// Wipe every Symbol owned by `path` and the File node itself. Used by the
    /// daemon when a watcher event indicates the file no longer exists on disk.
    /// Without this, deleting a source file leaves all its symbols and edges as
    /// ghosts in the graph.
    pub async fn forget_file(&self, path: &Utf8Path) -> Result<()> {
        let pruned = self.graph.prune_stale_symbols(path.as_str(), &[]).await?;
        self.graph.delete_file_record(path).await?;
        if pruned > 0 {
            info!(file=%path, pruned, "removed deleted file from graph");
        }
        Ok(())
    }

    pub async fn index_repo(&self, root: &Utf8Path) -> Result<usize> {
        let mut count = 0;
        let mut deferred_edges: Vec<Edge> = Vec::new();
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path: camino::Utf8PathBuf = match entry.path().to_path_buf().try_into() {
                Ok(p) => p,
                Err(_) => {
                    warn!(path = %entry.path().display(), "non-utf8 path, skipping");
                    continue;
                }
            };
            // skip target/, node_modules/, .git/
            if path.as_str().contains("/target/")
                || path.as_str().contains("/node_modules/")
                || path.as_str().contains("/.git/")
            {
                continue;
            }
            if for_language(&path).is_none() {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(content) => match self.index_file_collect_edges(&path, &content).await {
                    Ok(Some(edges)) => {
                        deferred_edges.extend(edges);
                        count += 1;
                    }
                    Ok(None) => {
                        // Dedup-skipped or no extractor; symbols already exist
                        // in the graph from a prior run, so cross-file edges
                        // pointing here will still resolve.
                    }
                    Err(e) => warn!(file=%path, error=%e, "index_file failed"),
                },
                Err(e) => warn!(file=%path, error=%e, "read failed"),
            }
        }
        // Final batch: all symbols across the repo are now present, so edges
        // referencing any file's symbols will resolve at MATCH time.
        if !deferred_edges.is_empty() {
            info!(n_edges = deferred_edges.len(), "writing deferred edges");
            self.graph.upsert_edge_batch(&deferred_edges).await?;
        }

        // Write/refresh the index manifest so downstream vector_search_top_k can
        // detect embedder mismatches. Use the graph_name as repo_id (Task 2.4
        // vector_search reads manifest by graph_name).
        let manifest = IndexManifest {
            repo_id: RepoId::new(self.graph.graph_name.clone()),
            embedder_identity: self.embedder.identity().to_string(),
            embedder_dimension: self.embedder.dimension(),
            schema_version: SCHEMA_VERSION,
            last_indexed_at: time::OffsetDateTime::now_utc(),
        };
        self.graph.write_manifest(&manifest).await?;

        Ok(count)
    }
}

/// Return the 0-indexed column of the first word-boundary occurrence of
/// `token` on `line` (1-indexed) in `content`. Returns 0 if not found —
/// caller-side LSP will return empty for that site, which is the same
/// behavior as the previous "always col 0" baseline.
///
/// Word-boundary: the char before must not be alphanumeric/underscore and
/// the char after must not be alphanumeric/underscore. This avoids matching
/// `add` inside `padded`.
fn locate_token_col(content: &str, line: u32, token: &str) -> u32 {
    if token.is_empty() {
        return 0;
    }
    let line_idx = line.saturating_sub(1) as usize;
    let Some(line_str) = content.lines().nth(line_idx) else {
        return 0;
    };
    let bytes = line_str.as_bytes();
    let tok = token.as_bytes();
    let mut i = 0usize;
    while i + tok.len() <= bytes.len() {
        if bytes[i..i + tok.len()] == *tok {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + tok.len() == bytes.len() || !is_ident_byte(bytes[i + tok.len()]);
            if before_ok && after_ok {
                return i as u32;
            }
        }
        i += 1;
    }
    0
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_token_col_finds_word_boundary() {
        let content = "    return `Hi ${name}, sum is ${add(1, 2)}`;\n";
        let col = locate_token_col(content, 1, "add");
        assert_eq!(col, 33, "expected col 33 for `add` in template literal");
    }

    #[test]
    fn locate_token_col_skips_substring_match() {
        // `add` inside `padded` must not match.
        let content = "let padded = 1;\n";
        let col = locate_token_col(content, 1, "add");
        assert_eq!(col, 0, "should not match `add` inside `padded`");
    }

    #[test]
    fn locate_token_col_returns_zero_when_line_missing() {
        let content = "only one line\n";
        assert_eq!(locate_token_col(content, 99, "missing"), 0);
    }
}
