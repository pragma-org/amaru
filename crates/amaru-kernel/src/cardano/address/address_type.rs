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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum AddressType {
    /// Base Key/Key Shelley address
    Type0 = 0,
    /// Base Script/Key Shelley address
    Type1 = 1,
    /// Base Key/Script Shelley address
    Type2 = 2,
    /// Base Script/Script Shelley address
    Type3 = 3,
    /// Pointer Key Shelley address deprecated since Conway
    Type4 = 4,
    /// Pointer Script Shelley address deprecated since Conway
    Type5 = 5,
    /// Payment Key Shelley address (a.k.a. Key Enterprise address)
    Type6 = 6,
    /// Payment Script Shelley address (a.k.a Script Enterprise address)
    Type7 = 7,
    /// Byron / Bootstrap address
    Type8 = 8,
    /// Stake Key Shelley address
    Type14 = 14,
    /// Stake Script Shelley address
    Type15 = 15,
}

impl AddressType {
    pub fn try_from_header_byte(header_byte: u8) -> Option<Self> {
        match (header_byte & 0b1111_0000) >> 4 {
            0 => Some(Self::Type0),
            1 => Some(Self::Type1),
            2 => Some(Self::Type2),
            3 => Some(Self::Type3),
            4 => Some(Self::Type4),
            5 => Some(Self::Type5),
            6 => Some(Self::Type6),
            7 => Some(Self::Type7),
            8 => Some(Self::Type8),
            14 => Some(Self::Type14),
            15 => Some(Self::Type15),
            _ => None,
        }
    }
}
