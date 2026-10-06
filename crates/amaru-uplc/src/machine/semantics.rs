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

use amaru_kernel::{PlutusVersion, ProtocolVersion, protocol_version::PROTOCOL_VERSION_11};

use super::MachineVersion;

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
                } else {
                    Self::B
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

    /// Whether a program declaring this UPLC language version may be evaluated.
    pub fn supports_program_version(&self, version: MachineVersion) -> bool {
        match self {
            Self::A | Self::B => version == MachineVersion::V1_0_0,
            Self::C | Self::D | Self::E => version == MachineVersion::V1_0_0 || version == MachineVersion::V1_1_0,
        }
    }

    pub fn costs_strings_by_utf8_bytes(&self) -> bool {
        matches!(self, Self::D | Self::E)
    }

    pub fn cons_byte_string_range_checks(&self) -> bool {
        matches!(self, Self::C | Self::E)
    }

    /// Whether arithmetic builtins reject integers outside Cardano's signed 262144-bit range.
    pub fn enforces_integer_bounds(&self) -> bool {
        matches!(self, Self::D | Self::E)
    }

    /// Whether `shiftByteString` and `rotateByteString` reject shift amounts outside the signed 64-bit range.
    pub fn bounds_shift_amount_to_int64(&self) -> bool {
        matches!(self, Self::D | Self::E)
    }

    /// Whether `writeBits` rejects inputs longer than `WRITE_BITS_MAXIMUM_INPUT_LENGTH` bytes.
    pub fn bounds_write_bits_input_length(&self) -> bool {
        matches!(self, Self::D | Self::E)
    }
}
