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

//! Regression test for DRep expiry across a bootstrap taken during dormant governance.
//!
//! A node bootstrapped from a snapshot carries an imported `numDormantEpochs` counter. The
//! effective expiry surfaced by [`GovernanceSummary`] is `stored valid_until +
//! consecutive_dormant_epochs`, so the imported counter must survive the import.
//!
//! Bootstrap imports in two steps: an authoritative save (`save_point`) that carries the counter,
//! then a governance-less UTxO import (`import_utxo_from_tvar`). This test reproduces that exact
//! two-save sequence at the store -> `GovernanceSummary` boundary: the follow-up UTxO save must not
//! clobber the counter recorded by the first. A single-save test cannot expose that; only the
//! second save does.

use amaru_kernel::{
    CertificatePointer, DRep, DRepRegistration, Hash, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PREPROD_ERA_HISTORY, Point,
    Slot, StakeCredential, TransactionPointer,
};
use amaru_ledger::{
    epoch_transition::GovernanceActivity,
    state::diff_bind::Resettable,
    store::{Columns, EpochTransitionProgress, ReadStore, Store, TransactionalContext},
    summary::governance::GovernanceSummary,
};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig};

const DORMANT_EPOCHS: u32 = 19;

#[test]
fn bootstrap_preserves_dormant_epochs_across_utxo_import() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let cfg = RocksDbConfig::new(tmp_dir.path().into());

    // A snapshot always sits at the last slot of an epoch; pick any preprod slot.
    let slot = Slot::from(50_000_000u64);
    let point = Point::Specific(slot, Hash::from([0u8; 32]));
    let epoch = PREPROD_ERA_HISTORY.slot_to_epoch(slot, slot).expect("slot_to_epoch");

    let drep_hash = Hash::from([1u8; 28]);
    let drep_key = StakeCredential::AddrKeyhash(drep_hash);
    let registered_at = CertificatePointer {
        transaction: TransactionPointer { slot: Slot::from(0), transaction_index: 0 },
        certificate_index: 0,
    };

    let raw_valid_until = {
        // `RocksDB::empty` is the bootstrap/import mode.
        let store = RocksDB::empty(&cfg).expect("open empty store");

        // 1. Authoritative import, as `save_point` performs it: a DRep together with the imported
        //    dormant-epoch counter.
        {
            let context = store.create_transaction();
            context
                .save(
                    &PREPROD_ERA_HISTORY,
                    &PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
                    GovernanceActivity { consecutive_dormant_epochs: DORMANT_EPOCHS },
                    &point,
                    None,
                    Columns {
                        utxo: std::iter::empty(),
                        pools: std::iter::empty(),
                        accounts: std::iter::empty(),
                        dreps: std::iter::once((
                            drep_key.clone(),
                            (
                                Resettable::Unchanged,
                                Some(DRepRegistration { deposit: 500_000_000, registered_at, valid_until: epoch }),
                            ),
                        )),
                        cc_members: std::iter::empty(),
                        proposals: std::iter::empty(),
                        votes: std::iter::empty(),
                    },
                    Columns::empty(),
                    std::iter::empty(),
                )
                .expect("import save");
            context.commit().expect("commit");
        }

        // 2. Follow-up UTxO import, as `import_utxo_from_tvar` performs it: it carries no governance
        //    state and passes a placeholder `GovernanceActivity::default()`, which must not clobber
        //    the counter recorded above.
        {
            let context = store.create_transaction();
            context
                .save(
                    &PREPROD_ERA_HISTORY,
                    &PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
                    GovernanceActivity::default(),
                    &point,
                    None,
                    Columns::empty(),
                    Columns::empty(),
                    std::iter::empty(),
                )
                .expect("utxo import save");
            context.commit().expect("commit");
        }

        // The raw expiry actually persisted for the DRep (before the summary re-applies dormancy).
        let raw_valid_until = store
            .iter_dreps()
            .expect("iter dreps")
            .find(|(k, _)| k == &drep_key)
            .map(|(_, row)| row.valid_until)
            .expect("drep persisted");

        store.next_snapshot(epoch).expect("snapshot");
        store
            .with_transaction(|batch| batch.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken)))
            .expect("epoch transition");

        raw_valid_until
    };

    // Re-open the epoch snapshot and compute the governance summary the conformance test uses.
    let snapshot = RocksDBHistoricalStores::for_epoch_with(&cfg, epoch).expect("open snapshot");
    let summary = GovernanceSummary::new(&snapshot, &PREPROD_ERA_HISTORY).expect("governance summary");

    let reported = summary.dreps.get(&DRep::Key(drep_hash)).expect("drep present in summary").valid_until;

    assert_eq!(
        reported,
        Some(raw_valid_until + DORMANT_EPOCHS as u64),
        "the imported dormant-epoch counter must survive the follow-up UTxO import and be re-applied \
         to DRep expiry (raw valid_until={raw_valid_until:?}, dormant_epochs={DORMANT_EPOCHS})",
    );
}
