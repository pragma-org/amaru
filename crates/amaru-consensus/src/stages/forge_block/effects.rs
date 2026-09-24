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

//! External effects the `forge_block` stage calls. Production resources for
//! credentials and mempool-for-parent are not provided here; tests override
//! these effects.

use std::sync::Arc;

use amaru_kernel::{
    Epoch, Hash, Header, HeaderBody, HeaderHash, KesPeriod, Nonce, PoolId, ProtocolVersion, RawBlock, Slot, VrfCert,
};
use amaru_ouroboros_traits::{ForgingCredentials, ForgingCredentialsError};
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Resources, SendData};

use super::schedule::EpochSchedule;

pub type ResourceForgingCredentials = Arc<dyn ForgingCredentials>;

/// Transactions selected for a parent and slot, already a well-formed block body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForgedBody {
    pub block: RawBlock,
    pub hash: Hash<32>,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum ForgeEffectError {
    #[error("forging credentials resource is missing")]
    CredentialsMissing,
    #[error(transparent)]
    Credentials(#[from] ForgingCredentialsError),
}

fn empty_body() -> ForgedBody {
    ForgedBody { block: RawBlock::from(&[][..]), hash: Hash::<32>::from([0u8; 32]), size: 0 }
}

/// Detached leader-schedule computation for one epoch.
///
/// 432,000 VRF evaluations on mainnet; the stage must not occupy the airlock
/// until it finishes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LeaderScheduleEffect {
    pub epoch: Epoch,
    pub nonce: Nonce,
    pub pool: PoolId,
    pub from: Slot,
    /// Exclusive upper bound: the first slot of the next epoch.
    pub until: Slot,
}

impl LeaderScheduleEffect {
    pub fn new(epoch: Epoch, nonce: Nonce, pool: PoolId, from: Slot, until: Slot) -> Self {
        Self { epoch, nonce, pool, from, until }
    }
}

impl ExternalEffectAPI for LeaderScheduleEffect {
    type Response = EpochSchedule;
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let schedule = EpochSchedule::empty(self.epoch, self.nonce);
        self.wrap_sync(schedule)
    }
}

/// Sign a header for `slot` over `body`. KES evolution happens inside this call.
///
/// `vrf_cert` comes from the leader schedule, so this call reaches for the KES
/// secret only; the VRF secret stays confined to [`LeaderScheduleEffect`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForgeHeaderEffect {
    pub slot: Slot,
    pub kes_period: KesPeriod,
    pub parent: HeaderHash,
    pub block_number: u64,
    pub body_hash: Hash<32>,
    pub body_size: u64,
    pub vrf_cert: VrfCert,
    pub protocol_version: ProtocolVersion,
}

impl ForgeHeaderEffect {
    pub fn new(
        slot: Slot,
        kes_period: KesPeriod,
        parent: HeaderHash,
        block_number: u64,
        body: &ForgedBody,
        vrf_cert: VrfCert,
        protocol_version: ProtocolVersion,
    ) -> Self {
        Self {
            slot,
            kes_period,
            parent,
            block_number,
            body_hash: body.hash,
            body_size: body.size,
            vrf_cert,
            protocol_version,
        }
    }

    fn forge(&self, credentials: &dyn ForgingCredentials) -> Result<Header, ForgeEffectError> {
        let body = HeaderBody {
            block_number: self.block_number,
            slot: u64::from(self.slot),
            prev_hash: Some(self.parent),
            issuer_verification_key: credentials.issuer_verification_key(),
            vrf_verification_key: credentials.vrf_verification_key(),
            vrf_result: self.vrf_cert.clone(),
            block_body_size: self.body_size,
            block_body_hash: self.body_hash,
            operational_cert: credentials.operational_cert(),
            protocol_version: self.protocol_version,
        };
        let signature = credentials.sign(self.kes_period, &body)?;
        Ok(Header::new(body, signature))
    }
}

impl ExternalEffectAPI for ForgeHeaderEffect {
    type Response = Result<Header, ForgeEffectError>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let header = match resources.get::<ResourceForgingCredentials>() {
            Ok(credentials) => self.forge(&**credentials),
            Err(_) => Err(ForgeEffectError::CredentialsMissing),
        };
        self.wrap_sync(header)
    }
}

/// Ask the mempool for a body valid on `parent` as of `slot`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TakeForForgeEffect {
    pub parent: HeaderHash,
    pub slot: Slot,
}

impl TakeForForgeEffect {
    pub fn new(parent: HeaderHash, slot: Slot) -> Self {
        Self { parent, slot }
    }
}

impl ExternalEffectAPI for TakeForForgeEffect {
    type Response = ForgedBody;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync(empty_body())
    }
}
