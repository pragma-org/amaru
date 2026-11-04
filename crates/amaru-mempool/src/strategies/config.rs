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

/// Configuration shared by in-memory mempool instances.
#[derive(Debug, Clone)]
pub struct MempoolConfig {
    /// Maximum cumulative CBOR size of transactions held simultaneously, in bytes.
    pub max_bytes: u64,
}

/// Default mempool size: roughly twice the Conway max block body size (~90 KB).
/// This matches the 2× block-size convention used by the Cardano Haskell node.
const DEFAULT_MAX_BYTES: u64 = 180_224;

impl Default for MempoolConfig {
    fn default() -> Self {
        Self { max_bytes: DEFAULT_MAX_BYTES }
    }
}

impl MempoolConfig {
    /// Set the maximum cumulative transaction size accepted by the mempool.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}
