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

use std::{io, path::PathBuf, time::SystemTime};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{
    ConsensusParameters, Epoch, EraHistory, GlobalParameters, Hasher, Lovelace, NetworkName, Nonce,
    maths::FixedDecimal, num::CheckedSub,
};
use amaru_ledger::{StakeSummary, store::HistoricalStores, summary::governance::GovernanceSummary};
use amaru_ouroboros::{
    PoolSummary, nonce_for_epoch,
    praos::leader::{LeaderParams, lead_slots},
    vrf,
};
use amaru_stores::rocksdb::{RocksDBHistoricalStores, RocksDbConfig, consensus::RocksDBStore};
use anyhow::Context;
use serde::Serialize;

/// Export the slots led by a pool in one epoch.
#[derive(Debug, clap::Args)]
pub struct InspectArgs {
    /// Network whose chain and ledger databases should be read.
    #[command(flatten)]
    network: amaru::args::Network,

    /// Epoch for which to export the schedule. Defaults to the epoch containing the current time.
    #[arg(long, value_name = amaru::value_names::UINT)]
    epoch: Option<Epoch>,

    /// VRF signing key, as raw bytes or an unencrypted cardano-cli envelope.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    vrf_signing_key: PathBuf,

    /// Emit UTC timestamps instead of timestamps in the configured local timezone.
    #[arg(long, action)]
    time_utc: bool,

    /// Chain database containing the epoch nonce and best chain.
    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,

    /// Ledger database containing pool registrations and delegated stake.
    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ScheduleEntry {
    slot: u64,
    relative_slot: u64,
    time: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ScheduleReport {
    epoch: Epoch,
    active_stake: Lovelace,
    pool_stake: Lovelace,
    share: String,
    schedule: Vec<ScheduleEntry>,
}

impl InspectArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    #[expect(clippy::print_stdout)]
    pub fn run(self) -> anyhow::Result<()> {
        let network = NetworkName::from(self.network);

        let global_parameters = network
            .as_global_parameters()
            .context("leader schedules require a built-in network with known global parameters")?;

        let era_history =
            network.as_era_history().context("leader schedules require a built-in network with known era history")?;

        let db_chain = self.db_chain.into_path_buf(network);
        let db_ledger = self.db_ledger.into_path_buf(network);

        let chain = RocksDBStore::open_for_readonly(&RocksDbConfig::new(db_chain.clone()))
            .with_context(|| format!("failed to open chain database at {}", db_chain.display()))?;

        let ledger_config = RocksDbConfig::new(db_ledger.clone());
        let ledger = RocksDBHistoricalStores::new(&ledger_config, 0);

        let vrf = vrf::SecretKey::from_file(&self.vrf_signing_key)
            .with_context(|| format!("failed to load VRF signing key: {}", self.vrf_signing_key.display()))?;
        let vrf_hash = Hasher::<256>::hash(vrf::PublicKey::from(&vrf).as_ref());

        let current_slot = era_history.posix_time_to_slot(SystemTime::now(), global_parameters.system_start())?;
        let epoch = self.epoch.unwrap_or(era_history.slot_to_epoch_unchecked_horizon(current_slot)?);

        let nonce =
            nonce_for_epoch(&chain, epoch).context("the chain database has no nonce for the requested epoch")?;

        let stake_epoch = epoch.checked_sub(Epoch::TWO).context("the requested epoch has no leader stake snapshot")?;

        let snapshot = ledger
            .for_epoch(stake_epoch)
            .with_context(|| format!("failed to open ledger snapshot for epoch {stake_epoch}"))?;

        let stake_summary =
            StakeSummary::new(&snapshot, GovernanceSummary::new(&snapshot, era_history)?, network, |_| {})?;

        // TODO: replace this scan with a VRF -> pool ID lookup once VRF uniqueness validation
        // introduces the corresponding index.
        let (_, pool) = stake_summary
            .pools
            .iter()
            .find(|(_, pool)| pool.parameters.vrf == vrf_hash)
            .context("the ledger snapshot has no pool matching the VRF verification key")?;

        let pool_summary = PoolSummary { vrf: vrf_hash, active_stake: stake_summary.active_stake, stake: pool.stake };

        let schedule = schedule_for_epoch(
            epoch,
            &nonce,
            &pool_summary,
            &vrf,
            global_parameters,
            era_history,
            ScheduleTime { system_start: global_parameters.system_start(), utc: self.time_utc },
        )?;

        serde_json::to_writer_pretty(
            io::stdout().lock(),
            &ScheduleReport {
                epoch,
                active_stake: pool_summary.active_stake,
                pool_stake: pool_summary.stake,
                share: amaru_kernel::utils::string::format_share(pool_summary.stake, pool_summary.active_stake),
                schedule,
            },
        )?;

        println!();

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct ScheduleTime {
    system_start: SystemTime,
    utc: bool,
}

fn schedule_for_epoch(
    epoch: Epoch,
    nonce: &Nonce,
    pool_summary: &PoolSummary,
    vrf: &vrf::SecretKey,
    global_parameters: &GlobalParameters,
    era_history: &EraHistory,
    time: ScheduleTime,
) -> anyhow::Result<Vec<ScheduleEntry>> {
    let bounds = era_history.epoch_bounds(epoch).context("epoch is not covered by the era history")?;
    let end = match bounds.end {
        Some(end) => end,
        None => era_history
            .next_epoch_first_slot(epoch, &bounds.start)
            .context("could not determine the end of the requested epoch")?,
    };
    let consensus = ConsensusParameters::new(global_parameters.clone(), era_history);
    let relative_stake = FixedDecimal::from(pool_summary.stake) / FixedDecimal::from(pool_summary.active_stake);

    lead_slots(
        bounds.start..end,
        &LeaderParams {
            nonce,
            relative_stake: &relative_stake,
            active_slot_coeff_ln_complement: consensus.active_slot_coeff_ln_complement(),
        },
        vrf,
    )
    .into_keys()
    .map(|slot| {
        Ok(ScheduleEntry {
            slot: slot.as_u64(),
            relative_slot: slot.elapsed_from(bounds.start)?,
            time: era_history.format_time(slot, time.system_start, time.utc)?,
        })
    })
    .collect()
}
