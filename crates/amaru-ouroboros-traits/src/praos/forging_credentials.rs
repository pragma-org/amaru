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

use amaru_kernel::{HeaderBody, KesPeriod, KesPeriodError, KesSignature, OperationalCert, VerificationKey};
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

/// A block producer's identity: the public material every forged header
/// carries, and the KES key that signs it.
///
/// The KES secret never leaves the implementation. Signing takes `&self`
/// even though the key evolves in place and one-way: the implementation
/// holds the key behind a lock so that evolving and signing happen as one
/// step, and no caller can interleave between them.
pub trait ForgingCredentials: Send + Sync {
    /// Cold verification key; `HeaderBody::issuer_verification_key`.
    fn issuer_verification_key(&self) -> VerificationKey;

    fn vrf_verification_key(&self) -> VerificationKey;

    /// Operational certificate delegating from the cold key to the current KES key.
    fn operational_cert(&self) -> OperationalCert;

    /// Sign `msg` with the KES key evolved to `period`.
    ///
    /// `period` is absolute (from `ConsensusParameters::slot_to_kes_period`);
    /// the implementation evolves from the certificate's start period.
    fn sign(&self, period: KesPeriod, header_body: &HeaderBody) -> Result<KesSignature, ForgingCredentialsError>;
}
