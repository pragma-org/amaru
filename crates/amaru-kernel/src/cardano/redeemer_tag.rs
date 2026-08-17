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

use std::fmt;

use crate::cbor;

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode,
)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
#[cbor(index_only)]
pub enum RedeemerTag {
    #[n(0)]
    Spend,
    #[n(1)]
    Mint,
    #[n(2)]
    Cert,
    #[n(3)]
    Reward,
    #[n(4)]
    Vote,
    #[n(5)]
    Propose,
}

impl fmt::Display for RedeemerTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Spend => "Spend",
            Self::Mint => "Mint",
            Self::Cert => "Cert",
            Self::Reward => "Reward",
            Self::Vote => "Vote",
            Self::Propose => "Propose",
        })
    }
}
