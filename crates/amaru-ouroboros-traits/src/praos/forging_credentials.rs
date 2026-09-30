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
    Hash, Hasher, HeaderBody, HeaderHash, KesPeriod, KesPeriodError, KesSignature, OperationalCert, PoolId,
    ProtocolVersion, Slot, VerificationKey, VrfCert, size::POOL_COLD_KEY, to_cbor,
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error, serde::Serialize, serde::Deserialize)]
pub enum ForgingCredentialsError {
    #[error("operational certificate does not cover KES period: {0}")]
    Period(#[from] KesPeriodError),
    #[error("KES key already evolved past the requested period: {0}")]
    EvolvedPast(String),
    #[error("KES signing failed: {0}")]
    Kes(String),
}

/// The header fields a block producer decides for a led slot.
///
/// Issuer keys and the operational certificate come from [`IssuerFields`].
/// The KES signature is computed separately, over [`kes_message`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HeaderDraft {
    pub block_number: u64,
    pub slot: Slot,
    pub prev_hash: Option<HeaderHash>,
    pub vrf_result: VrfCert,
    pub block_body_size: u64,
    pub block_body_hash: Hash<32>,
    pub protocol_version: ProtocolVersion,
}

impl HeaderDraft {
    /// Header body for `self` plus the producer's public issuer fields.
    pub fn body(self, issuer: &IssuerFields) -> HeaderBody {
        HeaderBody {
            block_number: self.block_number,
            slot: u64::from(self.slot),
            prev_hash: self.prev_hash,
            issuer_verification_key: issuer.issuer_verification_key,
            vrf_verification_key: issuer.vrf_verification_key,
            vrf_result: self.vrf_result,
            block_body_size: self.block_body_size,
            block_body_hash: self.block_body_hash,
            operational_cert: issuer.operational_cert.clone(),
            protocol_version: self.protocol_version,
        }
    }
}

/// Public issuer material a forged header carries.
///
/// These fields are not secret. The KES signing key stays inside the
/// credentials implementation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IssuerFields {
    pub issuer_verification_key: VerificationKey,
    pub vrf_verification_key: VerificationKey,
    pub operational_cert: OperationalCert,
}

/// Bytes the KES key signs: the CBOR encoding of the header body.
///
/// Header validation checks the signature against this same encoding.
pub fn kes_message(body: &HeaderBody) -> Vec<u8> {
    to_cbor(body)
}

/// A block producer's identity: the keys every forged header carries, and the
/// KES key that signs it.
///
/// The KES secret never leaves the implementation. Signing takes `&self`
/// even though the key evolves in place and one-way: the implementation
/// holds the key behind a lock so that evolving and signing happen as one
/// step, and no caller can interleave between them.
///
/// The caller builds the header. [`Self::sign`] receives only the bytes KES
/// signs and returns only the signature. [`Self::issuer_fields`] supplies the
/// public fields that go into that header. Those fields are fixed when the
/// caller reads them; a later certificate rotation has to be read again
/// before the next signature, or the header and the signature will disagree.
pub trait ForgingCredentials: Send + Sync {
    /// Cold verification key; `HeaderBody::issuer_verification_key`.
    fn issuer_verification_key(&self) -> VerificationKey;

    /// Pool id: blake2b-224 of the cold verification key.
    fn pool_id(&self) -> PoolId {
        Hasher::<{ 8 * POOL_COLD_KEY }>::hash(&self.issuer_verification_key()[..])
    }

    /// 32-byte VRF signing seed. The leader-schedule effect turns this into proofs;
    /// the seed does not enter stage state.
    fn vrf_secret_bytes(&self) -> [u8; 32];

    /// Public fields a forged header carries for this producer.
    fn issuer_fields(&self) -> IssuerFields;

    /// Evolve the KES key to `period` and sign `message`.
    ///
    /// `message` is the direct signing input, [`kes_message`] of the header body.
    /// `period` is absolute (from `ConsensusParameters::slot_to_kes_period`);
    /// the implementation evolves from the certificate's start period and
    /// reports [`ForgingCredentialsError::Period`] when the certificate does
    /// not cover `period`.
    fn sign(&self, period: KesPeriod, message: &[u8]) -> Result<KesSignature, ForgingCredentialsError>;
}
