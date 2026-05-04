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
        let extraction = extractor.extract(path, content)?;
        info!(
            file=%path,
            n_symbols = extraction.symbols.len(),
            n_edges = extraction.edges.len(),
            "extracted"
        );

        // 3. LSP refinement (if configured)
        let mut all_edges = extraction.edges.clone();
        if let Some(lsp) = &self.lsp {
            match lsp.refine(path, extractor.language_name(), &extraction).await {
                Ok(lsp_edges) => {
                    debug!(file=%path, n_lsp_edges = lsp_edges.len(), "lsp refined");
                    all_edges.extend(lsp_edges);
                }
                Err(e) => warn!(file=%path, error=%e, "lsp refine failed; proceeding with tree-sitter only"),
            }
        }

        // 4a. Graph upsert — SYMBOLS ONLY. Edges deferred to caller.
        self.graph.upsert_symbol_batch(&extraction.symbols).await?;

        // 5. Embed signatures (Phase 1: signature-only).
        //
        // CRITICAL: pair each symbol with its OWN embedding. The chunk-of-32
        // batching means we must zip the symbol-chunk with the text-chunk,
        // not zip `extraction.symbols.iter()` (which always restarts at 0)
        // with the most recent batch's vectors.
        if !extraction.symbols.is_empty() {
            let texts: Vec<&str> = extraction
                .symbols
                .iter()
                .map(|s| s.signature.as_str())
                .collect();
            for (sym_chunk, text_chunk) in extraction.symbols.chunks(32).zip(texts.chunks(32)) {
                let vecs = self.embedder.embed(text_chunk).await?;
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
                for (sym, vec) in sym_chunk.iter().zip(vecs.iter()) {
                    self.graph
                        .set_symbol_embedding(sym.qualified_name.as_str(), vec)
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
