// Copyright 2025 PRAGMA
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

use crate::store::{columns::dreps::Row, Snapshot, StoreError};
use amaru_kernel::{
    Anchor, DRep, Epoch, Lovelace, StakeCredential, DREP_EXPIRY, GOV_ACTION_LIFETIME,
};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug)]
pub struct DRepsSummary {
    pub dreps: BTreeMap<DRep, DRepState>,
}

#[derive(Debug, serde::Serialize)]
pub struct DRepState {
    pub mandate: Option<Epoch>,
    pub metadata: Option<Anchor>,
    pub stake: Lovelace,
}

impl DRepsSummary {
    pub fn new(db: &impl Snapshot) -> Result<Self, StoreError> {
        let all_proposals_epochs = db
            .iter_proposals()?
            .map(|(_, row)| row.epoch)
            .collect::<HashSet<_>>();
        // TODO filter out proposals that have been ratified

        let current_epoch = db.epoch();

        // A set containing all overlapping activity periods of all proposals. Might contain disjoint periods.
        // e.g.
        //
        // Considering a proposal created at epoch 163, it is valid until epoch 163 + GOV_ACTION_LIFETIME
        //
        // for epochs 163 and 165, with GOV_ACTION_LIFETIME = 6, proposals_activity_periods would equal
        //   [163, 164, 165, 166, 167, 168, 169, 170, 171]
        //
        // for epochs 163 and 172, with GOV_ACTION_LIFETIME = 6, proposals_activity_periods would equal
        //   [163, 164, 165, 166, 167, 168, 169, 172, 173, 174, 175, 176, 177, 178]
        let proposals_activity_periods = all_proposals_epochs
            .iter()
            .flat_map(|&value| (value..=value + GOV_ACTION_LIFETIME).collect::<HashSet<_>>())
            .collect::<HashSet<u64>>();

        let mut dreps = db
            .iter_dreps()?
            .map(
                |(
                    k,
                    Row {
                        last_interaction,
                        anchor,
                        ..
                    },
                )| {
                    let drep = match k {
                        StakeCredential::AddrKeyhash(hash) => DRep::Key(hash),
                        StakeCredential::ScriptHash(hash) => DRep::Script(hash),
                    };

                    // Each epoch with no active proposals increase the mandate by 1
                    let epochs_without_active_proposals =
                        HashSet::from_iter(last_interaction..=current_epoch) // Total period considered for this DRep
                            .difference(&proposals_activity_periods)
                            .count();

                    (
                        drep,
                        DRepState {
                            metadata: anchor,
                            mandate: Some(
                                last_interaction
                                    + DREP_EXPIRY
                                    + epochs_without_active_proposals as u64,
                            ),
                            stake: 0, // NOTE: The actual stake is filled later when computing the
                                      // stake distribution.
                        },
                    )
                },
            )
            .collect::<BTreeMap<_, _>>();

        dreps.insert(
            DRep::Abstain,
            DRepState {
                mandate: None,
                metadata: None,
                stake: 0,
            },
        );
        dreps.insert(
            DRep::NoConfidence,
            DRepState {
                mandate: None,
                metadata: None,
                stake: 0,
            },
        );

        Ok(DRepsSummary { dreps })
    }
}
