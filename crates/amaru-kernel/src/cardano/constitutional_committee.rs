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

use std::collections::BTreeMap;

use crate::{Epoch, RationalNumber, StakeCredential, cbor};

#[derive(Debug)]
pub struct ConstitutionalCommittee {
    pub members: BTreeMap<StakeCredential, Epoch>,
    pub threshold: RationalNumber,
}

impl<'d, C: cbor::HasProtocolVersion> cbor::decode::Decode<'d, C> for ConstitutionalCommittee {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(ConstitutionalCommittee { members: d.decode_with(ctx)?, threshold: d.decode_with(ctx)? })
        })
    }
}
