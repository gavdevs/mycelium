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

        // 3. LSP refinement (if configured)
        let mut all_edges = extraction.edges.clone();
        if let Some(lsp) = &self.lsp {
            match lsp.refine(path, extractor.language_name(), &extraction).await {
                Ok(mut lsp_edges) => {
                    mycel_extract::resolve_same_file_edges(path, &mut lsp_edges, &extraction.symbols);
                    debug!(file=%path, n_lsp_edges = lsp_edges.len(), "lsp refined");
                    all_edges.extend(lsp_edges);
                }
                Err(e) => warn!(file=%path, error=%e, "lsp refine failed; proceeding with tree-sitter only"),
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

        // 5. Embed signature + body slice. Phase 2 (post-2026-05-05) cold
        //    index no longer runs the Synthesizer; the embedding source is
        //    the same string body_hash is computed over so embedding and
        //    hash always move together.
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
                for ((sym, vec), (_, hash)) in
                    sym_chunk.iter().zip(vecs.iter()).zip(prep_chunk.iter())
                {
                    self.graph
                        .set_symbol_embedding_and_body_hash(
                            sym.qualified_name.as_str(),
                            vec,
                            hash.as_str(),
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
