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

use amaru_kernel::{
    PlutusVersion, ProtocolVersion,
    protocol_version::{PROTOCOL_VERSION_9, PROTOCOL_VERSION_11},
};

/// Ledger builtin semantics variants. The semantic versioning is a little weird and are in-fact
/// devided in two groups:
///
/// - PlutusV1 & PlutusV2 semantics, which can be A, B or D;
/// - PlutusV3 semantics, which can be C or E;
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Default)]
pub enum Semantics {
    A,
    B,
    C,
    D,
    #[default]
    E,
}

impl Semantics {
    pub fn new(plutus_version: PlutusVersion, protocol_version: ProtocolVersion) -> Self {
        match plutus_version {
            PlutusVersion::V1 | PlutusVersion::V2 => {
                if protocol_version >= PROTOCOL_VERSION_11 {
                    Self::D
                } else if protocol_version >= PROTOCOL_VERSION_9 {
                    Self::B
                } else {
                    Self::A
                }
            }
            PlutusVersion::V3 => {
                if protocol_version >= PROTOCOL_VERSION_11 {
                    Self::E
                } else {
                    Self::C
                }
            }
        }
    }

    pub fn costs_strings_by_utf8_bytes(&self) -> bool {
        matches!(self, Self::D | Self::E)
    }

    pub fn cons_byte_string_range_checks(&self) -> bool {
        matches!(self, Self::C | Self::E)
    }
}
