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

//! Mainnet-observed workload: per-block collection cardinalities for 2160 contiguous blocks
//! ([13586560,13588719], epoch 638), a window chosen for its moderate governance activity.
//! Counts were derived from full transaction data with fragment semantics already applied: intra-block
//! produce/consume cancellation for the UTxO diff, per-block deduplication of credentials, and
//! combined certificates split across the collections they touch. Fragments are synthesized by
//! drawing random entries to match each block's counts, so collection sizes are realistic.

use std::sync::{Arc, LazyLock};

use amaru_kernel::{CertificatePointer, Epoch, ProposalPointer};
use amaru_ledger::{
    context::ProposalState,
    epoch_transition::GovernanceActivity,
    state::volatile::{VolatileDB, VolatileFragment, VolatileSequence},
};
use rand::Rng;

use crate::common::fixture;

const CSV: &str = include_str!("observed_e638.csv");

pub const SAMPLE_NAME: &str = "mainnet-e638@13586560";

#[derive(Debug, Clone, Copy, Default)]
pub struct BlockCounts {
    pub utxo_consumed: usize,
    pub utxo_produced: usize,
    pub pools_registered: usize,
    pub pools_unregistered: usize,
    pub accounts_registered: usize,
    pub accounts_unregistered: usize,
    pub accounts_pool_delegated: usize,
    pub accounts_drep_delegated: usize,
    pub accounts_touched: usize,
    pub dreps_registered: usize,
    pub dreps_unregistered: usize,
    pub committee: usize,
    pub withdrawals: usize,
    pub proposals: usize,
    pub votes: usize,
}

pub static BLOCKS: LazyLock<Vec<BlockCounts>> = LazyLock::new(parse);

pub fn block(ix: usize) -> &'static BlockCounts {
    &BLOCKS[ix % BLOCKS.len()]
}

/// The collections a fragment can be restricted to when attributing memory usage.
pub const GROUPS: &[&str] = &["utxo", "pools", "accounts", "dreps", "committee", "withdrawals", "proposals", "votes"];

pub fn new_volatile_db(rng: &mut impl Rng, volatile_size: usize, only: Option<&str>) -> VolatileDB {
    let mut db = VolatileDB::new(
        Epoch::from(0),
        amaru_kernel::MAINNET_DEFAULT_PROTOCOL_PARAMETERS.clone(),
        GovernanceActivity::default(),
        None,
    );

    (0..volatile_size).for_each(|ix| {
        let mut fragment = VolatileFragment::default();
        let counts = block(ix);
        match only {
            None => fill_fragment(&mut fragment, rng, counts),
            Some("") => (),
            Some(group) => fill_group(&mut fragment, rng, counts, group),
        }
        db.push_back(fragment.anchor(fixture::tip(rng, ix as u64), fixture::default_pool_id()));
    });

    db
}

pub fn fill_fragment(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    GROUPS.iter().for_each(|group| fill_group(fragment, rng, counts, group));
}

fn fill_group(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts, group: &str) {
    match group {
        "utxo" => fill_utxo(fragment, rng, counts),
        "pools" => fill_pools(fragment, rng, counts),
        "accounts" => fill_accounts(fragment, rng, counts),
        "dreps" => fill_dreps(fragment, rng, counts),
        "committee" => fill_committee(fragment, rng, counts),
        "withdrawals" => fill_withdrawals(fragment, rng, counts),
        "proposals" => fill_proposals(fragment, rng, counts),
        "votes" => fill_votes(fragment, rng, counts),
        unknown => unreachable!("unknown collection group {unknown}"),
    }
}

fn fill_utxo(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    for _ in 0..counts.utxo_consumed {
        fragment.utxo.consume(fixture::input(rng));
    }
    for _ in 0..counts.utxo_produced {
        fragment.utxo.produce(fixture::input(rng), Arc::new(fixture::output(rng)));
    }
}

fn fill_pools(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    for _ in 0..counts.pools_registered {
        let params = fixture::pool_params(rng);
        fragment.pools.register(fixture::pool_id(rng), Arc::new((params, CertificatePointer::default(), rng.random())));
    }
    for _ in 0..counts.pools_unregistered {
        fragment.pools.unregister(fixture::pool_id(rng), Epoch::default() + 1);
    }
}

