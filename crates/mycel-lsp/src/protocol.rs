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

#[derive(Serialize)]
pub struct ResolveRefsReq<'a> {
    pub id: u64,
    pub op: &'static str, // always "resolve_refs_for_file"
    pub repo_root: &'a str,
    pub language: &'a str,
    pub path: &'a str,
    pub sites: Vec<RefSite>,
}

#[derive(Serialize, Clone, Debug)]
pub struct RefSite {
    pub line: u32, // 1-indexed (matches Edge::from_line)
    pub col: u32,  // 0-indexed (LSP convention)
    pub kind: String, // "calls" | "uses_type" | "implements"
}

#[derive(Deserialize, Debug)]
pub struct ResolveRefsResp {
    pub id: u64,
    #[serde(default)]
    pub refs: Vec<RawResolvedRef>,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct RawResolvedRef {
    pub from_path: String,
    pub from_line: u32,
    pub to_path: String, // file path, repo-relative (bridge does URI conversion)
    pub to_line: u32,    // 1-indexed
    pub kind: String,
}
