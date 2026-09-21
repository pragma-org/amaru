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

use std::sync::Arc;

use amaru_kernel::{Epoch, Header, IsHeader, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Point, Slot, make_header};
use amaru_observability::tracing::Level;
use amaru_ouroboros_traits::{Nonces, WriteChainStore, in_memory_chain_store::InMemoryChainStore};
use amaru_pure_stage::simulation::Run;

use super::{
    ForgeBlockMsg, FreezeWatch,
    protocol::{AdoptedTip, LeadSlot},
    test_setup::{setup, test_prep},
};
use crate::stages::test_utils::start_in_era;

#[test]
fn adopted_origin_records_the_tip_and_does_not_schedule() {
    let prep = test_prep();
    let msg = ForgeBlockMsg::from(AdoptedTip { tip: Point::Origin, parent: Point::Origin });
    let (running, _guards, mut logs, stage) = setup(&prep, msg);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);

    let state = running.get_state(&stage).cloned().unwrap();
    assert_eq!(state.adopted_tip, Point::Origin);
    assert!(state.next_lead.is_none());
    assert!(state.led_slots.is_empty());
}

#[test]
fn lead_slot_before_certificate_start_is_a_miss() {
    let mut prep = test_prep();
    prep.state.ocert_start_period = 1_000_000;
    prep.state.adopted_tip = Point::Specific(Slot::from(10), amaru_kernel::ORIGIN_HASH, 1.into());
    let msg = ForgeBlockMsg::from(LeadSlot { slot: Slot::from(11), generation: 0 });
    let (running, _guards, mut logs, stage) = setup(&prep, msg);

    logs.assert_and_remove(Level::WARN, &["ocert_not_yet_valid"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);

    let state = running.get_state(&stage).cloned().unwrap();
    assert!(state.next_lead.is_none());
}

#[test]
fn stale_lead_slot_does_not_forge() {
    let mut prep = test_prep();
    let slot = Slot::from(11);
    prep.state.schedule_generation = 2;
    prep.state.led_slots = vec![slot];
    prep.state.adopted_tip = Point::Specific(slot, amaru_kernel::ORIGIN_HASH, 1.into());
    let msg = ForgeBlockMsg::from(LeadSlot { slot, generation: 1 });
    let (running, _guards, mut logs, stage) = setup(&prep, msg);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
    let state = running.get_state(&stage).cloned().unwrap();
    assert_eq!(state.led_slots, vec![slot]);
    // The stale handler does not reschedule, so the generation stays put.
    assert_eq!(state.schedule_generation, 2);
    assert!(state.next_lead.is_none());
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
    prep.state.k = 2;
    let (epoch, freeze_slot, _) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let header = chain.extend(freeze_slot, 1);
    let (running, _guards, _logs, stage) = setup(&prep, adopted(&header, Point::Origin));
    let state = running.get_state(&stage).cloned().unwrap();
    assert_eq!(watch_epoch(&state.freeze), Some(epoch));
    assert_eq!(state.predicted_epoch, Some(epoch + 1));
}

#[test]
fn adopted_tip_holds_freeze_until_k_deep_across_epoch_boundary() {
    let mut prep = test_prep();
    prep.state.k = 10;
    let (epoch, freeze_slot, next_epoch_slot) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let freeze = chain.extend(freeze_slot, 1);
    let (mut running, _guards, _logs, stage) = setup(&prep, adopted(&freeze, Point::Origin));
    let next = chain.extend(next_epoch_slot, 2);
    running.enqueue_msg(&stage, [adopted(&next, freeze.point())]);
    running.run(Run::skip_and_resolve());
    let state = running.get_state(&stage).cloned().unwrap();
    assert_eq!(watch_epoch(&state.freeze), Some(epoch));
    assert_eq!(state.predicted_epoch, None);
}

#[test]
fn adopted_tip_clears_freeze_after_k_blocks() {
    let mut prep = test_prep();
    prep.state.k = 2;
    let (_epoch, freeze_slot, next_epoch_slot) = current_era_epochs();
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
    let state = running.get_state(&stage).cloned().unwrap();
    assert_eq!(watch_epoch(&state.freeze), None);
    assert_eq!(state.predicted_epoch, None);
}

#[test]
fn adopted_tip_enters_the_next_epoch_freeze_after_settling() {
    let mut prep = test_prep();
    prep.state.k = 2;
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
    let state = running.get_state(&stage).cloned().unwrap();
    assert_eq!(watch_epoch(&state.freeze), Some(epoch + 1));
    assert_eq!(state.predicted_epoch, Some(epoch + 2));
}

#[test]
fn adopted_tip_resets_freeze_when_a_fork_drops_the_watch() {
    let mut prep = test_prep();
    prep.state.k = 2;
    let (epoch, freeze_slot, _) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let before = chain.extend(Slot::from(u64::from(freeze_slot) - 10), 1);
    let freeze = chain.extend(freeze_slot, 2);
    let (mut running, _guards, _logs, stage) = setup(&prep, adopted(&freeze, before.point()));
    let fork = chain.fork_from(&before, freeze_slot + 1, 2);
    running.enqueue_msg(&stage, [adopted(&fork, before.point())]);
    running.run(Run::skip_and_resolve());
    let state = running.get_state(&stage).cloned().unwrap();
    assert_eq!(watch_epoch(&state.freeze), Some(epoch));
    assert_eq!(state.freeze.as_ref().map(|w| w.point.hash()), Some(fork.hash()));
    assert_eq!(state.predicted_epoch, Some(epoch + 1));
}

#[test]
fn adopted_tip_clears_freeze_on_a_switch_before_the_window() {
    let mut prep = test_prep();
    prep.state.k = 2;
    let (_epoch, freeze_slot, _) = current_era_epochs();
    let mut chain = Chain::new();
    prep.store = chain.store.clone();
    let before = chain.extend(Slot::from(u64::from(freeze_slot) - 10), 1);
    let freeze = chain.extend(freeze_slot, 2);
    let (mut running, _guards, _logs, stage) = setup(&prep, adopted(&freeze, before.point()));
    // Greater height, earlier slot: the only kind of switch `cmp_tip` will adopt.
    let switched = chain.fork_from(&before, Slot::from(u64::from(freeze_slot) - 5), 3);
    running.enqueue_msg(&stage, [adopted(&switched, before.point())]);
    running.run(Run::skip_and_resolve());
    let state = running.get_state(&stage).cloned().unwrap();
    assert_eq!(watch_epoch(&state.freeze), None);
    assert_eq!(state.predicted_epoch, None);
}
