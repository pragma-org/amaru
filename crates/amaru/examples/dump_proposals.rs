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

#![allow(clippy::print_stdout)]
#![allow(clippy::wildcard_enum_match_arm)]

use std::{collections::BTreeMap, fs::File, io::Read, path::PathBuf};

use amaru_kernel::{Epoch, NetworkName, expect_stake_credential};
use amaru_ledger::{
    store::ReadStore,
    summary::{governance::GovernanceSummary, stake_distribution::StakeDistribution},
};
use amaru_stores::rocksdb::{RocksDBHistoricalStores, RocksDbConfig};
use zstd::stream::read::Decoder as ZstdDecoder;

#[derive(serde::Deserialize)]
struct Expected {
    treasury: u64,
    reserves: u64,
    active_stake: u64,
    dreps_voting_stake: u64,
    pools_voting_stake: u64,
    accounts: ExpectedAccounts,
    dreps: ExpectedDReps,
    pools: BTreeMap<String, ExpectedPool>,
}

#[derive(serde::Deserialize)]
struct ExpectedAccounts {
    scripts: BTreeMap<String, ExpectedAccount>,
    verification_keys: BTreeMap<String, ExpectedAccount>,
}

#[derive(serde::Deserialize)]
struct ExpectedAccount {
    balance: u64,
    drep: Option<serde_json::Value>,
    pool: Option<String>,
}

#[derive(serde::Deserialize)]
struct ExpectedDReps {
    abstain: ExpectedVotingStake,
    no_confidence: ExpectedVotingStake,
    scripts: BTreeMap<String, ExpectedDRep>,
    verification_keys: BTreeMap<String, ExpectedDRep>,
}

#[derive(serde::Deserialize)]
struct ExpectedVotingStake {
    voting_stake: u64,
}

#[derive(serde::Deserialize)]
struct ExpectedDRep {
    valid_until: Option<u64>,
    voting_stake: u64,
}

#[derive(serde::Deserialize)]
struct ExpectedPool {
    stake: u64,
    voting_stake: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    let mode = args.get(1).cloned().unwrap_or_else(|| "dump".to_string());
    let ledger_dir = args.get(2).cloned().unwrap_or_else(|| "./ledger.mainnet.db".to_string());
    let epochs = args[3..].iter().map(|s| s.parse::<u64>()).collect::<Result<Vec<_>, _>>()?;

    let network = NetworkName::Mainnet;
    let era_history = network.as_era_history().ok_or("no era history")?;

