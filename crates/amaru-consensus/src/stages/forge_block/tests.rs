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

use std::{
    ops::Range,
    sync::Arc,
    time::{Duration, SystemTime},
};

use amaru_kernel::{
    Epoch, Header, IsHeader, Nonce, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point, Slot, make_header,
    maths::FixedDecimal,
};
use amaru_observability::tracing::Level;
use amaru_ouroboros::vrf;
use amaru_ouroboros_traits::{Nonces, WriteChainStore, in_memory_chain_store::InMemoryChainStore};
use amaru_pure_stage::simulation::Run;

use super::{
    ForgeBlockMsg, FreezeWatch,
    protocol::{AdoptedTip, DueLead},
    schedule::{EpochSchedule, Schedule},
    test_setup::{setup, setup_until_sleeping, test_prep},
};
use crate::stages::test_utils::start_in_era;

fn ready_schedule(epoch: Epoch, slots: Range<Slot>) -> Schedule {
    let nonce = Nonce::from([0u8; 32]);
    let always = FixedDecimal::one();
    let vrf = vrf::SecretKey::from(&[7u8; vrf::SecretKey::SIZE]);
    let mut schedule = Schedule::default();
    schedule.request(epoch, nonce);
    schedule.install(EpochSchedule::compute(epoch, nonce, slots, always, always, &vrf));
    schedule
}

#[test]
fn adopted_origin_records_the_tip_and_does_not_schedule() {
    let prep = test_prep();
    let msg = ForgeBlockMsg::from(AdoptedTip { tip: Point::Origin, parent: Point::Origin });
    let (running, _guards, mut logs, stage) = setup(&prep, msg);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);

    let state = running.get_state(&stage).cloned().unwrap().data;
    assert_eq!(state.adopted_tip, Point::Origin);
    assert!(state.next_lead.is_none());
    assert_eq!(state.schedule.slots(), 0);
}

fn simulation_slot() -> Slot {
    let start = start_in_era();
    // The harness clock is this far after `start_in_era`, on a slot boundary.
    PREPROD_ERA_HISTORY.relative_time_to_slot(start.relative_time + Duration::from_secs(10)).unwrap()
}

#[test]
fn lead_slot_before_certificate_start_is_a_miss() {
    let mut prep = test_prep();
    prep.state.data.ocert_start_period = 1_000_000;
    let slot = simulation_slot();
    prep.state.data.adopted_tip = Point::Specific(slot, amaru_kernel::ORIGIN_HASH, 1.into());
    let msg = ForgeBlockMsg::from(DueLead { slot, generation: 0 });
    let (running, _guards, mut logs, stage) = setup(&prep, msg);

    logs.assert_and_remove(Level::WARN, &["ocert_not_yet_valid"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);

    let state = running.get_state(&stage).cloned().unwrap().data;
    assert!(state.next_lead.is_none());
}

#[test]
fn stale_lead_slot_does_not_forge() {
    let mut prep = test_prep();
    let slot = Slot::from(11);
    prep.state.data.schedule_generation = 2;
    prep.state.data.schedule = ready_schedule(Epoch::from(0), slot..slot + 1);
    prep.state.data.adopted_tip = Point::Specific(slot, amaru_kernel::ORIGIN_HASH, 1.into());
    let msg = ForgeBlockMsg::from(DueLead { slot, generation: 1 });
    let (running, _guards, mut logs, stage) = setup(&prep, msg);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
    let state = running.get_state(&stage).cloned().unwrap().data;
    assert_eq!(state.schedule.slots(), 1);
    // The stale handler does not reschedule, so the generation stays put.
    assert_eq!(state.schedule_generation, 2);
    assert!(state.next_lead.is_none());
}

#[test]
fn an_accepted_lead_arms_the_following_slot() {
    let mut prep = test_prep();
    prep.state.data.ocert_start_period = 1_000_000;
    // Inside the open forge window, so the miss is the certificate rather than a late wake.
    let slot = simulation_slot();
    prep.state.data.schedule = ready_schedule(start_in_era().epoch, slot..slot + 2);
    prep.state.data.adopted_tip = Point::Specific(slot, amaru_kernel::ORIGIN_HASH, 1.into());
    let msg = ForgeBlockMsg::from(DueLead { slot, generation: 0 });
    let (running, _guards, mut logs, stage) = setup_until_sleeping(&prep, msg);

    let onset =
        prep.state.data.consensus_parameters.era_history().slot_to_relative_time_unchecked_horizon(slot + 1).unwrap();
    let wall = SystemTime::UNIX_EPOCH + Duration::from_millis(prep.state.data.system_start_unix_ms) + onset;
    let timestamp = super::calc::format_utc_timestamp(wall).unwrap();
    logs.assert_and_remove(Level::WARN, &["ocert_not_yet_valid"])
        .assert_and_remove(Level::INFO, &[timestamp.as_str()])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);

    let state = running.get_state(&stage).cloned().unwrap().data;
    assert!(state.schedule.lead_at(slot).is_none());
    assert!(state.schedule.lead_at(slot + 1).is_some());
    assert!(state.next_lead.is_some());
    assert_eq!(state.schedule_generation, 1);
}

