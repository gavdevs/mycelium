use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QualifiedName(String);

impl QualifiedName {
    /// Constructs a QualifiedName, replacing any backtick with `_` to avoid
    /// breaking Cypher identifier-delimited string literals downstream.
    /// (Symbols with backticks in their name are vanishingly rare in practice.)
    pub fn new(s: impl Into<String>) -> Self {
        let s = s.into();
        if s.contains('`') {
            Self(s.replace('`', "_"))
        } else {
            Self(s)
        }
    }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Signature(pub String);

impl Signature {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Type,
    Component,
    Hook,
    Constant,
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
    Static,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub qualified_name: QualifiedName,
    pub kind: SymbolKind,
    pub file_path: Utf8PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: Signature,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsdoc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesized_description: Option<String>,
    pub exported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}
