use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct EdgesForFileReq<'a> {
    pub id: u64,
    pub op: &'static str,
    pub repo_root: &'a str,
    pub language: &'a str,
    pub path: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct EdgesForFileResp {
    pub id: u64,
    #[serde(default)]
    pub edges: Vec<RawEdge>,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RawEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    #[serde(default)]
    pub source: Option<String>,
}
