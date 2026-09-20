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

use amaru_kernel::{Epoch, Hash, Header, HeaderHash, Nonce, PoolId, RawBlock, Slot};
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Resources, SendData};

/// Transactions selected for a parent and slot, already a well-formed block body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForgedBody {
    pub block: RawBlock,
    pub hash: Hash<32>,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum ForgeEffectError {
    #[error("forging credentials resource is not implemented")]
    CredentialsUnimplemented,
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
    pub until: Slot,
}

impl LeaderScheduleEffect {
    pub fn new(epoch: Epoch, nonce: Nonce, pool: PoolId, from: Slot, until: Slot) -> Self {
        Self { epoch, nonce, pool, from, until }
    }
}

impl ExternalEffectAPI for LeaderScheduleEffect {
    type Response = Vec<Slot>;
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync(Vec::new())
    }
}

/// Sign a header for `slot` over `body`. KES evolution happens inside this call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForgeHeaderEffect {
    pub slot: Slot,
    pub parent: HeaderHash,
    pub block_number: u64,
    pub body_hash: Hash<32>,
    pub body_size: u64,
}

impl ForgeHeaderEffect {
    pub fn new(slot: Slot, parent: HeaderHash, block_number: u64, body: &ForgedBody) -> Self {
        Self { slot, parent, block_number, body_hash: body.hash, body_size: body.size }
    }
}

impl ExternalEffectAPI for ForgeHeaderEffect {
    type Response = Result<Header, ForgeEffectError>;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync(Err(ForgeEffectError::CredentialsUnimplemented))
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
