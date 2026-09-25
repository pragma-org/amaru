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

use crate::{Hash, OperationalCert, ProtocolVersion, VerificationKey, VrfCert, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct HeaderBody {
    #[n(0)]
    pub block_number: u64,

    #[n(1)]
    pub slot: u64,

    #[n(2)]
    pub prev_hash: Option<Hash<32>>,

    #[n(3)]
    pub issuer_verification_key: VerificationKey,

    #[n(4)]
    pub vrf_verification_key: VerificationKey,

    #[n(5)]
    pub vrf_result: VrfCert,

    #[n(6)]
    pub block_body_size: u64,

    #[n(7)]
    pub block_body_hash: Hash<32>,

    #[n(8)]
    pub operational_cert: OperationalCert,

    #[n(9)]
    pub protocol_version: ProtocolVersion,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for HeaderBody {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(10)?;
            Ok(Self {
                block_number: d.decode_with(ctx)?,
                slot: d.decode_with(ctx)?,
                prev_hash: d.decode_with(ctx)?,
                issuer_verification_key: d.decode_with(ctx)?,
                vrf_verification_key: d.decode_with(ctx)?,
                vrf_result: d.decode_with(ctx)?,
                block_body_size: d.decode_with(ctx)?,
                block_body_hash: d.decode_with(ctx)?,
                operational_cert: d.decode_with(ctx)?,
                protocol_version: d.decode_with(ctx)?,
            })
        })
    }
}
