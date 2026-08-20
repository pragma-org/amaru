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

use crate::{StakeCredential, cbor, utils::cbor::SerialisedAsArray};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConstitutionalCommitteeMemberStatus {
    DelegatedToHotCredential(StakeCredential),

    // NOTE: ignored anchor on 'Resigned' status
    //
    // This contains an anchor on-the-wire. We ignore it entirely in the decoded version, although
    // we make sure it is properly decoded if present. This is because the anchor:
    //
    // 1. is completely useless to the ledger
    // 2. prevents the type from being `Copy`
    Resigned,
}

impl TryFrom<ConstitutionalCommitteeMemberStatus> for StakeCredential {
    type Error = ();
    fn try_from(status: ConstitutionalCommitteeMemberStatus) -> Result<Self, Self::Error> {
        match status {
            ConstitutionalCommitteeMemberStatus::DelegatedToHotCredential(hot_credential) => Ok(hot_credential),
            ConstitutionalCommitteeMemberStatus::Resigned => Err(()),
        }
    }
}

impl From<StakeCredential> for ConstitutionalCommitteeMemberStatus {
    fn from(hot_credential: StakeCredential) -> Self {
        Self::DelegatedToHotCredential(hot_credential)
    }
}

impl ConstitutionalCommitteeMemberStatus {
    /// Extract the hot credential from a status, if any is set.
    pub fn as_hot_credential(&self) -> Option<&StakeCredential> {
        match self {
            Self::DelegatedToHotCredential(hot_credential) => Some(hot_credential),
            Self::Resigned => None,
        }
    }
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for ConstitutionalCommitteeMemberStatus {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;

        match self {
            Self::DelegatedToHotCredential(hot_credential) => {
                e.u8(0)?;
                e.encode_with(hot_credential, ctx)?;
            }
            Self::Resigned => {
                e.u8(1)?;
                e.encode_with(SerialisedAsArray(None::<crate::Anchor>), ctx)?;
            }
        }

        Ok(())
    }
}

impl<'d, C: cbor::HasProtocolVersion> cbor::decode::Decode<'d, C> for ConstitutionalCommitteeMemberStatus {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| match d.u8()? {
            0 => {
                assert_len(2)?;

                // NOTE: Legacy encoding of ConstitutionalCommitteeMemberStatus
                //
                // In the past, we would only encode the hot credential which share most of their
                // encoding with this type up-to-this point. Here we allow decoding StakeCredential
                // directly.
                if matches!(d.datatype()?, cbor::Type::Bytes | cbor::Type::BytesIndef) {
                    return Ok(Self::DelegatedToHotCredential(StakeCredential::KeyHash(d.decode_with(ctx)?)));
                }

                Ok(Self::DelegatedToHotCredential(d.decode_with(ctx)?))
            }
            1 => {
                assert_len(2)?;

                // NOTE: Legacy encoding of ConstitutionalCommitteeMemberStatus
                //
                // Same as above, but the variant '1', corresponding to a script hash.
                if matches!(d.datatype()?, cbor::Type::Bytes | cbor::Type::BytesIndef) {
                    return Ok(Self::DelegatedToHotCredential(StakeCredential::ScriptHash(d.decode_with(ctx)?)));
                }

                d.decode_with::<_, SerialisedAsArray<Option<crate::Anchor>>>(ctx)?;

                Ok(Self::Resigned)
            }
            t => Err(cbor::decode::Error::message(format!(
                "unexpected ConstitutionalCommitteeMemberStatus variant: {t}; expected 0 or 1."
            ))),
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{ConstitutionalCommitteeMemberStatus, any_stake_credential, prop_cbor_roundtrip};

    prop_cbor_roundtrip!(ConstitutionalCommitteeMemberStatus, any_constitutional_committee_member_status());

    proptest! {
        // ensure compatibility with legacy format.
        #[test]
        fn decode_from_stake_credential(stake_credential in any_stake_credential()) {
            use crate::{from_cbor,  to_cbor};

            let bytes = to_cbor(&stake_credential);
            let status = from_cbor::<ConstitutionalCommitteeMemberStatus>(&bytes).unwrap();

            assert_eq!(status, ConstitutionalCommitteeMemberStatus::DelegatedToHotCredential(stake_credential));
        }

        #[test]
        fn decode_with_anchor(anchor in crate::any_anchor()) {
            use crate::{from_cbor, to_cbor, utils::cbor::SerialisedAsArray};

            let bytes = to_cbor(&(1, SerialisedAsArray(Some(anchor))));
            let status = from_cbor::<ConstitutionalCommitteeMemberStatus>(&bytes).unwrap();

            assert_eq!(status, ConstitutionalCommitteeMemberStatus::Resigned)
        }
    }

    pub fn any_constitutional_committee_member_status() -> impl Strategy<Value = ConstitutionalCommitteeMemberStatus> {
        prop_oneof![
            any_stake_credential().prop_map(ConstitutionalCommitteeMemberStatus::DelegatedToHotCredential),
            Just(ConstitutionalCommitteeMemberStatus::Resigned),
        ]
    }
}
