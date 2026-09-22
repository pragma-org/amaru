use std::{
    collections::BTreeMap,
    ops::{Bound, Range},
};

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

    pub fn next_after<'a>(&'a self, slot: Slot) -> Option<LeadSlot<'a>> {
        let (found, cert) = self.slots.range((Bound::Excluded(slot), Bound::Unbounded)).next()?;

        Some(LeadSlot { slot: *found, cert })
    }

    pub fn at<'a>(&'a self, slot: Slot) -> Option<LeadSlot<'a>> {
        self.slots.get(&slot).map(|cert| LeadSlot { cert, slot })
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn has_slots(&self) -> bool {
        !self.slots.is_empty()
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

    pub fn prune(&mut self, keep_from: Epoch) {
        self.epochs.retain(|&epoch, _| epoch >= keep_from);
    }

    pub fn knows(&self, epoch: Epoch) -> bool {
        self.epochs.contains_key(&epoch)
    }

    pub fn next_lead(&self, after: Slot) -> Option<LeadSlot<'_>> {
        self.ready().find_map(|schedule| schedule.next_after(after))
    }

    pub fn lead_at(&self, slot: Slot) -> Option<LeadSlot<'_>> {
        self.ready().find_map(|schedule| schedule.at(slot))
    }

    pub fn slots(&self) -> usize {
        self.ready().map(EpochSchedule::len).sum()
    }

    fn ready(&self) -> impl Iterator<Item = &EpochSchedule> {
        self.epochs.values().filter_map(|state| match state {
            ScheduleState::Ready(schedule) => Some(schedule),
            ScheduleState::Pending(_) => None,
        })
    }
}
