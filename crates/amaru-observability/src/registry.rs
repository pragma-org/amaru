// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Runtime schema registry for introspection and JSON schema generation
//!
//! This module provides runtime access to schema definitions for debugging,
//! JSON schema generation, and other introspection needs.

use inventory;

/// A schema entry in the runtime registry
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    pub path: &'static str,
    pub name: &'static str,
    pub target: &'static str,
    pub level: &'static str,
    pub description: &'static str,
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
