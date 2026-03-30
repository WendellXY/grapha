pub mod json;
pub mod sqlite;

use grapha_core::graph::Graph;

/// Abstraction over graph storage backends.
pub trait Store {
    fn save(&self, graph: &Graph) -> anyhow::Result<()>;
    fn load(&self) -> anyhow::Result<Graph>;
}
