use crate::GraphClient;
use crate::cypher::escape;
use camino::Utf8Path;
use falkordb::FalkorValue;
use mycel_core::*;

impl GraphClient {
    /// Returns the stored content_hash for a File node, if any.
    pub async fn file_content_hash(&self, path: &Utf8Path) -> Result<Option<String>> {
        let cypher = format!(
            "MATCH (f:File {{path: '{}'}}) RETURN f.content_hash",
            escape(path.as_str())
        );
        let rows = self.query(&cypher).await?;
        Ok(rows
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next())
            .and_then(|v| match v {
                FalkorValue::String(s) => Some(s),
                _ => None,
            }))
    }

    /// Returns the stored extractor_version for a File node, if any.
    pub async fn file_extractor_version(&self, path: &Utf8Path) -> Result<Option<u32>> {
        let cypher = format!(
            "MATCH (f:File {{path: '{}'}}) RETURN f.extractor_version",
            escape(path.as_str())
        );
        let rows = self.query(&cypher).await?;
        Ok(rows
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next())
            .and_then(|v| match v {
                FalkorValue::I64(n) if n >= 0 => Some(n as u32),
                _ => None,
            }))
    }

    pub async fn upsert_file_record(
        &self,
        path: &Utf8Path,
        language: &str,
        content_hash: &str,
        extractor_version: u32,
    ) -> Result<()> {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let cypher = format!(
            "MERGE (f:File {{path: '{path}'}}) SET f.language='{lang}', f.content_hash='{hash}', f.last_modified={ts}, f.extractor_version={ver}",
            path = escape(path.as_str()),
            lang = escape(language),
            hash = escape(content_hash),
            ts = now,
            ver = extractor_version,
        );
        self.query(&cypher).await?;
        Ok(())
    }

    /// Drop the File node entirely. Callers must run `prune_stale_symbols(path, &[])`
    /// first if they also want to wipe symbols owned by the file. Used when a
    /// file is deleted on disk.
    pub async fn delete_file_record(&self, path: &Utf8Path) -> Result<()> {
        let cypher = format!(
            "MATCH (f:File {{path: '{p}'}}) DETACH DELETE f",
            p = escape(path.as_str())
        );
        self.query(&cypher).await?;
        Ok(())
    }

    /// Writes embedding and body_hash atomically. The two fields must move
    /// together — body_hash describes which `signature + body slice` the
    /// embedding was computed against. Splitting the write would let the
    /// daemon observe a stale embedding paired with a fresh hash (or vice
    /// versa) and falsely conclude staleness was/wasn't present.
    pub async fn set_symbol_embedding_and_body_hash(
        &self,
        qname: &str,
        embedding: &[f32],
        body_hash: &str,
    ) -> Result<()> {
        let vec_lit = embedding
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let cypher = format!(
            "MATCH (s:Symbol {{qualified_name: '{q}'}}) \
             SET s.embedding = vecf32([{v}]), s.body_hash = '{h}'",
            q = escape(qname),
            v = vec_lit,
            h = escape(body_hash),
        );
        self.query(&cypher).await?;
        Ok(())
    }
}