#[test]
fn a_late_due_lead_is_not_forged() {
    let mut prep = test_prep();
    let slot = Slot::from(u64::from(simulation_slot()).saturating_sub(5));
    prep.state.data.schedule = ready_schedule(Epoch::from(0), slot..slot + 1);
    prep.state.data.adopted_tip = Point::Specific(slot, amaru_kernel::ORIGIN_HASH, 1.into());
    let msg = ForgeBlockMsg::from(DueLead { slot, generation: 0 });
    let (running, _guards, mut logs, stage) = setup(&prep, msg);

    logs.assert_and_remove(Level::WARN, &["woke_late"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);

    let state = running.get_state(&stage).cloned().unwrap().data;
    assert!(state.schedule.lead_at(slot).is_none());
    assert!(state.next_lead.is_none());
}

#[test]
fn an_early_due_lead_waits_for_the_forge_window() {
    let mut prep = test_prep();
    prep.state.data.ocert_start_period = 1_000_000;
    let slot = simulation_slot() + 100;
    prep.state.data.schedule = ready_schedule(start_in_era().epoch, slot..slot + 1);
    let msg = ForgeBlockMsg::from(DueLead { slot, generation: 0 });
    let (_running, _guards, mut logs, _stage) = setup_until_sleeping(&prep, msg);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

fn current_era_epochs() -> (Epoch, Slot, Slot) {
    let start = start_in_era();
    let next = PREPROD_ERA_HISTORY.next_epoch_first_slot(start.epoch, &start.slot).expect("next epoch");
    let window = PREPROD_GLOBAL_PARAMETERS.randomness_stabilization_window();
    let freeze = Slot::from(u64::from(next) - window);
    (start.epoch, freeze, next)
}

struct Chain {
    store: Arc<InMemoryChainStore>,
    tip: Point,
}

impl Chain {
    fn new() -> Self {
        Self { store: Arc::new(InMemoryChainStore::new()), tip: Point::Origin }
    }

    fn extend(&mut self, slot: Slot, height: u64) -> Header {
        let parent = match self.tip {
            Point::Origin => None,
            Point::Specific(_, hash, _) => Some(hash),
        };
        let header = make_header(height, u64::from(slot), parent);
        self.store.store_validated_header(&header, &Nonces::for_tests()).unwrap();
        self.store.roll_forward_chain(&header.point()).unwrap();
        self.tip = header.point();
        header
    }

    fn fork_from(&mut self, parent: &Header, slot: Slot, height: u64) -> Header {
        let header = make_header(height, u64::from(slot), Some(parent.hash()));
        self.store.store_validated_header(&header, &Nonces::for_tests()).unwrap();
        self.store.switch_to_fork(&parent.point(), &[header.point()]).unwrap();
        self.tip = header.point();
        header
    }
}

fn adopted(header: &Header, parent: Point) -> ForgeBlockMsg {
    ForgeBlockMsg::from(AdoptedTip { tip: header.point(), parent })
}

fn watch_epoch(watch: &Option<FreezeWatch>) -> Option<Epoch> {
    watch.as_ref().map(|w| w.epoch)
}

#[test]
fn adopted_tip_enters_freeze_and_predicts_next_epoch() {
    let mut prep = test_prep();
    prep.state.data.k = 2;
    let (epoch, freeze_slot, _) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let header = chain.extend(freeze_slot, 1);
    let (running, _guards, _logs, stage) = setup(&prep, adopted(&header, Point::Origin));
    let state = running.get_state(&stage).cloned().unwrap().data;
    assert_eq!(watch_epoch(&state.freeze), Some(epoch));
    assert!(state.schedule.knows(epoch + 1));
}

#[test]
fn adopted_tip_holds_freeze_until_k_deep_across_epoch_boundary() {
    let mut prep = test_prep();
    prep.state.data.k = 10;
    let (epoch, freeze_slot, next_epoch_slot) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let freeze = chain.extend(freeze_slot, 1);
    let (mut running, _guards, _logs, stage) = setup(&prep, adopted(&freeze, Point::Origin));
    let next = chain.extend(next_epoch_slot, 2);
    running.enqueue_msg(&stage, [adopted(&next, freeze.point())]);
    running.run(Run::skip_and_resolve());
    let state = running.get_state(&stage).cloned().unwrap().data;
    assert_eq!(watch_epoch(&state.freeze), Some(epoch));
    // The schedule computed from that freeze is now the current epoch's, and is kept.
    assert!(state.schedule.knows(epoch + 1));
}

#[test]
fn adopted_tip_clears_freeze_after_k_blocks() {
    let mut prep = test_prep();
    prep.state.data.k = 2;
    let (epoch, freeze_slot, next_epoch_slot) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let freeze = chain.extend(freeze_slot, 1);
    let (mut running, _guards, _logs, stage) = setup(&prep, adopted(&freeze, Point::Origin));
    let h2 = chain.extend(next_epoch_slot, 2);
    running.enqueue_msg(&stage, [adopted(&h2, freeze.point())]);
    running.run(Run::skip_and_resolve());
    let h3 = chain.extend(next_epoch_slot + 1, 3);
    running.enqueue_msg(&stage, [adopted(&h3, h2.point())]);
    running.run(Run::skip_and_resolve());
    let state = running.get_state(&stage).cloned().unwrap().data;
    assert_eq!(watch_epoch(&state.freeze), None);
    assert!(state.schedule.knows(epoch + 1));
    assert!(!state.schedule.knows(epoch + 2));
}

#[test]
fn adopted_tip_enters_the_next_epoch_freeze_after_settling() {
    let mut prep = test_prep();
    prep.state.data.k = 2;
    let (epoch, freeze_slot, next_epoch_slot) = current_era_epochs();
    let window = PREPROD_GLOBAL_PARAMETERS.randomness_stabilization_window();
    let next_next = PREPROD_ERA_HISTORY.next_epoch_first_slot(epoch + 1, &next_epoch_slot).expect("epoch after next");
    let next_freeze = Slot::from(u64::from(next_next) - window);
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let freeze = chain.extend(freeze_slot, 1);
    let (mut running, _guards, _logs, stage) = setup(&prep, adopted(&freeze, Point::Origin));
    let h2 = chain.extend(next_epoch_slot, 2);
    running.enqueue_msg(&stage, [adopted(&h2, freeze.point())]);
    running.run(Run::skip_and_resolve());
    let h3 = chain.extend(next_epoch_slot + 1, 3);
    running.enqueue_msg(&stage, [adopted(&h3, h2.point())]);
    running.run(Run::skip_and_resolve());
    let next_window = chain.extend(next_freeze, 4);
    running.enqueue_msg(&stage, [adopted(&next_window, h3.point())]);
    running.run(Run::skip_and_resolve());
    let state = running.get_state(&stage).cloned().unwrap().data;
    assert_eq!(watch_epoch(&state.freeze), Some(epoch + 1));
    assert!(state.schedule.knows(epoch + 2));
}

#[test]
fn adopted_tip_resets_freeze_when_a_fork_drops_the_watch() {
    let mut prep = test_prep();
    prep.state.data.k = 2;
    let (epoch, freeze_slot, _) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let before = chain.extend(Slot::from(u64::from(freeze_slot) - 10), 1);
    let freeze = chain.extend(freeze_slot, 2);
    let (mut running, _guards, _logs, stage) = setup(&prep, adopted(&freeze, before.point()));
    let fork = chain.fork_from(&before, freeze_slot + 1, 2);
    running.enqueue_msg(&stage, [adopted(&fork, before.point())]);
    running.run(Run::skip_and_resolve());
    let state = running.get_state(&stage).cloned().unwrap().data;
    assert_eq!(watch_epoch(&state.freeze), Some(epoch));
    assert_eq!(state.freeze.as_ref().map(|w| w.point.hash()), Some(fork.hash()));
    assert!(state.schedule.knows(epoch + 1));
}

#[test]
fn adopted_tip_clears_freeze_on_a_switch_before_the_window() {
    let mut prep = test_prep();
    prep.state.data.k = 2;
    let (epoch, freeze_slot, _) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let before = chain.extend(Slot::from(u64::from(freeze_slot) - 10), 1);
    let freeze = chain.extend(freeze_slot, 2);
    let (mut running, _guards, _logs, stage) = setup(&prep, adopted(&freeze, before.point()));
    // Greater height, earlier slot: the only kind of switch `cmp_tip` will adopt.
    let switched = chain.fork_from(&before, Slot::from(u64::from(freeze_slot) - 5), 3);
    running.enqueue_msg(&stage, [adopted(&switched, before.point())]);
    running.run(Run::skip_and_resolve());
    let state = running.get_state(&stage).cloned().unwrap().data;
    assert_eq!(watch_epoch(&state.freeze), None);
    // The fork dropped that schedule and this tip is before the window, so nothing replaced it.
    assert!(!state.schedule.knows(epoch + 1));
}
