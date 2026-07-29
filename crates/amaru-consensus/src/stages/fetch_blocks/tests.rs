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

use amaru_kernel::BlockHeight;
use amaru_ouroboros_traits::MissingBlocks;
use amaru_protocols::manager::ManagerMessage;
use amaru_pure_stage::{
    Instant, ScheduleIds, assert_trace_contains, tm_add_stage, trace_buffer::TerminationReason,
    trace_match::tm_wire_stage_state,
};
use tracing::Level;

use super::*;
use crate::stages::{
    fetch_blocks::test_setup::{
        TestPrep, make_block_header, setup, te_ancestors_between, te_cancel_schedule, te_clock, te_find_missing_blocks,
        te_has_block, te_load_header, te_schedule, te_store_block, test_peer, test_prep,
    },
    test_utils::{
        assert_trace, start_in_era, te_clock_read, te_input, te_send, te_state, te_terminate, te_terminated, tm_state,
    },
};

#[test]
fn test_new_tip_load_header_fails() {
    let prep = test_prep();
    // Tip h2 but store has no headers - load will fail
    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_find_missing_blocks("fb-1", tip.hash(), 25),
            te_terminate("fb-1"),
            te_terminated("fb-1", TerminationReason::Voluntary),
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
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_find_missing_blocks("fb-1", tip.hash(), 25),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(tip.point())),
            te_state("fb-1", &prep.state_with_block_height(3)),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["no blocks to fetch"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_recover_stored_blocks_validates_downloaded_unvalidated_blocks() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h1);
    prep.store_block(&prep.headers.h2);
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_validity(prep.headers.h0.hash(), true);

    let msg = FetchBlocksMsg::recover_stored_blocks(prep.headers.h0.point(), prep.headers.h2.hash());

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected = prep.state_with_block_height(3);
    assert_trace(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_ancestors_between("fb-1", prep.headers.h0.point(), prep.headers.h2.hash()),
            te_load_header("fb-1", prep.headers.h2.hash(), false),
            te_has_block("fb-1", prep.headers.h1.hash()),
            te_send(
                "fb-1",
                "downstream",
                DownloadedBlock::new(prep.headers.h1.tip(), prep.headers.h0.point(), BlockHeight::from(3)),
            ),
            te_has_block("fb-1", prep.headers.h2.hash()),
            te_send(
                "fb-1",
                "downstream",
                DownloadedBlock::new(prep.headers.h2.tip(), prep.headers.h1.point(), BlockHeight::from(3)),
            ),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h2.point())),
            te_state("fb-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["recovering stored blocks"])
        .assert_and_remove(Level::DEBUG, &["validating stored block"])
        .assert_and_remove(Level::DEBUG, &["validating stored block"])
        .assert_and_remove(Level::INFO, &["no blocks to fetch"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

/// On a restart, headers usually run ahead of downloaded blocks, so the replay covers a prefix and
/// the rest has to be fetched. The request must cover the whole gap up to the best candidate, not
/// just the first block missing from it.
#[test]
fn test_recover_stored_blocks_fetches_the_whole_gap_after_the_replayed_prefix() {
    let prep = test_prep();
    let h3 = make_block_header(4, 4, Some(prep.headers.h2.hash()));
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2, &h3]);
    // Only h1 was downloaded before the restart; h2 and h3 are headers we know but have no block for.
    prep.store_block(&prep.headers.h1);
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_validity(prep.headers.h0.hash(), true);

    let msg = FetchBlocksMsg::recover_stored_blocks(prep.headers.h0.point(), h3.hash());

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let timeout_at = Instant::at_offset(Duration::from_secs(10 + 5), start_in_era().relative_time);
    let schedule_id = ScheduleIds::default().next_at(timeout_at);
    let requested_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    let expected = {
        let mut state = prep.state_with_request(
            MissingBlocks::new(prep.headers.h1.point(), vec![prep.headers.h2.point(), h3.point()]),
            1,
            schedule_id,
        );
        state.block_height = BlockHeight::from(4);
        state.trace_context = Some(Default::default());
        state
    };
    assert_trace(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_ancestors_between("fb-1", prep.headers.h0.point(), h3.hash()),
            te_load_header("fb-1", h3.hash(), false),
            te_has_block("fb-1", prep.headers.h1.hash()),
            te_send(
                "fb-1",
                "downstream",
                DownloadedBlock::new(prep.headers.h1.tip(), prep.headers.h0.point(), BlockHeight::from(4)),
            ),
            // The tail of the replay path is the batch to fetch, so no second search of the store.
            te_has_block("fb-1", prep.headers.h2.hash()),
            te_clock_read("fb-1"),
            te_send(
                "fb-1",
                "manager",
                ManagerMessage::FetchBlocks {
                    from: prep.headers.h2.point(),
                    through: h3.point(),
                    id: 1,
                    cr: prep.cleanup_replies.clone(),
                },
            ),
            te_send(
                "fb-1",
                "upstream",
                SelectChainMsg::BlocksRequested(vec![prep.headers.h2.hash(), h3.hash()], requested_at),
            ),
            te_schedule("fb-1", FetchBlocksMsg::Timeout(1), schedule_id),
            te_state("fb-1", &expected),
            // The batch chains onto h1, so a timeout must resume from there.
            te_clock(timeout_at),
            te_input("fb-1", &FetchBlocksMsg::Timeout(1)),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h1.point())),
            te_state("fb-1", &{
                let mut state = expected.clone();
                state.missing = None;
                state.timeout = None;
                state.trace_context = None;
                state
            }),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["recovering stored blocks"])
        .assert_and_remove(Level::DEBUG, &["validating stored block"])
        .assert_and_remove(Level::DEBUG, &["requesting blocks"])
        .assert_and_remove(Level::ERROR, &["timeout fetching blocks"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_new_tip_blocks_to_fetch() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.set_anchor(prep.headers.h0.hash());
    // No blocks stored - so we need to fetch

    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    // run_simulation: initial clock +10s, global_epoch_offset from start_in_era; timeout is +5s.
    let timeout_at = Instant::at_offset(Duration::from_secs(10 + 5), start_in_era().relative_time);
    let schedule_id = ScheduleIds::default().next_at(timeout_at);
    let mut state_with_timeout = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point(), prep.headers.h2.point()]),
        1,
        schedule_id,
    );
    state_with_timeout.block_height = BlockHeight::from(3);
    state_with_timeout.trace_context = Some(Default::default());
    let requested_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    let state_after_timeout = {
        let mut state = state_with_timeout.clone();
        state.missing = None;
        state.timeout = None;
        state.trace_context = None;
        state
    };
    assert_trace(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_find_missing_blocks("fb-1", tip.hash(), 25),
            te_clock_read("fb-1"),
            te_send(
                "fb-1",
                "manager",
                ManagerMessage::FetchBlocks {
                    from: prep.headers.h1.point(),
                    through: prep.headers.h2.point(),
                    id: 1,
                    cr: prep.cleanup_replies.clone(),
                },
            ),
            te_send(
                "fb-1",
                "upstream",
                SelectChainMsg::BlocksRequested(vec![prep.headers.h1.hash(), prep.headers.h2.hash()], requested_at),
            ),
            te_schedule("fb-1", FetchBlocksMsg::Timeout(1), schedule_id),
            te_state("fb-1", &state_with_timeout),
            te_clock(timeout_at),
            te_input("fb-1", &FetchBlocksMsg::Timeout(1)),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h0.point())),
            te_state("fb-1", &state_after_timeout),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["requesting blocks"])
        .assert_and_remove(Level::WARN, &["timeout fetching blocks"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_received() {
    let mut prep = test_prep();
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point(), prep.headers.h2.point()]),
        1,
        prep.schedule_at(Duration::from_secs(5)),
    );
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let msg = FetchBlocksMsg::Block(test_peer(), TestPrep::network_block(&prep.headers.h1));
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected = {
        let mut state = prep.state.clone();
        state.missing = Some(MissingBlocks::new(prep.headers.h1.point(), vec![prep.headers.h2.point()]));
        state
    };
    assert_trace(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_clock_read("fb-1"),
            te_send(
                "fb-1",
                "upstream",
                SelectChainMsg::BlockDownloaded(
                    prep.headers.h1.hash(),
                    Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time),
                ),
            ),
            te_store_block("fb-1", prep.headers.h1.hash(), TestPrep::raw_block(&prep.headers.h1)),
            te_send(
                "fb-1",
                "downstream",
                DownloadedBlock::new(prep.headers.h1.tip(), prep.headers.h0.point(), BlockHeight::from(0)),
            ),
            te_state("fb-1", &expected),
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
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h1.point(), vec![prep.headers.h2.point()]),
        1,
        schedule_id,
    );
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let msg = FetchBlocksMsg::Block(test_peer(), TestPrep::network_block(&prep.headers.h2));
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected = {
        let mut state = prep.state.clone();
        state.missing = None;
        state.timeout = None;
        state
    };
    assert_trace(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_clock_read("fb-1"),
            te_send(
                "fb-1",
                "upstream",
                SelectChainMsg::BlockDownloaded(
                    prep.headers.h2.hash(),
                    Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time),
                ),
            ),
            te_store_block("fb-1", prep.headers.h2.hash(), TestPrep::raw_block(&prep.headers.h2)),
            te_send(
                "fb-1",
                "downstream",
                DownloadedBlock::new(prep.headers.h2.tip(), prep.headers.h1.point(), BlockHeight::from(0)),
            ),
            te_cancel_schedule("fb-1", schedule_id),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h2.point())),
            te_state("fb-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["received block"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

// ---------------------------------------------------------------------------
// Additional comprehensive stage tests using selective trace matching
// ---------------------------------------------------------------------------

#[test]
fn test_new_tip_find_missing_blocks_error() {
    let prep = test_prep();
    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    // We trigger the error path from find_missing_blocks (in this harness it surfaces
    // similarly to the StartHeaderNotFound case and leads to termination).
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(
        &running,
        &[
            te_input("fb-1", &msg).into(),
            te_find_missing_blocks("fb-1", tip.hash(), 25).into(),
            te_terminate("fb-1").into(),
            te_terminated("fb-1", TerminationReason::Voluntary).into(),
        ],
    );

    logs.assert_and_remove(Level::ERROR, &["failed to load initial header"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_point_mismatch() {
    let mut prep = test_prep();
    // Expect h2 directly after h0 (so first() == h2). Sending h1 will pass the parent check
    // (parent of h1 is h0) but fail the point check (h1 != h2).
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h2.point()]),
        1,
        prep.schedule_at(Duration::from_secs(5)),
    );
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1]);
    prep.set_anchor(prep.headers.h0.hash());

    // Send h1 — correct parent for current boundary, but not the expected next point.
    let msg = FetchBlocksMsg::Block(test_peer(), TestPrep::network_block(&prep.headers.h1));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(&running, &[te_input("fb-1", &msg).into(), te_state("fb-1", &prep.state).into()]);

    logs.assert_and_remove(Level::WARN, &["block point mismatch"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_block_straggler_no_outstanding_missing() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1]);
    prep.set_anchor(prep.headers.h0.hash());

    let msg = FetchBlocksMsg::Block(test_peer(), TestPrep::network_block(&prep.headers.h1));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(&running, &[te_input("fb-1", &msg).into(), te_state("fb-1", &prep.state).into()]);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_timeout_stale_is_ignored() {
    let mut prep = test_prep();
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point()]),
        5, // current req_id is 5
        schedule_id,
    );

    // Send a stale timeout for a different req_id
    let msg = FetchBlocksMsg::Timeout(3);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(&running, &[te_input("fb-1", &msg).into(), te_state("fb-1", &prep.state).into()]);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_no_peers_available_pauses_without_error() {
    let mut prep = test_prep();
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point(), prep.headers.h2.point()]),
        1,
        schedule_id,
    );
    prep.state.trace_context = Some(Default::default());

    let msg = FetchBlocksMsg::NoPeersAvailable(1);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    let state_after_pause = {
        let mut state = prep.state.clone();
        state.no_peers_pause = true;
        state
    };

    assert_trace(
        &running,
        &[te_state("fb-1", &prep.state), te_input("fb-1", &msg), te_state("fb-1", &state_after_pause)],
    );
    logs.assert_and_remove(Level::INFO, &["block fetching paused due to no upstream peers"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_timeout_after_no_peers_pause_retries_without_error() {
    let mut prep = test_prep();
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point(), prep.headers.h2.point()]),
        1,
        schedule_id,
    );
    prep.state.no_peers_pause = true;
    prep.state.trace_context = Some(Default::default());

    let msg = FetchBlocksMsg::Timeout(1);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    let state_after_timeout = {
        let mut state = prep.state.clone();
        state.missing = None;
        state.timeout = None;
        state.no_peers_pause = false;
        state.trace_context = None;
        state
    };

    assert_trace(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h0.point())),
            te_state("fb-1", &state_after_timeout),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["retrying block fetch after no-peers pause"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_no_peers_available_stale_is_ignored() {
    let mut prep = test_prep();
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point()]),
        5,
        schedule_id,
    );

    let msg = FetchBlocksMsg::NoPeersAvailable(3);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace(&running, &[te_state("fb-1", &prep.state), te_input("fb-1", &msg), te_state("fb-1", &prep.state)]);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_first_message_wires_cleanup_replies_child() {
    let mut prep = test_prep();
    prep.state.cleanup_replies = StageRef::blackhole();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    let (running, _guards, _logs) = setup(&prep, msg.clone());

    // On the first message the stage wires up the cleanup_replies child.
    // We assert its creation using selective matching on the generated name.
    assert_trace_contains(
        &running,
        &[
            tm_add_stage("fb-1", "cleanup_replies"),
            tm_wire_stage_state(
                "fb-1",
                "cleanup_replies",
                Cleanup::new(
                    StageRef::named_for_tests("fb-1"),
                    StageRef::named_for_tests("block_source"),
                    StageRef::named_for_tests("peer_selection"),
                ),
            ),
            tm_state(
                "fb-1",
                |s: &FetchBlocks| s.cleanup_replies.name().as_str().contains("cleanup_replies"),
                "state with cleanup_replies child",
            ),
        ],
    );
}
