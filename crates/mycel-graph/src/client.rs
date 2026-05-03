use falkordb::{FalkorAsyncClient, FalkorClientBuilder, FalkorConnectionInfo, FalkorValue};
use mycel_core::{MycelError, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// A connected handle to a FalkorDB graph for a specific repo.
///
/// All FalkorDB operations issued through `GraphClient` are raw Cypher
/// against the underlying `falkordb` crate's query interface.
// TODO(perf): FalkorAsyncClient owns an internal connection pool, so the
// outer Arc<Mutex<...>> wrapper serializes every query through one tokio
// Mutex before the pool is even consulted. Acceptable for v0 (Phase 1 is
// single-threaded indexing); revisit when batching ergonomics are figured
// out for Tasks 2.3/2.4/6.1.
#[derive(Clone)]
pub struct GraphClient {
    inner: Arc<Mutex<FalkorAsyncClient>>,
    pub graph_name: String,
}

impl GraphClient {
    /// Connect to FalkorDB and run migrations.
    pub async fn connect(url: &str, graph_name: &str) -> Result<Self> {
        let info: FalkorConnectionInfo = url
            .try_into()
            .map_err(|e| MycelError::Graph(format!("invalid url {url}: {e:?}")))?;
        let client = FalkorClientBuilder::new_async()
            .with_connection_info(info)
            .build()
            .await
            .map_err(|e| MycelError::Graph(format!("connect: {e}")))?;
        info!(graph = %graph_name, "connected to FalkorDB");
        let me = Self {
            inner: Arc::new(Mutex::new(client)),
            graph_name: graph_name.into(),
        };
        crate::migrations::run_all(&me).await?;
        Ok(me)
    }

    /// Run a Cypher query against this graph and return raw rows.
    ///
    /// Implementation note: `LazyResultSet` from the falkordb crate is a
    /// synchronous `Iterator`, not an async `Stream`. Do **not** add `.await`
    /// to `.collect()`. Also: the `QueryBuilder` returned by `graph.query(...)`
    /// holds a mutable borrow of `graph` for its lifetime — keep `.execute()`
    /// in the same expression and don't store the builder in a local.
    pub async fn query(&self, cypher: &str) -> Result<Vec<Vec<FalkorValue>>> {
        debug!(cypher = %cypher, "issuing cypher");
        let guard = self.inner.lock().await;
        let mut graph = guard.select_graph(&self.graph_name);
        let res = graph
            .query(cypher)
            .execute()
            .await
            .map_err(|e| MycelError::Graph(format!("{e}")))?;
        let rows: Vec<Vec<FalkorValue>> = res.data.collect();
        Ok(rows)
    }

    /// Run a Cypher query against the meta graph (`mycel:meta`).
    pub async fn meta_query(&self, cypher: &str) -> Result<Vec<Vec<FalkorValue>>> {
        debug!(cypher = %cypher, target = "mycel:meta", "issuing meta cypher");
        let guard = self.inner.lock().await;
        let mut graph = guard.select_graph("mycel:meta");
        let res = graph
            .query(cypher)
            .execute()
            .await
            .map_err(|e| MycelError::Graph(format!("{e}")))?;
        let rows: Vec<Vec<FalkorValue>> = res.data.collect();
        Ok(rows)
    }

    /// Returns the IDs of migrations already applied to *this* per-repo graph
    /// (records live in the meta graph but are scoped by the graph name).
    pub async fn applied_migrations(&self) -> Result<Vec<String>> {
        let cypher = format!(
            "MATCH (m:MigrationRecord {{graph_name: '{}'}}) RETURN m.migration_id",
            crate::cypher::escape(&self.graph_name)
        );
        let rows = self.meta_query(&cypher).await?;
        let mut ids = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(FalkorValue::String(s)) = row.into_iter().next() {
                ids.push(s);
            }
        }
        Ok(ids)
    }
}
