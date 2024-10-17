use crate::consensus::{Point, ValidateHeaderEvent};
use crate::sync::PullEvent;
use gasket::framework::*;
use miette::miette;
use ouroboros::ledger::{issuer_vkey_to_pool_id, MockLedgerState, PoolSigma};
use ouroboros::validator::Validator;
use ouroboros_praos::consensus::BlockValidator;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_math::math::{FixedDecimal, FixedPrecision};
use pallas_network::facades::PeerClient;
use pallas_traverse::MultiEraHeader;
use std::collections::HashMap;
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
fn mock_ledger_state(vrf_vkey_hash: Hash<32>, pool_sigma: &PoolSigma) -> MockLedgerState {
    let mut ledger_state = MockLedgerState::new();

    let ps = pool_sigma.clone();
    ledger_state
        .expect_pool_id_to_sigma()
        .returning(move |_| Ok(ps.clone()));

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

    // assume this is the first block we've ever seen from this issuer_vkey (never fail).
    ledger_state
        .expect_latest_opcert_sequence_number()
        .returning(|_| None);

    ledger_state
}

/// Mocking this calculation specifically for preprod. We need to know the epoch for the slot so
/// we can look up the epoch nonce.
fn epoch_for_preprod_slot(slot: u64) -> u64 {
    let shelley_epoch_length = 432000; // 5 days in seconds
    let shelley_transition_epoch: u64 = 4;
    let byron_protocol_consts_k: u64 = 2160;
    let byron_epoch_length = 10 * byron_protocol_consts_k;
    let byron_slots = byron_epoch_length * shelley_transition_epoch;
    let shelley_slots = slot - byron_slots;
    let epoch = (shelley_slots / shelley_epoch_length) + shelley_transition_epoch;

    epoch
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
    pool_id_to_sigma: HashMap<Hash<28>, PoolSigma>,
    epoch_to_nonce: HashMap<u64, Hash<32>>,
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(_stage: &Stage) -> Result<Self, WorkerError> {
        let stake_snapshot_csv = include_str!("../../preprod_172_stake_snapshot.csv");
        // create a hashmap of pool_id to Sigma based on the stake snapshot. this file contains the pool_id and then the sigma numerator and denominator
        let mut pool_id_to_sigma = HashMap::new();
        for line in stake_snapshot_csv.lines() {
            let mut parts = line.split(',');
            let pool_id: Hash<28> = parts.next().unwrap().parse().unwrap();
            let numerator: u64 = parts.next().unwrap().parse().unwrap();
            let denominator: u64 = parts.next().unwrap().parse().unwrap();
            pool_id_to_sigma.insert(
                pool_id,
                PoolSigma {
                    numerator,
                    denominator,
                },
            );
        }

        // #!/bin/bash
        //
        // rm -f preprod_nonce.csv
        // touch preprod_nonce.csv
        //
        // # loop from 4 to 174 inclusive
        // for i in {4..174}
        // do
        //     nonce=$(curl -X GET "https://preprod.koios.rest/api/v1/epoch_params?_epoch_no=$i" -H "accept: application/json" 2>/dev/null | jq -r '.[0].nonce')
        //     echo "$i,$nonce" >> preprod_nonce.csv
        //     echo "Epoch $i nonce: $nonce"
        // done
        let epoch_nonce_csv = include_str!("../../preprod_nonce.csv");
        let mut epoch_to_nonce = HashMap::new();
        for line in epoch_nonce_csv.lines() {
            let mut parts = line.split(',');
            let epoch: u64 = parts.next().unwrap().parse().unwrap();
            let nonce: Hash<32> = parts.next().unwrap().parse().unwrap();
            epoch_to_nonce.insert(epoch, nonce);
        }

        let worker = Self {
            pool_id_to_sigma,
            epoch_to_nonce,
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
        match unit {
            PullEvent::RollForward(point, raw_header) => {
                info!(?point, "validating roll forward");
                let header = MultiEraHeader::decode(6, None, raw_header)
                    .map_err(|e| miette!(e))
                    .or_panic()?;

                match header {
                    MultiEraHeader::BabbageCompatible(_) => {
                        let minted_header = header.as_babbage().unwrap();
                        let vrf_vkey_hash =
                            Hasher::<256>::hash(minted_header.header_body.vrf_vkey.as_ref());
                        let pool_id =
                            issuer_vkey_to_pool_id(&minted_header.header_body.issuer_vkey);

                        let epoch = epoch_for_preprod_slot(minted_header.header_body.slot);
                        let epoch_nonce = self
                            .epoch_to_nonce
                            .get(&epoch)
                            .ok_or(miette!("epoch nonce not found"))
                            .or_panic()?;

                        let pool_sigma = if epoch == 172 {
                            self.pool_id_to_sigma
                                .get(&pool_id)
                                .ok_or(miette!("pool not found"))
                                .or_panic()?
                        } else {
                            // if validating other epochs on preprod (never fail)
                            &PoolSigma {
                                numerator: 1,
                                denominator: 1,
                            }
                        };

                        let ledger_state = mock_ledger_state(vrf_vkey_hash, pool_sigma);

                        let active_slots_coeff: FixedDecimal =
                            FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
                        let c = (FixedDecimal::from(1u64) - active_slots_coeff).ln();
                        let block_validator =
                            BlockValidator::new(minted_header, &ledger_state, epoch_nonce, &c);
                        block_validator.validate().or_panic()?;
                        info!(?minted_header.header_body.block_number, "validated block");
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

                let block = {
                    info!("fetching block...");
                    let mut peer_session = stage.peer_session.lock().unwrap();
                    let client = (*peer_session).blockfetch();
                    let block = client.fetch_single(point.clone()).await.or_restart()?;
                    info!("fetched block done.");
                    block
                };

                // info!(?block, "fetched block");

                info!("sending validated block downstream");
                stage
                    .downstream
                    .send(ValidateHeaderEvent::Validated(point.clone(), block).into())
                    .await
                    .or_panic()?;

                info!("sent validated block downstream");

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
