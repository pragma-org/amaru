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

use tracing::Level;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Debug, Default)]
pub struct TraceCollectConfig {
    pub include_targets: Option<Vec<String>>,
    pub exclude_targets: Option<Vec<String>>,
    pub env_filter: EnvFilter,
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

    #[expect(clippy::unwrap_used)]
    pub fn show_default_logs(self) -> Self {
        let filter = EnvFilter::from_default_env()
            .add_directive(Level::DEBUG.into())
            .add_directive("amaru::stages=trace".parse().unwrap())
            .add_directive("amaru::ledger=error".parse().unwrap())
            .add_directive("amaru_consensus=trace".parse().unwrap())
            .add_directive("amaru_protocols=trace".parse().unwrap())
            .add_directive("amaru_protocols::mux=info".parse().unwrap())
            .add_directive("pure_stage=info".parse().unwrap());

        self.with_env_filter(filter)
    }
}
