//! Shared helper for slicing a Symbol's body out of a source file and
//! computing a stable hash of the slice. Used by both:
//! - `pipeline.rs` — to compute the embedding input for cold index
//! - `synthesize.rs` — to build a Synthesizer prompt
//!
//! Same input → same output → same hash, so the pipeline's `body_hash`
//! and the synthesizer's prompt body agree byte-for-byte.

use std::collections::HashMap;

/// Cap on body lines included in the slice. Keeps embed input under
/// EmbeddingGemma's 2048-token context with margin and prevents a 5000-line
/// legacy file from dominating the embedding signal.
pub const BODY_LINE_CAP: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodySlice {
    pub text: String,
    pub hash: String,
}

/// Lazily read and cache file contents. `None` for files that don't exist
/// or aren't UTF-8 — both treated as "no body available."
pub type FileCache = HashMap<String, Option<Vec<String>>>;

/// Build the (signature + body slice) text used as the embed input AND
/// hash that text with blake3. Returns the same string and hash regardless
/// of which call site invokes it.
///
/// `start_line` and `end_line` are 1-indexed and inclusive (matching how
/// extractors report them).
pub fn signature_plus_body_slice(
    cache: &mut FileCache,
    signature: &str,
    file_path: &str,
    start_line: u32,
    end_line: u32,
) -> BodySlice {
    let body = read_body_slice(cache, file_path, start_line, end_line);
    let mut text = String::with_capacity(
        signature.len() + body.as_deref().map(str::len).unwrap_or(0) + 2,
    );
    text.push_str(signature);
    if let Some(b) = body {
        text.push_str("\n\n");
        text.push_str(&b);
    }
    let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
    BodySlice { text, hash }
}

/// Read up to `BODY_LINE_CAP` lines of the body and return them joined.
/// Truncation appends a marker so the embedder doesn't see an abrupt cut.
pub fn read_body_slice(
    cache: &mut FileCache,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir_for_test() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mycel-bs-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn signature_plus_body_slice_is_deterministic() {
        let dir = tempdir_for_test();
        let p = dir.join("a.rs");
        std::fs::write(&p, "fn a() {\n  let x = 1;\n  x + 1\n}\n").unwrap();
        let mut cache = FileCache::new();
        let s1 = signature_plus_body_slice(&mut cache, "fn a()", p.to_str().unwrap(), 1, 4);
        let s2 = signature_plus_body_slice(&mut cache, "fn a()", p.to_str().unwrap(), 1, 4);
        assert_eq!(s1, s2);
        assert!(s1.text.contains("fn a()"));
        assert!(s1.text.contains("let x = 1"));
        assert_eq!(s1.hash.len(), 64); // blake3 hex
    }

    #[test]
    fn missing_file_returns_signature_only() {
        let mut cache = FileCache::new();
        let s =
            signature_plus_body_slice(&mut cache, "fn missing()", "/no/such/file.rs", 1, 5);
        assert_eq!(s.text, "fn missing()");
        assert_eq!(s.hash.len(), 64);
    }

    #[test]
    fn truncation_marker_appended_when_over_cap() {
        let dir = tempdir_for_test();
        let p = dir.join("big.rs");
        let content: String = (1..=200).map(|i| format!("line{i}\n")).collect();
        std::fs::write(&p, &content).unwrap();
        let mut cache = FileCache::new();
        let s = signature_plus_body_slice(&mut cache, "fn big()", p.to_str().unwrap(), 1, 200);
        assert!(s.text.contains("(body truncated)"));
        assert!(s.text.starts_with("fn big()\n\nline1"));
    }

    #[test]
    fn read_body_slice_caps_long_bodies() {
        let dir = tempdir_for_test();
        let p = dir.join("big.rs");
        let content: String = (1..=200).map(|i| format!("line{i}\n")).collect();
        std::fs::write(&p, &content).unwrap();

        let mut cache = FileCache::new();
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

        let mut cache = FileCache::new();
        assert!(read_body_slice(&mut cache, path_str, 1, 10).is_none());

        std::fs::write(&path, "let x = 1;\n").unwrap();
        assert!(
            read_body_slice(&mut cache, path_str, 1, 10).is_none(),
            "negative result should be cached even after the file appears"
        );
    }
}
