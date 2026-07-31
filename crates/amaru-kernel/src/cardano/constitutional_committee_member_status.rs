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

use crate::{Anchor, StakeCredential, cbor, utils::cbor::SerialisedAsArray};

#[derive(Debug)]
pub enum ConstitutionalCommitteeMemberStatus {
    DelegatedToHotCredential(StakeCredential),
    Resigned(Option<Anchor>),
}

impl<'d, C> cbor::decode::Decode<'d, C> for ConstitutionalCommitteeMemberStatus {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| match d.u8()? {
            0 => {
                assert_len(2)?;
                Ok(Self::DelegatedToHotCredential(d.decode_with(ctx)?))
            }
            1 => {
                assert_len(2)?;
                Ok(Self::Resigned(d.decode_with::<_, SerialisedAsArray<_>>(ctx)?.0))
            }
            t => Err(cbor::decode::Error::message(format!(
                "unexpected ConstitutionalCommitteeMemberStatus variant: {t}; expected 0 or 1."
            ))),
        })
    }
}
