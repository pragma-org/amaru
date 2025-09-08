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

use crate::stages::metrics::Metric;

pub struct PullMetrics {
    pub header_size_bytes: u64,
}

impl Metric for PullMetrics {
    fn record(&self, meter: &opentelemetry::metrics::Meter) {
        let counter = meter
            .u64_counter("cardano_node_header_total_bytes")
            .with_description("Total bytes of headers received")
            .with_unit("int")
            .build();

        counter.add(self.header_size_bytes, &[]);
    }
}