    for epoch in epochs {
        let epoch = Epoch::from(epoch);
        let snapshot = RocksDBHistoricalStores::for_epoch_with(&RocksDbConfig::new(PathBuf::from(&ledger_dir)), epoch)?;

        println!("==== epoch {epoch} ====");

        if mode == "account" {
            let target = hex::decode("8583857e4a12ffe1e6f641a1785a0f2f036c565cfbe6ff9db8e5a469")?;
            for (credential, row) in snapshot.iter_accounts()? {
                let hash = match &credential {
                    amaru_kernel::StakeCredential::AddrKeyhash(hash) => hash.as_slice(),
                    amaru_kernel::StakeCredential::ScriptHash(hash) => hash.as_slice(),
                };
                if hash == target.as_slice() {
                    println!("  {credential:?}: rewards={} pool={:?} drep={:?}", row.rewards, row.pool, row.drep);
                }
            }
            println!("  pots: {:?}", snapshot.pots()?);
            println!();
            continue;
        }

        if mode == "dump" {
            println!("-- recently pruned --");
            for (id, status) in snapshot.iter_recently_pruned_proposals()? {
                println!("  {id:?} status={status:?}");
            }

            println!("-- proposals --");
            for (id, row) in snapshot.iter_proposals()? {
                let proposed_in = era_history.slot_to_epoch_unchecked_horizon(row.proposed_in.transaction.slot)?;
                let cred = expect_stake_credential(&row.proposal.reward_account);
                let action = match &row.proposal.gov_action {
                    amaru_kernel::GovernanceAction::TreasuryWithdrawals(withdrawals, _) => format!(
                        "TreasuryWithdrawals({})",
                        withdrawals
                            .iter()
                            .map(|(account, amount)| format!("{:?} <- {amount}", expect_stake_credential(account)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    other => format!("{other:?}").chars().take(60).collect::<String>(),
                };
                println!(
                    "  {id:?} proposed_in={proposed_in} valid_until={} deposit={} reward_account={cred:?} action={action}",
                    row.valid_until, row.proposal.deposit,
                );
            }
            println!();
            continue;
        }

        let governance = GovernanceSummary::new(&snapshot, era_history)?;
        let pools_deposits = governance.pools_deposits.clone();
        let stake_distr = StakeDistribution::new(&snapshot, governance)?;

        let fixture_path =
            PathBuf::from(format!("crates/amaru/tests/conformance/stake-distributions/Mainnet/epoch_{epoch}.json.zst"));
        let mut expected_json = String::new();
        ZstdDecoder::new(File::open(&fixture_path)?)?.read_to_string(&mut expected_json)?;
        let expected: Expected = serde_json::from_str(&expected_json)?;
        drop(expected_json);

        let totals = [
            ("treasury", expected.treasury, stake_distr.treasury),
            ("reserves", expected.reserves, stake_distr.reserves),
            ("active_stake", expected.active_stake, stake_distr.active_stake),
            ("dreps_voting_stake", expected.dreps_voting_stake, stake_distr.dreps_voting_stake),
            ("pools_voting_stake", expected.pools_voting_stake, stake_distr.pools_voting_stake),
        ];
        for (name, exp, act) in totals {
            if exp != act {
                println!("total {name}: {exp} vs {act} (delta {})", act as i128 - exp as i128);
            }
        }

        let mut drep_diffs = 0usize;
        for (drep, st) in stake_distr.dreps.iter() {
            use amaru_kernel::DRep;
            let (exp_valid_until, exp_voting_stake, label) = match drep {
                DRep::Abstain => (None, expected.dreps.abstain.voting_stake, "abstain".to_string()),
                DRep::NoConfidence => (None, expected.dreps.no_confidence.voting_stake, "no_confidence".to_string()),
                DRep::Key(hash) => {
                    let key = hex::encode(hash);
                    match expected.dreps.verification_keys.get(&key) {
                        None => {
                            println!("drep vk {key}: missing in haskell fixture");
                            drep_diffs += 1;
                            continue;
                        }
                        Some(exp) => (exp.valid_until, exp.voting_stake, format!("vk {key}")),
                    }
                }
                DRep::Script(hash) => {
                    let key = hex::encode(hash);
                    match expected.dreps.scripts.get(&key) {
                        None => {
                            println!("drep script {key}: missing in haskell fixture");
                            drep_diffs += 1;
                            continue;
                        }
                        Some(exp) => (exp.valid_until, exp.voting_stake, format!("script {key}")),
                    }
                }
            };
            let act_valid_until = st.valid_until.map(u64::from);
            if exp_voting_stake != st.voting_stake
                || (!matches!(drep, DRep::Abstain | DRep::NoConfidence) && exp_valid_until != act_valid_until)
            {
                drep_diffs += 1;
                if drep_diffs <= 20 {
                    println!(
                        "drep {label}: voting_stake {exp_voting_stake} vs {} (delta {}), valid_until {exp_valid_until:?} vs {act_valid_until:?}",
                        st.voting_stake,
                        st.voting_stake as i128 - exp_voting_stake as i128,
                    );
                }
            }
        }
        if drep_diffs > 20 {
            println!("...plus {} more drep diff(s)", drep_diffs - 20);
        }

        let mut account_diffs = 0usize;
        for (credential, account) in stake_distr.accounts.iter() {
            use amaru_kernel::StakeCredential;
            let (key, exp) = match credential {
                StakeCredential::AddrKeyhash(hash) => {
                    let key = hex::encode(hash);
                    (key.clone(), expected.accounts.verification_keys.get(&key))
                }
                StakeCredential::ScriptHash(hash) => {
                    let key = hex::encode(hash);
                    (key.clone(), expected.accounts.scripts.get(&key))
                }
            };
            let Some(exp) = exp else {
                account_diffs += 1;
                if account_diffs <= 20 {
                    println!("account {key}: missing in haskell fixture");
                }
                continue;
            };
            let act_pool = account.pool.as_ref().map(hex::encode);
            let act_drep =
                serde_json::to_value(account.drep.as_ref().map(amaru_kernel::drep::AsJson)).unwrap_or_default();
            let exp_drep = exp.drep.clone().unwrap_or(serde_json::Value::Null);
            if exp.balance != account.balance || exp.pool != act_pool || exp_drep != act_drep {
                account_diffs += 1;
                if account_diffs <= 20 {
                    println!(
                        "account {key}: balance {} vs {} (delta {}), pool {:?} vs {act_pool:?}, drep(expected)={:?} drep(actual)={:?}",
                        exp.balance,
                        account.balance,
                        account.balance as i128 - exp.balance as i128,
                        exp.pool,
                        exp.drep,
                        account.drep,
                    );
                }
            }
        }
        if account_diffs > 20 {
            println!("...plus {} more account diff(s)", account_diffs - 20);
        }

        let amaru_keys = stake_distr
            .accounts
            .keys()
            .map(|credential| match credential {
                amaru_kernel::StakeCredential::AddrKeyhash(hash) => (false, hex::encode(hash)),
                amaru_kernel::StakeCredential::ScriptHash(hash) => (true, hex::encode(hash)),
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut missing = 0usize;
        for (is_script, source) in [(false, &expected.accounts.verification_keys), (true, &expected.accounts.scripts)] {
            for (key, exp) in source.iter() {
                if !amaru_keys.contains(&(is_script, key.clone())) {
                    missing += 1;
                    if missing <= 20 {
                        println!(
                            "account {key} (script={is_script}): in haskell fixture but not in amaru; balance={} pool={:?} drep={:?}",
                            exp.balance, exp.pool, exp.drep
                        );
                    }
                }
            }
        }
        if missing > 20 {
            println!("...plus {} more fixture-only account(s)", missing - 20);
        }

        for (pool_id, pool) in stake_distr.pools.iter() {
            let key = format!("{pool_id}");
            let Some(exp) = expected.pools.get(&key) else {
                println!("pool {key}: missing in haskell fixture");
                continue;
            };
            if exp.stake != pool.stake || exp.voting_stake != pool.voting_stake {
                println!(
                    "pool {key}: stake {} vs {} (delta {}), voting_stake {} vs {} (delta {})",
                    exp.stake,
                    pool.stake,
                    pool.stake as i128 - exp.stake as i128,
                    exp.voting_stake,
                    pool.voting_stake,
                    pool.voting_stake as i128 - exp.voting_stake as i128,
                );
                for (credential, deposit) in pools_deposits.iter() {
                    let delegated = stake_distr
                        .accounts
                        .get(credential)
                        .and_then(|account| account.pool.as_ref())
                        .is_some_and(|delegated| delegated == pool_id);
                    if delegated {
                        println!("  deposit-bearing delegator: {credential:?} deposits={deposit}");
                    }
                }
            }
        }
        println!();
    }

    Ok(())
}
