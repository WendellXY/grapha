pub mod json;
pub mod sqlite;

use crate::graph::Graph;

/// Abstraction over graph storage backends.
pub trait Store {
    fn save(&self, graph: &Graph) -> anyhow::Result<()>;
    fn load(&self) -> anyhow::Result<Graph>;
    fn exists(&self) -> bool;
}
