//! Query implementations called by the mycel CLI.
//!
//! Each function is a thin wrapper around the underlying GraphClient method
//! so the CLI (mycel-cli, Task 7.2) doesn't have to know the graph crate's
//! method names. `find` is the only function that adds logic on top — it
//! embeds the user's query string before delegating to vector_search_top_k.

pub mod find;

use mycel_core::*;
use mycel_graph::GraphClient;

pub async fn callers(g: &GraphClient, sym: &str) -> Result<Vec<Symbol>>     { g.query_callers(sym).await }
pub async fn callees(g: &GraphClient, sym: &str) -> Result<Vec<Symbol>>     { g.query_callees(sym).await }
pub async fn definers(g: &GraphClient, name: &str) -> Result<Vec<Symbol>>   { g.query_definers(name).await }
pub async fn imports(g: &GraphClient, file: &str) -> Result<Vec<Symbol>>    { g.query_imports(file).await }
pub async fn uses(g: &GraphClient, ty: &str) -> Result<Vec<Symbol>>         { g.query_uses(ty).await }
pub async fn implements(g: &GraphClient, iface: &str) -> Result<Vec<Symbol>>{ g.query_implements(iface).await }

pub use find::find;
