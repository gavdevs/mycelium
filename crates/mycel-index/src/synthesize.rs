//! Phase 2 description synthesis.
//!
//! For each indexed Symbol, build a prompt from (signature + body slice +
//! 1-hop callers + 1-hop callees), call the configured Synthesizer to
//! produce a 1-3 sentence behavioral description, persist it on the Symbol
//! node, and re-embed using the description so vector search clusters by
//! behavior rather than by surface syntax.
//!
//! The pass is idempotent — symbols with an existing
//! `synthesized_description` are skipped unless `force` is set. Failures on
//! individual symbols are logged at WARN and do not abort the run; one bad
//! synth shouldn't poison the whole repo's Phase 2 lift.

use mycel_core::*;
use mycel_graph::GraphClient;
use mycel_graph::symbol::{SymbolForSynthesis, SynthesisContext};
use mycel_models::{Embedder, Synthesizer};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// How many caller/callee neighbors to include in the prompt per side.
/// Three each keeps the prompt small enough for `gemma4:e2b` without losing
/// the structural signal that justifies graph-augmented synthesis.
const NEIGHBOR_LIMIT: usize = 3;

/// Cap on body lines to include in the prompt. A 200-line function still
/// fits in `gemma4:e2b`'s context, but a 2000-line legacy file would blow
/// the budget — clip to the first `BODY_LINE_CAP` lines.
const BODY_LINE_CAP: usize = 60;

pub struct SynthesisOptions {
    /// Re-synthesize even if a description already exists.
    pub force: bool,
    /// Maximum number of symbols to process (None = no cap). Useful for
    /// quick smoke tests against a fresh cold index.
    pub limit: Option<usize>,
    /// Concurrency for synthesizer calls. Ollama's default is `num_parallel=4`;
    /// we match that to keep the GPU saturated without queueing.
    pub concurrency: usize,
}

impl Default for SynthesisOptions {
    fn default() -> Self {
        Self {
            force: false,
            limit: None,
            concurrency: 4,
        }
    }
}

pub struct SynthesisOutcome {
    pub considered: usize,
    pub synthesized: usize,
    pub skipped: usize,
    pub failed: usize,
}

pub async fn synthesize_descriptions(
    graph: &GraphClient,
    synthesizer: Arc<dyn Synthesizer>,
    embedder: Arc<dyn Embedder>,
    opts: SynthesisOptions,
) -> Result<SynthesisOutcome> {
    let mut symbols = graph.list_symbols_for_synthesis(!opts.force).await?;
    if let Some(limit) = opts.limit {
        symbols.truncate(limit);
    }
    let total = symbols.len();
    info!(
        n_symbols = total,
        force = opts.force,
        synthesizer = synthesizer.identity(),
        "starting description synthesis"
    );

    // File-content cache: avoid re-reading the same file once per symbol.
    // Most files have multiple symbols; a small HashMap pays for itself
    // immediately. UTF-8-only — non-text files are not in the symbol set
    // anyway.
    let mut file_cache: HashMap<String, Option<Vec<String>>> = HashMap::new();

    let mut synthesized = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    // Process in parallel batches sized to the concurrency cap. We do not
    // pipeline graph writes — they're cheap relative to the LLM call, and
    // serializing them keeps the FalkorDB connection's tokio Mutex from
    // becoming a bottleneck.
    use futures::stream::{StreamExt, iter};
    let concurrency = opts.concurrency.max(1);

    // Build prompts up-front (sequential disk reads + sequential graph
    // reads — both fast). Then run the synth+embed+write fan-out at the
    // configured concurrency.
    let mut work: Vec<(SymbolForSynthesis, String)> = Vec::with_capacity(total);
    for sym in symbols {
        if !opts.force && sym.has_description {
            skipped += 1;
            continue;
        }
        let body = read_body_slice(&mut file_cache, &sym.file_path, sym.start_line, sym.end_line);
        let context = match graph.query_synthesis_context(&sym.qualified_name, NEIGHBOR_LIMIT).await {
            Ok(c) => c,
            Err(e) => {
                warn!(qname = %sym.qualified_name, error = %e, "context fetch failed; using empty");
                SynthesisContext::default()
            }
        };
        let prompt = build_prompt(&sym, body.as_deref(), &context);
        work.push((sym, prompt));
    }

    let stream = iter(work.into_iter().map(|(sym, prompt)| {
        let synth = synthesizer.clone();
        let emb = embedder.clone();
        async move {
            let description = match synth.synthesize(&prompt).await {
                Ok(d) => d,
                Err(e) => {
                    warn!(qname = %sym.qualified_name, error = %e, "synthesize failed; skipping");
                    return Err(sym.qualified_name);
                }
            };
            let description = clean_description(&description);
            if description.is_empty() {
                warn!(qname = %sym.qualified_name, "synthesizer returned empty description");
                return Err(sym.qualified_name);
            }
            // Embed the description. This produces 1 vector — call embed()
            // with a single-element slice.
            let vec = match emb.embed(&[description.as_str()]).await {
                Ok(mut v) if !v.is_empty() => v.swap_remove(0),
                Ok(_) => {
                    warn!(qname = %sym.qualified_name, "embedder returned no vectors");
                    return Err(sym.qualified_name);
                }
                Err(e) => {
                    warn!(qname = %sym.qualified_name, error = %e, "re-embed failed; skipping");
                    return Err(sym.qualified_name);
                }
            };
            Ok((sym.qualified_name, description, vec))
        }
    }))
    .buffer_unordered(concurrency);

    let mut stream = std::pin::pin!(stream);
    while let Some(result) = stream.next().await {
        match result {
            Ok((qname, description, vec)) => {
                // Atomic write: description + embedding land together or not
                // at all. A separate description+embedding pair could leave
                // a node with a fresh description and a stale, signature-
                // derived embedding that subsequent `only_missing` filters
                // would silently skip.
                if let Err(e) = graph
                    .set_symbol_description_and_embedding(&qname, &description, &vec)
                    .await
                {
                    warn!(qname = %qname, error = %e, "write description+embedding failed");
                    failed += 1;
                    continue;
                }
                synthesized += 1;
                debug!(qname = %qname, "synthesized + re-embedded");
            }
            Err(_qname) => failed += 1,
        }
    }

    info!(
        considered = total,
        synthesized,
        skipped,
        failed,
        "description synthesis complete"
    );
    Ok(SynthesisOutcome {
        considered: total,
        synthesized,
        skipped,
        failed,
    })
}

