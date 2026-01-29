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

use crate::{Network, ScriptPurpose};

// TODO: Unnecessary AsIndex trait
//
// Probably unnecessary now that we've internalized types. Could be replaced with a From/Into
// instance.
pub trait AsIndex<T> {
    fn as_index(&self) -> T;
}

impl AsIndex<u32> for ScriptPurpose {
    fn as_index(&self) -> u32 {
        match self {
            Self::Spend => 0,
            Self::Mint => 1,
            Self::Cert => 2,
            Self::Reward => 3,
            Self::Vote => 4,
            Self::Propose => 5,
        }
    }
}

impl AsIndex<u8> for Network {
    fn as_index(&self) -> u8 {
        match self {
            Self::Testnet => 0,
            Self::Mainnet => 1,
            Self::Other(id) => *id,
        }
    }
}
