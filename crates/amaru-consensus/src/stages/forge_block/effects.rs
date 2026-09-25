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

//! External effects the `forge_block` stage calls.
//!
//! Leader schedules are computed here from the credentials resource and
//! [`EpochSchedule::compute`](super::schedule::EpochSchedule::compute). The body
//! effect returns an empty block until the mempool can supply one.

use std::sync::Arc;

use amaru_kernel::{
    Block, BodyParts, Epoch, EraHistory, EraHistoryError, Hash, Header, HeaderBody, HeaderHash, KesPeriod, Nonce,
    PoolId, ProtocolVersion, RawBlock, Slot, VrfCert, cardano::network_block::NetworkBlock, maths::FixedDecimal,
};
use amaru_ouroboros::vrf;
use amaru_ouroboros_traits::{ForgingCredentials, ForgingCredentialsError};
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Resources, SendData};

use super::schedule::EpochSchedule;
use crate::effects::{ResourceConsensusParameters, ResourcePoolSummaries};

/// `None` in the product binary. Tests (and, later, a configured pool) install `Some`.
pub type ResourceForgingCredentials = Option<Arc<dyn ForgingCredentials>>;

/// Transactions selected for a parent and slot, already a well-formed block body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForgedBody {
    pub hash: Hash<32>,
    pub size: u64,
    /// Body items encoded once. [`Self::seal`] prefixes the signed header; it does not encode the body again.
    parts: BodyParts,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum ForgeEffectError {
    #[error("forging credentials resource is missing")]
    CredentialsMissing,
    #[error(transparent)]
    Credentials(#[from] ForgingCredentialsError),
}

#[derive(Debug, thiserror::Error)]
pub enum EmptyBlockError {
    #[error("forged header does not commit to the empty block body")]
    CommitmentMismatch,
    #[error("empty block encoding failed: {0}")]
    Encode(String),
    #[error("era for the forged slot is unknown: {0}")]
    Era(#[from] EraHistoryError),
}

impl ForgedBody {
    /// No transactions. `hash` and `size` are what the header must commit to.
    ///
    /// `block` is empty until [`Self::seal`] wraps a signed header around that body.
    pub fn empty() -> Self {
        let parts = BodyParts::from_transactions(std::iter::empty())
            .unwrap_or_else(|error| unreachable!("an empty transaction list fits in a block: {error}"));
        let (hash, size) = parts.commitment();
        Self { hash, size, parts }
    }

    /// Network-block bytes for `header` and this body.
    ///
    /// `header` must already carry [`Self::hash`] and [`Self::size`]. The body bytes are the ones
    /// [`BodyParts`] encoded when this value was built.
    pub fn seal(&self, header: &Header, era_history: &EraHistory) -> Result<RawBlock, EmptyBlockError> {
        let committed = &header.body().block_body_hash;
        if *committed != self.hash || header.body().block_body_size != self.size {
            return Err(EmptyBlockError::CommitmentMismatch);
        }
        let bytes = self.parts.encode_block(header);
        let actual = Block::hash_encoded_body(&bytes).map_err(|error| EmptyBlockError::Encode(error.to_string()))?;
        if actual != self.hash {
            return Err(EmptyBlockError::CommitmentMismatch);
        }
        Ok(NetworkBlock::from_encoded_block(era_history, Slot::from(header.body().slot), bytes)?.raw_block())
    }
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

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let schedule = schedule_for(&self, &resources);
        self.wrap_sync(schedule)
    }
}

fn schedule_for(effect: &LeaderScheduleEffect, resources: &Resources) -> EpochSchedule {
    let Some(schedule) = (|| -> Option<EpochSchedule> {
        let credentials = resources.get::<ResourceForgingCredentials>().ok()?.clone()?;
        let parameters = resources.get::<ResourceConsensusParameters>().ok()?.clone();
        let pools = resources.get::<ResourcePoolSummaries>().ok()?.clone();
        let summary = pools.get_pool(effect.from, &effect.pool, parameters.era_history()).ok().flatten()?;
        if summary.active_stake == 0 {
            return None;
        }
        let relative = &FixedDecimal::from(summary.stake) / &FixedDecimal::from(summary.active_stake);
        let coefficient = parameters.active_slot_coeff();
        let vrf = vrf::SecretKey::from(&credentials.vrf_secret_bytes());
        Some(EpochSchedule::compute(
            effect.epoch,
            effect.nonce,
            effect.from..effect.until,
            &relative,
            &coefficient,
            &vrf,
        ))
    })() else {
        return EpochSchedule::empty(effect.epoch, effect.nonce);
    };
    schedule
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
        let header = match resources.get::<ResourceForgingCredentials>().as_deref().cloned() {
            Ok(Some(credentials)) => self.forge(credentials.as_ref()),
            Ok(None) | Err(_) => Err(ForgeEffectError::CredentialsMissing),
        };
        self.wrap_sync(header)
    }
}

