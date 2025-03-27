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
    encode_bech32, Anchor, DRep, Epoch, StakeCredential, DREP_EXPIRY, GOV_ACTION_LIFETIME,
};
use serde::ser::SerializeStruct;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug)]
pub struct DRepsSummary {
    pub dreps: BTreeMap<DRep, DRepState>,
}

#[derive(Debug, serde::Serialize)]
pub struct DRepState {
    pub mandate: Epoch,
    pub metadata: Option<Anchor>,
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

        let dreps = db
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
                            mandate: last_interaction
                                + DREP_EXPIRY
                                + epochs_without_active_proposals as u64,
                        },
                    )
                },
            )
            .collect::<BTreeMap<_, _>>();

        Ok(DRepsSummary { dreps })
    }
}

impl serde::Serialize for DRepsSummary {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("DRepsSummary", 1)?;

        let mut dreps = BTreeMap::new();
        self.dreps.iter().for_each(|(drep, st)| {
            if let Some(id) = into_drep_id(drep) {
                dreps.insert(id, st);
            }
        });
        s.serialize_field("dreps", &dreps)?;

        s.end()
    }
}

/// Serialize a (registerd) DRep to bech32, according to [CIP-0129](https://cips.cardano.org/cip/CIP-0129).
/// The always-Abstain and always-NoConfidence dreps are ignored (i.e. return `None`).
///
/// ```rust
/// use amaru_kernel::{DRep, Hash};
/// use amaru_ledger::summary::governance::into_drep_id;
///
/// let key_drep = DRep::Key(Hash::from(
///   hex::decode("7a719c71d1bc67d2eb4af19f02fd48e7498843d33a22168111344a34")
///     .unwrap()
///     .as_slice()
/// ));
///
/// let script_drep = DRep::Script(Hash::from(
///   hex::decode("429b12461640cefd3a4a192f7c531d8f6c6d33610b727f481eb22d39")
///     .unwrap()
///     .as_slice()
/// ));
///
/// assert_eq!(into_drep_id(&DRep::Abstain), None);
///
/// assert_eq!(into_drep_id(&DRep::NoConfidence), None);
///
/// assert_eq!(
///   into_drep_id(&key_drep).as_deref(),
///   Some("drep1yfa8r8r36x7x05htftce7qhafrn5nzzr6vazy95pzy6y5dqac0ss7"),
/// );
///
/// assert_eq!(
///   into_drep_id(&script_drep).as_deref(),
///   Some("drep1ydpfkyjxzeqvalf6fgvj7lznrk8kcmfnvy9hyl6gr6ez6wgsjaelx"),
/// );
/// ```
pub fn into_drep_id(drep: &DRep) -> Option<String> {
    match drep {
        DRep::Key(hash) => encode_bech32("drep", &[&[34], hash.as_slice()].concat()).ok(),
        DRep::Script(hash) => encode_bech32("drep", &[&[35], hash.as_slice()].concat()).ok(),
        DRep::Abstain | DRep::NoConfidence => None,
    }
}
