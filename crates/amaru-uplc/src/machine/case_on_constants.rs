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

use amaru_kernel::{ProtocolVersion, protocol_version::PROTOCOL_VERSION_11};

/// Which constants a `case` term may scrutinize, selected by protocol version alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseOnConstants {
    /// Every `case` on a constant fails.
    Unavailable,
    /// Unit, booleans, integers, lists and pairs may be scrutinized; `data` may not.
    #[default]
    NoData,
}

impl CaseOnConstants {
    pub fn new(protocol_version: ProtocolVersion) -> Self {
        if protocol_version >= PROTOCOL_VERSION_11 { Self::NoData } else { Self::Unavailable }
    }
}
