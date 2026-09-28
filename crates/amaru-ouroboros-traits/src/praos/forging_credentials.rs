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
    Hash, Hasher, Header, HeaderHash, KesPeriod, KesPeriodError, PoolId, ProtocolVersion, Slot, VerificationKey,
    VrfCert, size::POOL_COLD_KEY,
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
/// The credentials add the issuer and VRF verification keys and the operational
/// certificate, then sign the whole body.
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

/// A block producer's identity: the keys every forged header carries, and the
/// KES key that signs it.
///
/// The KES secret never leaves the implementation. Signing takes `&self`
/// even though the key evolves in place and one-way: the implementation
/// holds the key behind a lock so that evolving and signing happen as one
/// step, and no caller can interleave between them. The operational
/// certificate is written into the header under that same lock, so a header
/// always carries the certificate of the key that signed it.
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

    /// Complete `draft` with this producer's keys and certificate and sign it
    /// with the KES key evolved to `period`.
    ///
    /// `period` is absolute (from `ConsensusParameters::slot_to_kes_period`);
    /// the implementation evolves from the certificate's start period and
    /// reports [`ForgingCredentialsError::Period`] when the certificate does
    /// not cover `period`.
    fn sign(&self, period: KesPeriod, draft: HeaderDraft) -> Result<Header, ForgingCredentialsError>;
}
