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
//! [`EpochSchedule::compute`](super::schedule::EpochSchedule::compute). The body effect resolves
//! the current ledger parameters and selects transactions from the registered mempool without
//! cloning transactions that do not fit in the block.

use std::sync::Arc;

use amaru_kernel::{
    BlockBodyBuilder, BlockBodyParts, Epoch, HeaderHash, KesPeriod, KesSignature, Nonce, Point, PoolId, Slot,
    Transaction, maths::FixedDecimal,
};
use amaru_observability::{info, info_span};
use amaru_ouroboros::{ResourceMempool, vrf};
use amaru_ouroboros_traits::{BlockBuilderConstraintsResponse, ForgingCredentials, ForgingCredentialsError};
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Resources, SendData};

use super::schedule::EpochSchedule;
use crate::effects::{ResourceBlockBuilderConstraints, ResourceConsensusParameters, ResourcePoolSummaries};

/// `None` in the product binary. Tests (and, later, a configured pool) install `Some`.
pub type ResourceForgingCredentials = Option<Arc<dyn ForgingCredentials>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum SignHeaderError {
    #[error("forging credentials resource is missing")]
    CredentialsMissing,
    #[error(transparent)]
    Credentials(#[from] ForgingCredentialsError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum TakeForForgeError {
    #[error("unable to construct block body: {0}")]
    Body(#[from] amaru_kernel::BlockBodyBuilderError),
    #[error("ledger worker terminated while reading block-builder constraints")]
    LedgerThreadTerminated,
    #[error("ledger or mempool tip does not match forge parent {parent} at slot {slot} (ledger tip: {ledger_tip})")]
    TipsChanged { ledger_tip: Point, parent: HeaderHash, slot: Slot },
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
        let vrf = vrf::SecretKey::from(&credentials.vrf_secret_bytes());
        Some(EpochSchedule::compute(
            effect.epoch,
            effect.nonce,
            effect.from..effect.until,
            &relative,
            parameters.active_slot_coeff_ln_complement(),
            &vrf,
        ))
    })() else {
        return EpochSchedule::empty(effect.epoch, effect.nonce);
    };
    schedule
}

/// Sign `message` with the KES key evolved to `kes_period`.
///
/// `message` is the header-body CBOR from [`kes_message`](amaru_ouroboros_traits::kes_message).
/// The stage builds the header around the returned signature. Signing is the
/// effect because it talks to the credentials resource; assembling the header is not.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SignHeaderEffect {
    pub kes_period: KesPeriod,
    pub message: Vec<u8>,
}

impl SignHeaderEffect {
    pub fn new(kes_period: KesPeriod, message: Vec<u8>) -> Self {
        Self { kes_period, message }
    }
}

impl ExternalEffectAPI for SignHeaderEffect {
    type Response = Result<KesSignature, SignHeaderError>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let signature = match resources.get::<ResourceForgingCredentials>().as_deref().cloned() {
            Ok(Some(credentials)) => credentials.sign(self.kes_period, &self.message).map_err(Into::into),
            Ok(None) | Err(_) => Err(SignHeaderError::CredentialsMissing),
        };
        self.wrap_sync(signature)
    }
}

/// Select a body only when the ledger and mempool views still match `parent` at `slot`.
///
/// The consistency check prevents transactions validated against a different ledger tip from
/// being removed and included in a block on a stale fork.
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
    type Response = Result<BlockBodyParts, TakeForForgeError>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let mempool = resources
            .get::<ResourceMempool<Transaction>>()
            .expect("ResourceMempool must be registered before the forge stage is started")
            .clone();

        let constraints = resources
            .get::<ResourceBlockBuilderConstraints>()
            .expect("ResourceBlockBuilderConstraints must be registered before the forge stage is started")
            .clone();

        self.wrap(|this| async move {
            let BlockBuilderConstraintsResponse { tip: ledger_tip, constraints } =
                constraints.block_builder_constraints().await.map_err(|_| TakeForForgeError::LedgerThreadTerminated)?;

            if ledger_tip.hash() != this.parent || ledger_tip.slot() > this.slot {
                return Err(TakeForForgeError::TipsChanged { ledger_tip, parent: this.parent, slot: this.slot });
            }

            let Some(mempool_view) = mempool.read_at(ledger_tip) else {
                return Err(TakeForForgeError::TipsChanged { ledger_tip, parent: this.parent, slot: this.slot });
            };

            info_span!(consensus::forge::BUILD_BLOCK, slot = this.slot).in_scope(|| {
                let mut builder = BlockBodyBuilder::default();
                let mut selected = Vec::new();

                // FIXME: Assumptions of block forging iterating through mempool
                //
                // This simple construction works only because we assume transactions in the mempool to
                // be independent; which is the case with the current strategy, but the underlying code
                // here should not be able to make this assumption.
                //
                // In principle, we should be able to inform the iterator of us skipping a transaction;
                // or reverse the problem and have the iterator return batches of dependent
                // transactions. Skipping one transaction from the batch should skip all the subsequent
                // one and move to the next batch.
                for transaction in mempool_view.iter() {
                    // The ledger tip can be older than the slot being forged. Revalidation at the
                    // previous tip is therefore not sufficient to prove that the transaction is
                    // still valid for this block.
                    if transaction.body.validity_interval().includes(this.slot)
                        && builder.try_push(transaction, &constraints)?
                    {
                        selected.push(transaction.tx_id());
                    }
                }

                info!(
                    consensus::forge::BLOCK_BODY,
                    slot = this.slot,
                    tx_count = selected.len() as u64,
                    size_bytes = builder.size(),
                    ex_units = builder.ex_units(),
                    ref_scripts_size = builder.ref_scripts_size(),
                );

                let body = builder.finalize()?;

                drop(mempool_view);
                mempool.remove(&selected);

                Ok(body)
            })
        })
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
            PoolId::new([0u8; 28]),
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
        let pools = PoolSummaries { by_epoch: BTreeMap::from([(Epoch::from(161), by_pool)]), ..Default::default() };
        let parameters = ConsensusParameters::create(1, 129_600, 62, 1, &PREPROD_ERA_HISTORY);
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
