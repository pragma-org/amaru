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

use crate::{Ed25519Signature, VerificationKey, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct OperationalCert {
    #[n(0)]
    pub operational_cert_hot_verification_key: VerificationKey,

    #[n(1)]
    pub operational_cert_sequence_number: u64,

    #[n(2)]
    pub operational_cert_kes_period: u64,

    #[n(3)]
    pub operational_cert_sigma: Ed25519Signature,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for OperationalCert {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(4)?;
            Ok(Self {
                operational_cert_hot_verification_key: d.decode_with(ctx)?,
                operational_cert_sequence_number: d.decode_with(ctx)?,
                operational_cert_kes_period: d.decode_with(ctx)?,
                operational_cert_sigma: d.decode_with(ctx)?,
            })
        })
    }
}