/// Ask the mempool for a body valid on `parent` as of `slot`.
///
/// The mempool is not consulted yet. The default body is empty and [`ForgedBody::seal`]
/// turns it into a block the ledger can apply.
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
        self.wrap_sync(ForgedBody::empty())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use amaru_kernel::{ConsensusParameters, Epoch, Hash, Nonce, PREPROD_ERA_HISTORY, Slot};
    use amaru_ouroboros::{praos::header::AssertVrfProofError, vrf};
    use amaru_ouroboros_traits::{PoolSummaries, PoolSummary};
    use amaru_pure_stage::Resources;

    use super::*;
    use crate::stages::forge_block::{TestCredentials, test_vrf_key};

    fn resources_with(
        credentials: Option<Arc<dyn ForgingCredentials>>,
        parameters: ConsensusParameters,
        pools: PoolSummaries,
    ) -> Resources {
        let resources = Resources::default();
        resources.put::<ResourceForgingCredentials>(credentials);
        resources.put::<ResourceConsensusParameters>(Arc::new(parameters));
        resources.put::<ResourcePoolSummaries>(Arc::new(pools));
        resources
    }

    #[test]
    fn missing_credentials_schedule_no_slots() {
        let effect = LeaderScheduleEffect::new(
            Epoch::from(2),
            Nonce::from([1u8; 32]),
            Hash::new([0u8; 28]),
            Slot::from(0),
            Slot::from(10),
        );
        let resources = Resources::default();
        resources.put::<ResourceForgingCredentials>(None);
        let schedule = schedule_for(&effect, &resources);
        assert_eq!(schedule.epoch(), effect.epoch);
        assert!(schedule_is_empty(&schedule));
    }

    #[test]
    fn full_stake_and_coefficient_lead_every_slot_in_range() {
        let credentials = TestCredentials::for_test_keys(amaru_kernel::KesPeriod::from(0), 62);
        let pool = credentials.pool_id();
        // First Conway slot on preprod is epoch 163, so leadership reads the epoch 161 snapshot.
        let from = Slot::from(68_774_400);
        let until = Slot::from(u64::from(from) + 4);
        let nonce = Nonce::from([1u8; 32]);
        let mut by_pool = BTreeMap::new();
        by_pool.insert(pool, PoolSummary { vrf: Hash::new([0u8; 32]), stake: 1, active_stake: 1 });
        let pools = PoolSummaries { by_epoch: BTreeMap::from([(Epoch::from(161), by_pool)]) };
        let parameters = ConsensusParameters::create(1, 129_600, 62, 1.0, &PREPROD_ERA_HISTORY);
        let resources = resources_with(Some(Arc::new(credentials)), parameters, pools);
        let effect = LeaderScheduleEffect::new(Epoch::from(163), nonce, pool, from, until);
        let schedule = schedule_for(&effect, &resources);

        let led: Vec<u64> = schedule.slots().keys().map(|slot| u64::from(*slot)).collect();
        let expected: Vec<u64> = (u64::from(from)..u64::from(until)).collect();
        assert_eq!(led, expected);

        let cert = schedule.slots().get(&from).expect("first slot");
        AssertVrfProofError::new(from, &nonce, &vrf::PublicKey::from(&test_vrf_key()), cert).unwrap();
    }

    fn schedule_is_empty(schedule: &EpochSchedule) -> bool {
        schedule.slots().is_empty()
    }
}
