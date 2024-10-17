use std::collections::HashMap;
use crate::consensus::{Point, ValidateHeaderEvent};
use crate::sync::PullEvent;
use gasket::framework::*;
use miette::miette;
use ouroboros::ledger::{MockLedgerState, PoolSigma};
use ouroboros::validator::Validator;
use ouroboros_praos::consensus::BlockValidator;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_math::math::{FixedDecimal, FixedPrecision};
use pallas_network::facades::PeerClient;
use pallas_traverse::MultiEraHeader;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tracing::{info, trace};

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;

/// Notes:
/// We're currently testing block validation in epoch 172 on preprod. This is a happy path test.
/// Get the stake snapshot from cncli. I took this example during 173 so I used the go snapshot.
/// `$ cncli pool-stake --socket-path /home/westbam/haskell/pprod/db/socket --name go --output-file preprod_172_stake_snapshot.csv --network-magic 1`
/// the last block of epoch 171 is 72662384.2f2bcab30dc53444cef311d6985eb126fd577a548b8e71cafecbb6e855bc8755

/// Mock the ledger state for the block validator. These values are the happy path
/// and don't really validate anything currently. We assume the pool has plenty of stake, the
/// VRF key hash is always available.
fn mock_ledger_state(vrf_vkey_hash: Hash<32>, pool_id_to_sigma: &HashMap<Hash<28>, PoolSigma>) -> MockLedgerState {
    let mut ledger_state = MockLedgerState::new();
    ledger_state
        .expect_pool_id_to_sigma()
        .returning(|pool_id| {
            // TODO: use pool_id_to_sigma here
            Ok(PoolSigma {
                numerator: 1,
                denominator: 1,
            })
        });

    // assume that the vrf_vkey_hash is always available and the ledger value matches what was in the block.
    ledger_state
        .expect_vrf_vkey_hash()
        .returning(move |_| Ok(vrf_vkey_hash));

    // standard calculation for preprod and mainnet. Preview is different.
    ledger_state.expect_slot_to_kes_period().returning(|slot| {
        // hardcode some values from shelley-genesis.json for the mock implementation
        let slots_per_kes_period: u64 = 129600; // from shelley-genesis.json (1.5 days in seconds)
        slot / slots_per_kes_period
    });

    // standard kes rotation for preview, preprod, and mainnet configs
    ledger_state.expect_max_kes_evolutions().returning(|| 62);
    ledger_state
        .expect_latest_opcert_sequence_number()
        .returning(|_| None);

    ledger_state
}


#[derive(Stage)]
#[stage(name = "validate", unit = "PullEvent", worker = "Worker")]
pub struct Stage {
    peer_session: Arc<Mutex<PeerClient>>,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,

    #[metric]
    block_count: gasket::metrics::Counter,

    #[metric]
    rollback_count: gasket::metrics::Counter,

    #[metric]
    validation_tip: gasket::metrics::Gauge,
}

impl Stage {
    pub fn new(peer_session: Arc<Mutex<PeerClient>>) -> Self {
        Self {
            peer_session,
            upstream: Default::default(),
            downstream: Default::default(),
            block_count: Default::default(),
            rollback_count: Default::default(),
            validation_tip: Default::default(),
        }
    }

    fn track_validation_tip(&self, tip: &Point) {
        self.validation_tip.set(tip.slot_or_default() as i64);
    }
}

pub struct Worker {
    pool_id_to_sigma: std::collections::HashMap<Hash<28>, PoolSigma>,
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(_stage: &Stage) -> Result<Self, WorkerError> {
        let stake_snapshot_csv = include_str!("../../preprod_172_stake_snapshot.csv");
        // create a hashmap of pool_id to Sigma based on the stake snapshot. this file contains the pool_id and then the sigma numerator and denominator
        let mut pool_id_to_sigma = std::collections::HashMap::new();
        for line in stake_snapshot_csv.lines() {
            let mut parts = line.split(',');
            let pool_id: Hash<28> = parts.next().unwrap().parse().unwrap();
            let numerator: u64 = parts.next().unwrap().parse().unwrap();
            let denominator: u64 = parts.next().unwrap().parse().unwrap();
            pool_id_to_sigma.insert(pool_id, PoolSigma { numerator, denominator });
        }

        let worker = Self {
            pool_id_to_sigma,
        };

        Ok(worker)
    }

    async fn schedule(
        &mut self,
        stage: &mut Stage,
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(&mut self, unit: &PullEvent, stage: &mut Stage) -> Result<(), WorkerError> {
        let mut peer_session = stage.peer_session.lock().unwrap();
        let client = (*peer_session).blockfetch();

        match unit {
            PullEvent::RollForward(point, raw_header) => {
                info!(?point, "validating roll forward");
                let header = MultiEraHeader::decode(6, None, raw_header)
                    .map_err(|e| miette!(e))
                    .or_panic()?;

                match header {
                    MultiEraHeader::BabbageCompatible(_) => {
                        let minted_header = header.as_babbage().unwrap();
                        let vrf_vkey_hash = Hasher::<256>::hash(minted_header.header_body.vrf_vkey.as_ref());
                        let ledger_state = mock_ledger_state(vrf_vkey_hash, &self.pool_id_to_sigma);

                        // FIXME: We need a better way to fetch this value. Maybe get it from https://koios.rest ?
                        let epoch_172_nonce = Hash::<32>::from_str("99154c200bba185c215a3648e13e02b07143d4bc86c372bcfb57084ead1fd158").unwrap();

                        let active_slots_coeff: FixedDecimal =
                            FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
                        let c = (FixedDecimal::from(1u64) - active_slots_coeff).ln();
                        let block_validator = BlockValidator::new(minted_header, &ledger_state, &epoch_172_nonce, &c);
                        block_validator.validate().or_panic()?;
                    }
                    MultiEraHeader::ShelleyCompatible(_) => {
                        trace!("shelley compatible header, skipping validation");
                    }
                    MultiEraHeader::EpochBoundary(_) => {
                        trace!("epoch boundary header, skipping validation");
                    }
                    MultiEraHeader::Byron(_) => {
                        trace!("byron header, skipping validation");
                    }
                }

                let block = client
                    .fetch_single(point.clone())
                    .await
                    .or_restart()?;

                info!(?block, "fetched block");

                stage
                    .downstream
                    .send(ValidateHeaderEvent::Validated(point.clone(), block).into())
                    .await
                    .or_panic()?;

                stage.block_count.inc(1);
                stage.track_validation_tip(point);
            }
            PullEvent::Rollback(rollback) => {
                info!(?rollback, "validating roll back");

                stage
                    .downstream
                    .send(ValidateHeaderEvent::Rollback(rollback.clone()).into())
                    .await
                    .or_panic()?;

                stage.rollback_count.inc(1);
                stage.track_validation_tip(rollback);
            }
        }

        Ok(())
    }
}
