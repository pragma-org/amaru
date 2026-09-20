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

use amaru_kernel::{Point, Slot};
use amaru_observability::tracing::Level;

use super::{
    ForgeBlockMsg,
    protocol::{AdoptedTip, LeadSlot},
    test_setup::{setup, test_prep},
};

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
    let msg = ForgeBlockMsg::from(LeadSlot { slot: Slot::from(11) });
    let (running, _guards, mut logs, stage) = setup(&prep, msg);

    logs.assert_and_remove(Level::WARN, &["ocert_not_yet_valid"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);

    let state = running.get_state(&stage).cloned().unwrap();
    assert!(state.next_lead.is_none());
}
