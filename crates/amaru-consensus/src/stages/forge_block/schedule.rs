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

//! Leader schedules for the epochs this stage is forging.
//!
//! An epoch is pending from the moment its nonce is requested until a result
//! with that same nonce is installed. A mismatched result was computed from a
//! nonce a rollback has already replaced, and is dropped.

use std::{collections::BTreeMap, ops::Range};

use amaru_kernel::{Epoch, Nonce, Slot, VrfCert, maths::FixedDecimal};
use amaru_ouroboros::{
    praos::leader::{LeaderParams, lead_slots},
    vrf,
};

pub struct LeadSlot<'a> {
    slot: Slot,
    cert: &'a VrfCert,
}

impl LeadSlot<'_> {
    pub fn slot(&self) -> Slot {
        self.slot
    }

    pub fn cert(&self) -> &VrfCert {
        self.cert
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EpochSchedule {
    epoch: Epoch,
    nonce: Nonce,
    slots: BTreeMap<Slot, VrfCert>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ScheduleState {
    Pending(Nonce),
    Ready(EpochSchedule),
}

#[derive(Default, Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Schedule {
    epochs: BTreeMap<Epoch, ScheduleState>,
}

impl EpochSchedule {
    /// Run leader election over `slots`, the epoch's slot range.
    pub fn compute(
        epoch: Epoch,
        nonce: Nonce,
        slots: Range<Slot>,
        relative_stake: &FixedDecimal,
        active_slot_coeff: &FixedDecimal,
        vrf: &vrf::SecretKey,
    ) -> Self {
        let params = LeaderParams { nonce: &nonce, relative_stake, active_slot_coeff };
        Self { epoch, nonce, slots: lead_slots(slots, &params, vrf) }
    }

    pub fn empty(epoch: Epoch, nonce: Nonce) -> Self {
        Self { epoch, nonce, slots: BTreeMap::new() }
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    #[cfg(test)]
    pub(crate) fn slots(&self) -> &BTreeMap<Slot, VrfCert> {
        &self.slots
    }

    fn at(&self, slot: Slot) -> Option<LeadSlot<'_>> {
        self.slots.get(&slot).map(|cert| LeadSlot { slot, cert })
    }
}

impl Schedule {
    pub fn request(&mut self, epoch: Epoch, nonce: Nonce) -> bool {
        match self.epochs.get(&epoch) {
            Some(ScheduleState::Pending(pending)) if *pending == nonce => false,
            Some(ScheduleState::Ready(ready)) if ready.nonce == nonce => false,
            _ => {
                self.epochs.insert(epoch, ScheduleState::Pending(nonce));
                true
            }
        }
    }

    pub fn install(&mut self, schedule: EpochSchedule) -> bool {
        match self.epochs.get(&schedule.epoch) {
            Some(ScheduleState::Pending(pending)) if *pending == schedule.nonce => {
                self.epochs.insert(schedule.epoch, ScheduleState::Ready(schedule));
                true
            }
            _ => false,
        }
    }

    pub fn forget(&mut self, epoch: Epoch) {
        self.epochs.remove(&epoch);
    }

    /// Remove every led slot at or before `slot`.
    ///
    /// A lead is armed about 50ms before onset, so the slot just handled is still
    /// ahead of the clock and would otherwise be selected again.
    pub fn drop_through(&mut self, slot: Slot) {
        for state in self.epochs.values_mut() {
            if let ScheduleState::Ready(schedule) = state {
                schedule.slots.retain(|&scheduled, _| scheduled > slot);
            }
        }
    }

    pub fn prune_before(&mut self, keep_from: Epoch) {
        self.epochs.retain(|&epoch, _| epoch >= keep_from);
    }

    pub fn knows(&self, epoch: Epoch) -> bool {
        self.epochs.contains_key(&epoch)
    }

    /// Led slots in time order, across every ready epoch.
    pub fn leads(&self) -> impl Iterator<Item = LeadSlot<'_>> {
        self.ready().flat_map(|schedule| schedule.slots.iter().map(|(&slot, cert)| LeadSlot { slot, cert }))
    }

    pub fn lead_at(&self, slot: Slot) -> Option<LeadSlot<'_>> {
        self.ready().find_map(|schedule| schedule.at(slot))
    }

    /// Led slots still held in each ready epoch. Pending epochs are omitted.
    pub fn led_counts(&self) -> BTreeMap<Epoch, usize> {
        self.epochs
            .iter()
            .filter_map(|(&epoch, state)| match state {
                ScheduleState::Ready(schedule) => Some((epoch, schedule.slots.len())),
                ScheduleState::Pending(_) => None,
            })
            .collect()
    }

    pub fn slots(&self) -> usize {
        self.ready().map(|schedule| schedule.slots.len()).sum()
    }

    fn ready(&self) -> impl Iterator<Item = &EpochSchedule> {
        self.epochs.values().filter_map(|state| match state {
            ScheduleState::Ready(schedule) => Some(schedule),
            ScheduleState::Pending(_) => None,
        })
    }
}
