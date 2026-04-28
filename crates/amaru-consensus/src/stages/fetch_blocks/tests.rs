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

use std::time::Duration;

use pure_stage::{Instant, ScheduleIds, trace_buffer::TerminationReason};
use tracing::Level;

use super::*;
use crate::stages::{
    fetch_blocks::test_setup::{
        TestPrep, assert_trace, setup, test_prep, tm_cancel_schedule, tm_clock, tm_get_anchor_hash, tm_load_block,
        tm_load_header, tm_schedule, tm_send, tm_store_block, tm_terminate, tm_terminated,
    },
    test_utils::{tm_input, tm_state},
};

#[test]
fn test_new_tip_load_header_fails() {
    let prep = test_prep();
    // Tip h2 but store has no headers - load will fail
    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::NewTip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            tm_state("fb-1", &prep.state),
            tm_input("fb-1", &msg),
            tm_load_header("fb-1", tip.hash()),
            tm_terminate("fb-1"),
            tm_terminated("fb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["failed to load initial header"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_new_tip_no_blocks_to_fetch() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.store_block(&prep.headers.h1);
    prep.store_block(&prep.headers.h2);
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::NewTip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            tm_state("fb-1", &prep.state),
            tm_input("fb-1", &msg),
            tm_load_header("fb-1", tip.hash()),
            tm_get_anchor_hash("fb-1"),
            tm_get_anchor_hash("fb-1"),
            tm_load_header("fb-1", prep.headers.h0.hash()),
            tm_load_header("fb-1", prep.headers.h1.hash()),
            tm_load_block("fb-1", prep.headers.h2.hash()),
            tm_send("fb-1", "upstream", SelectChainMsg::FetchNextFrom(tip.point())),
            tm_state("fb-1", &prep.state_with_block_height(3)),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["no blocks to fetch"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_new_tip_blocks_to_fetch() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.set_anchor(prep.headers.h0.hash());
    // No blocks stored - so we need to fetch

    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::NewTip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let timeout_at = Instant::at_offset(Duration::from_secs(5));
    let schedule_id = ScheduleIds::default().next_at(timeout_at);
    let mut state_with_timeout = prep.state_with_request(
        vec![prep.headers.h0.point(), prep.headers.h1.point(), prep.headers.h2.point()],
        1,
        schedule_id,
    );
    state_with_timeout.block_height = BlockHeight::from(3);
    let state_after_timeout = {
        let mut state = state_with_timeout.clone();
        state.missing.clear();
        state.timeout = None;
        state
    };
    assert_trace(
        &running,
        &[
            tm_state("fb-1", &prep.state),
            tm_input("fb-1", &msg),
            tm_load_header("fb-1", tip.hash()),
            tm_get_anchor_hash("fb-1"),
            tm_get_anchor_hash("fb-1"),
            tm_load_header("fb-1", prep.headers.h0.hash()),
            tm_load_header("fb-1", prep.headers.h1.hash()),
            tm_load_block("fb-1", prep.headers.h2.hash()),
            tm_load_header("fb-1", prep.headers.h0.hash()),
            tm_load_block("fb-1", prep.headers.h1.hash()),
            tm_load_block("fb-1", prep.headers.h0.hash()),
            tm_send(
                "fb-1",
                "manager",
                ManagerMessage::FetchBlocks2 {
                    from: prep.headers.h1.point(),
                    through: prep.headers.h2.point(),
                    id: 1,
                    cr: prep.cleanup_replies.clone(),
                },
            ),
            tm_schedule("fb-1", FetchBlocksMsg::Timeout(1), schedule_id),
            tm_state("fb-1", &state_with_timeout),
            tm_clock(timeout_at),
            tm_input("fb-1", &FetchBlocksMsg::Timeout(1)),
            tm_send("fb-1", "upstream", SelectChainMsg::FetchNextFrom(prep.headers.h0.point())),
            tm_state("fb-1", &state_after_timeout),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["requesting blocks"])
        .assert_and_remove(Level::ERROR, &["timeout fetching blocks"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_received() {
    let mut prep = test_prep();
    prep.state = prep.state_with_request(
        vec![prep.headers.h0.point(), prep.headers.h1.point(), prep.headers.h2.point()],
        1,
        prep.schedule_at(Duration::from_secs(5)),
    );
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let msg = FetchBlocksMsg::Block(TestPrep::network_block(&prep.headers.h1));
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected = {
        let mut state = prep.state.clone();
        state.missing.remove(0);
        state
    };
    assert_trace(
        &running,
        &[
            tm_state("fb-1", &prep.state),
            tm_input("fb-1", &msg),
            tm_store_block("fb-1", prep.headers.h1.hash(), TestPrep::raw_block(&prep.headers.h1)),
            tm_send("fb-1", "downstream", (prep.headers.h1.tip(), prep.headers.h0.point(), BlockHeight::from(0))),
            tm_state("fb-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["received block"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_block2_received() {
    let mut prep = test_prep();
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    prep.state = prep.state_with_request(vec![prep.headers.h1.point(), prep.headers.h2.point()], 1, schedule_id);
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let msg = FetchBlocksMsg::Block(TestPrep::network_block(&prep.headers.h2));
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected = {
        let mut state = prep.state.clone();
        state.missing.clear();
        state.timeout = None;
        state
    };
    assert_trace(
        &running,
        &[
            tm_state("fb-1", &prep.state),
            tm_input("fb-1", &msg),
            tm_store_block("fb-1", prep.headers.h2.hash(), TestPrep::raw_block(&prep.headers.h2)),
            tm_send("fb-1", "downstream", (prep.headers.h2.tip(), prep.headers.h1.point(), BlockHeight::from(0))),
            tm_cancel_schedule("fb-1", schedule_id),
            tm_send("fb-1", "upstream", SelectChainMsg::FetchNextFrom(prep.headers.h2.point())),
            tm_state("fb-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["received block"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}