/// Lazily read and cache the lines of a file. Returns None if the file is
/// missing or non-UTF-8 — both treated as "no body available," which is
/// fine: the prompt has signature + neighbors as fallback signal.
fn read_body_slice(
    cache: &mut HashMap<String, Option<Vec<String>>>,
    path: &str,
    start: u32,
    end: u32,
) -> Option<String> {
    let entry = cache.entry(path.to_string()).or_insert_with(|| {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.lines().map(|l| l.to_string()).collect())
    });
    let lines = entry.as_ref()?;
    let start_idx = (start.saturating_sub(1)) as usize;
    let end_idx = (end as usize).min(lines.len());
    if start_idx >= end_idx {
        return None;
    }
    let slice = &lines[start_idx..end_idx];
    let cap = BODY_LINE_CAP.min(slice.len());
    let mut body = slice[..cap].join("\n");
    if slice.len() > cap {
        body.push_str("\n// ... (body truncated)");
    }
    Some(body)
}

fn build_prompt(sym: &SymbolForSynthesis, body: Option<&str>, ctx: &SynthesisContext) -> String {
    let mut p = String::with_capacity(1024);
    p.push_str("You describe code symbols in 1-3 sentences for a code-search index. \
                Focus on what the symbol does and its role in the codebase. \
                Do not restate the signature, do not start with 'This function...', \
                do not include code blocks, do not output anything except the description.\n\n");
    p.push_str("Symbol: ");
    p.push_str(&sym.qualified_name);
    p.push_str("\nSignature: ");
    p.push_str(&sym.signature);
    p.push('\n');
    if let Some(body) = body {
        p.push_str("Body:\n");
        p.push_str(body);
        p.push('\n');
    }
    if !ctx.callers.is_empty() {
        p.push_str("Called by:\n");
        for c in &ctx.callers {
            p.push_str("- ");
            p.push_str(&c.qualified_name);
            p.push_str(": ");
            p.push_str(&c.signature);
            p.push('\n');
        }
    }
    if !ctx.callees.is_empty() {
        p.push_str("Calls:\n");
        for c in &ctx.callees {
            p.push_str("- ");
            p.push_str(&c.qualified_name);
            p.push_str(": ");
            p.push_str(&c.signature);
            p.push('\n');
        }
    }
    p.push_str("\nDescription:");
    p
}