/// Distribute the observed registration/delegation operations over exactly
/// `accounts_touched - accounts_unregistered` distinct credentials.
fn fill_accounts(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    for _ in 0..counts.accounts_unregistered {
        fragment.accounts.unregister(fixture::stake_credential(rng));
    }

    let ops = counts.accounts_registered + counts.accounts_pool_delegated + counts.accounts_drep_delegated;
    if ops == 0 {
        return;
    }

    #[derive(Default)]
    struct Ops {
        register: bool,
        pool: bool,
        drep: bool,
    }

    let distinct = counts.accounts_touched.saturating_sub(counts.accounts_unregistered).max(1);
    let mut credentials = (0..distinct).map(|_| (fixture::stake_credential(rng), Ops::default())).collect::<Vec<_>>();

    let mut slot = 0;
    let mut assign = |n: usize, pick: fn(&mut Ops) -> &mut bool| {
        for _ in 0..n {
            *pick(&mut credentials[slot % distinct].1) = true;
            slot += 1;
        }
    };
    assign(counts.accounts_registered, |ops| &mut ops.register);
    assign(counts.accounts_pool_delegated, |ops| &mut ops.pool);
    assign(counts.accounts_drep_delegated, |ops| &mut ops.drep);

    for (credential, ops) in credentials {
        let pool = ops.pool.then(|| (fixture::pool_id(rng), CertificatePointer::default()));
        let drep = ops.drep.then(|| (fixture::drep(rng), CertificatePointer::default()));
        if ops.register {
            fragment.accounts.register(credential, rng.random(), pool, drep).unwrap();
        } else {
            if pool.is_some() {
                fragment.accounts.bind_left(credential, pool).unwrap();
            }
            if drep.is_some() {
                fragment.accounts.bind_right(credential, drep).unwrap();
            }
        }
    }
}

fn fill_dreps(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    for _ in 0..counts.dreps_registered {
        fragment.dreps.register(fixture::stake_credential(rng), fixture::drep_registration(rng), None, None).unwrap();
    }
    for _ in 0..counts.dreps_unregistered {
        fragment.dreps.unregister(fixture::stake_credential(rng));
    }
}

fn fill_committee(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    for _ in 0..counts.committee {
        fragment
            .committee
            .bind_left(fixture::stake_credential(rng), Some(fixture::stake_credential(rng).into()))
            .unwrap();
    }
}

fn fill_withdrawals(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    for _ in 0..counts.withdrawals {
        fragment.withdrawals.insert(fixture::stake_credential(rng));
    }
}

fn fill_proposals(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    for _ in 0..counts.proposals {
        fragment.proposals.insert(
            fixture::comparable_proposal_id(rng),
            Arc::new(ProposalState {
                proposed_in: ProposalPointer::default(),
                valid_until: Epoch::default(),
                proposal: fixture::proposal(rng),
            }),
        );
    }
}

fn fill_votes(fragment: &mut VolatileFragment, rng: &mut impl Rng, counts: &BlockCounts) {
    for _ in 0..counts.votes {
        fragment.votes.produce(fixture::ballot_id(rng), fixture::ballot(rng));
    }
}

fn parse() -> Vec<BlockCounts> {
    let mut lines = CSV.lines();
    let header = lines.next().unwrap().split(',').collect::<Vec<_>>();
    let column = |name: &str| header.iter().position(|h| *h == name).unwrap();

    let columns = [
        column("utxo_consumed"),
        column("utxo_produced"),
        column("pools_registered"),
        column("pools_unregistered"),
        column("accounts_registered"),
        column("accounts_unregistered"),
        column("accounts_pool_delegated"),
        column("accounts_drep_delegated"),
        column("accounts_touched"),
        column("dreps_registered"),
        column("dreps_unregistered"),
        column("committee"),
        column("withdrawals"),
        column("proposals"),
        column("votes"),
    ];

    lines
        .map(|line| {
            let fields = line.split(',').map(|field| field.parse::<usize>().unwrap()).collect::<Vec<_>>();
            let at = |ix: usize| fields[columns[ix]];
            BlockCounts {
                utxo_consumed: at(0),
                utxo_produced: at(1),
                pools_registered: at(2),
                pools_unregistered: at(3),
                accounts_registered: at(4),
                accounts_unregistered: at(5),
                accounts_pool_delegated: at(6),
                accounts_drep_delegated: at(7),
                accounts_touched: at(8),
                dreps_registered: at(9),
                dreps_unregistered: at(10),
                committee: at(11),
                withdrawals: at(12),
                proposals: at(13),
                votes: at(14),
            }
        })
        .collect()
}
