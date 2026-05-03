use serde::{Deserialize, Serialize};
use crate::file::RepoId;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexManifest {
    pub repo_id: RepoId,
    pub embedder_identity: String,
    pub embedder_dimension: u32,
    pub schema_version: u32,
    pub last_indexed_at: time::OffsetDateTime,
}
