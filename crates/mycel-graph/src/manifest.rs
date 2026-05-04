use crate::GraphClient;
use crate::cypher::escape;
use falkordb::FalkorValue;
use mycel_core::*;

impl GraphClient {
    pub async fn write_manifest(&self, m: &IndexManifest) -> Result<()> {
        let cypher = format!(
            r#"MERGE (i:IndexManifest {{repo_id: '{repo}'}})
               SET i.embedder_identity = '{embed_id}',
                   i.embedder_dimension = {dim},
                   i.schema_version = {schema},
                   i.last_indexed_at = {ts}"#,
            repo = escape(m.repo_id.as_str()),
            embed_id = escape(&m.embedder_identity),
            dim = m.embedder_dimension,
            schema = m.schema_version,
            ts = m.last_indexed_at.unix_timestamp(),
        );
        self.meta_query(&cypher).await?;
        Ok(())
    }

    pub async fn read_manifest(&self, repo_id: &str) -> Result<Option<IndexManifest>> {
        let cypher = format!(
            "MATCH (i:IndexManifest {{repo_id: '{repo}'}}) \
             RETURN i.repo_id, i.embedder_identity, i.embedder_dimension, i.schema_version, i.last_indexed_at",
            repo = escape(repo_id),
        );
        let rows = self.meta_query(&cypher).await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let mut iter = row.into_iter();
        let repo = match iter.next() {
            Some(FalkorValue::String(s)) => s,
            _ => return Ok(None),
        };
        let embed_id = match iter.next() {
            Some(FalkorValue::String(s)) => s,
            _ => return Ok(None),
        };
        let dim = match iter.next() {
            Some(FalkorValue::I64(n)) => n as u32,
            _ => return Ok(None),
        };
        let schema = match iter.next() {
            Some(FalkorValue::I64(n)) => n as u32,
            _ => return Ok(None),
        };
        let ts = match iter.next() {
            Some(FalkorValue::I64(n)) => n,
            _ => return Ok(None),
        };
        Ok(Some(IndexManifest {
            repo_id: RepoId::new(repo),
            embedder_identity: embed_id,
            embedder_dimension: dim,
            schema_version: schema,
            last_indexed_at: time::OffsetDateTime::from_unix_timestamp(ts)
                .map_err(|e| MycelError::Graph(e.to_string()))?,
        }))
    }
}