/// Clean up small-model artifacts: leading/trailing whitespace, surrounding
/// quotes, and the chatty "Description:" prefix some models emit when they
/// echo the prompt. Keep this conservative — over-cleaning eats real text.
fn clean_description(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // Strip leading "Description:" if the model echoed it.
    for prefix in ["Description:", "description:"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
            break;
        }
    }
    // Strip surrounding double-quotes if the model wrapped its output.
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s = s[1..s.len() - 1].to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_description_strips_echoed_prefix() {
        assert_eq!(
            clean_description("Description: Validates a JWT token."),
            "Validates a JWT token."
        );
        assert_eq!(
            clean_description("description: Picks a tier."),
            "Picks a tier."
        );
    }

    #[test]
    fn clean_description_strips_surrounding_quotes() {
        assert_eq!(
            clean_description("\"Returns the canonical path.\""),
            "Returns the canonical path."
        );
    }

    #[test]
    fn clean_description_handles_combined_artifacts() {
        // Trim outer whitespace, strip the echoed prefix, then strip the
        // surrounding quotes the prefix was hiding. Both passes apply.
        assert_eq!(
            clean_description("   Description: \"Logs a warning.\"   "),
            "Logs a warning."
        );
    }

    #[test]
    fn clean_description_passthrough_for_clean_input() {
        assert_eq!(
            clean_description("Validates input and returns the parsed token."),
            "Validates input and returns the parsed token."
        );
    }

    #[test]
    fn build_prompt_includes_signature_and_neighbors() {
        let sym = SymbolForSynthesis {
            qualified_name: "foo::bar".into(),
            signature: "fn bar(x: u32) -> u32".into(),
            file_path: "foo.rs".into(),
            start_line: 1,
            end_line: 3,
            has_description: false,
        };
        let ctx = SynthesisContext {
            callers: vec![mycel_graph::symbol::SymbolNeighbor {
                qualified_name: "foo::caller".into(),
                signature: "fn caller()".into(),
            }],
            callees: vec![mycel_graph::symbol::SymbolNeighbor {
                qualified_name: "foo::callee".into(),
                signature: "fn callee(y: u32)".into(),
            }],
        };
        let prompt = build_prompt(&sym, Some("fn bar(x: u32) -> u32 { x + 1 }"), &ctx);
        // Sanity checks — the prompt should reference the symbol, both
        // neighbors, and end with the "Description:" cue that elicits the
        // completion. Don't assert on the exact phrasing of the system
        // instructions; that's stylistic and free to evolve.
        assert!(prompt.contains("foo::bar"));
        assert!(prompt.contains("fn bar(x: u32) -> u32"));
        assert!(prompt.contains("foo::caller"));
        assert!(prompt.contains("foo::callee"));
        assert!(prompt.trim_end().ends_with("Description:"));
    }

    #[test]
    fn read_body_slice_caps_long_bodies() {
        let dir = tempdir_for_test();
        let p = dir.join("big.rs");
        let content: String = (1..=200)
            .map(|i| format!("line{i}\n"))
            .collect();
        std::fs::write(&p, &content).unwrap();

        let mut cache = HashMap::new();
        let body = read_body_slice(&mut cache, p.to_str().unwrap(), 1, 200).unwrap();
        // Should include the truncation marker since 200 > BODY_LINE_CAP.
        assert!(body.contains("(body truncated)"));
        // First line preserved.
        assert!(body.starts_with("line1"));
    }

    #[test]
    fn read_body_slice_handles_missing_file_and_caches_negative() {
        let dir = tempdir_for_test();
        let path = dir.join("not-yet.rs");
        let path_str = path.to_str().unwrap();

        let mut cache = HashMap::new();
        // First call: file doesn't exist, expect None.
        assert!(read_body_slice(&mut cache, path_str, 1, 10).is_none());

        // Now create the file. A non-caching implementation would re-read
        // it and return Some(...) on the second call; the caching one
        // remembers None and short-circuits.
        std::fs::write(&path, "let x = 1;\n").unwrap();
        assert!(
            read_body_slice(&mut cache, path_str, 1, 10).is_none(),
            "negative result should be cached even after the file appears"
        );
    }

    fn tempdir_for_test() -> std::path::PathBuf {
        // Use the OS temp dir + a unique-ish suffix; avoids pulling in the
        // tempfile crate just for these unit tests. Cleanup is best-effort.
        let dir = std::env::temp_dir().join(format!(
            "mycel-synth-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
