// Copyright 2025 PRAGMA
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

use tracing_subscriber::EnvFilter;

/// Configuration for the collection of traces.
///
/// - The `EnvFilter` is the general configuration that specifies the required levels.
/// - `include_targets` and `exclude_targets` are used to further filter the collected records by their targets.
///     - If `include_targets` is set, only records with targets in that list will be collected.
///     - If `exclude_targets` is set, records with targets in that list will be excluded from collection.
///     - If both are set, `include_targets` is applied first, and then `exclude_targets` is applied to the result.
///
#[derive(Clone, Debug, Default)]
pub struct TraceCollectConfig {
    pub env_filter: EnvFilter,
    pub include_targets: Option<Vec<String>>,
    pub exclude_targets: Option<Vec<String>>,
}

impl TraceCollectConfig {
    pub fn with_include_targets(mut self, targets: &[&str]) -> Self {
        self.include_targets = Some(targets.iter().map(|s| (*s).into()).collect());
        self
    }

    pub fn with_exclude_targets(mut self, targets: &[&str]) -> Self {
        self.exclude_targets = Some(targets.iter().map(|s| (*s).into()).collect());
        self
    }

    pub fn with_env_filter(mut self, env_filter: EnvFilter) -> Self {
        self.env_filter = env_filter;
        self
    }
}
