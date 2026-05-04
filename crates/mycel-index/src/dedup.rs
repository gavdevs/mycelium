use blake3::Hasher;
use camino::Utf8Path;
use mycel_core::Result;
use mycel_graph::GraphClient;

/// Bump this when ANY extractor's behavior changes (new SYMBOLS_QUERY pattern,
/// new edge kind extracted, .scm file edited, etc). Files indexed at a lower
/// version will be re-extracted on the next pass even when content_hash matches.
pub const EXTRACTOR_VERSION: u32 = 1;

pub fn content_hash(content: &str) -> String {
    let mut h = Hasher::new();
    h.update(content.as_bytes());
    h.finalize().to_hex().to_string()
}

/// True if the file's stored hash equals `hash` AND its stored extractor
/// version equals the current `EXTRACTOR_VERSION`. Either mismatch forces
/// re-extraction.
pub async fn is_unchanged(client: &GraphClient, path: &Utf8Path, hash: &str) -> Result<bool> {
    let Some(stored_hash) = client.file_content_hash(path).await? else {
        return Ok(false);
    };
    if stored_hash != hash {
        return Ok(false);
    }
    let stored_version = client.file_extractor_version(path).await?;
    Ok(stored_version == Some(EXTRACTOR_VERSION))
}

pub async fn upsert_file_record(
    client: &GraphClient,
    path: &Utf8Path,
    language: &str,
    hash: &str,
) -> Result<()> {
    client
        .upsert_file_record(path, language, hash, EXTRACTOR_VERSION)
        .await
}
