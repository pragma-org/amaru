//! Runtime schema registry for introspection and JSON schema generation
//!
//! This module provides runtime access to schema definitions for debugging,
//! JSON schema generation, and other introspection needs.

use inventory;

/// A schema entry in the runtime registry
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    pub path: &'static str,
    pub target: &'static str,
    pub required_fields: &'static [(&'static str, &'static str)],
    pub optional_fields: &'static [(&'static str, &'static str)],
}

inventory::collect!(SchemaEntry);

impl SchemaEntry {
    /// Get all registered schemas
    pub fn all() -> Vec<SchemaEntry> {
        inventory::iter::<SchemaEntry>().cloned().collect()
    }

    /// Find a schema by path
    pub fn find(path: &str) -> Option<SchemaEntry> {
        inventory::iter::<SchemaEntry>()
            .find(|s| s.path == path)
            .cloned()
    }

    /// Get the number of registered schemas
    pub fn count() -> usize {
        inventory::iter::<SchemaEntry>().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_collection() {
        // This test verifies that inventory can be used at runtime
        let _count = SchemaEntry::count();
        let _all = SchemaEntry::all();
    }
}
