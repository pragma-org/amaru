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

use amaru_kernel::{BlockHeight, IsHeader, ORIGIN_HASH, Peer, Point};
use amaru_observability::tracing::Level;
use amaru_ouroboros_traits::MissingBlocks;
use amaru_protocols::manager::ManagerMessage;
use amaru_pure_stage::{
    Instant, ScheduleIds, assert_trace_contains, simulation::running::OverrideResult, tm_add_stage,
    trace_buffer::TerminationReason, trace_match::tm_wire_stage_state,
};

use super::*;
use crate::{
    performance::FetchPeerSet,
    stages::{
        fetch_blocks::test_setup::{
            TestPrep, make_block_header, setup, setup_with_overrides, te_ancestors_between, te_cancel_schedule,
            te_find_missing_blocks, te_has_block, te_load_header, te_record_block_delivery, te_record_blocks_requested,
            te_record_fetch_failure, te_schedule, te_select_peers_for_fetch, te_store_block, test_peer, test_prep,
        },
        test_utils::{
            assert_trace_no_clock, start_in_era, te_clock_read, te_input, te_send, te_state, te_terminate,
            te_terminated, tm_state,
        },
    },
};

#[test]
fn test_new_tip_load_header_fails() {
    let prep = test_prep();
    // Point h2 but store has no headers - load will fail
    let tip = prep.headers.h2.point();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_no_clock(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_find_missing_blocks("fb-1", tip.hash(), 25),
            te_terminate("fb-1"),
            te_terminated("fb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["blocks.header_not_found"]).assert_no_remaining_at([
        Level::DEBUG,
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_new_tip_no_blocks_to_fetch() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.store_block(&prep.headers.h1);
    prep.store_block(&prep.headers.h2);
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_validity(prep.headers.h2.hash(), true);

    let tip = prep.headers.h2.point();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_no_clock(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_find_missing_blocks("fb-1", tip.hash(), 25),
            te_load_header("fb-1", tip.hash(), true),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(tip)),
            te_state("fb-1", &prep.state_with_block_height(3)),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["blocks.nothing_to_fetch"]).assert_no_remaining_at([
        Level::DEBUG,
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

/// A forged block is stored before `select_chain` announces it. The body is local, so there is
/// nothing to download, and the block still has to reach validation.
#[test]
fn test_new_tip_forwards_stored_unvalidated_block() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.store_block(&prep.headers.h1);
    prep.store_block(&prep.headers.h2);
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_validity(prep.headers.h0.hash(), true);
    prep.set_validity(prep.headers.h1.hash(), true);

    let tip = prep.headers.h2.point();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_no_clock(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_find_missing_blocks("fb-1", tip.hash(), 25),
            te_load_header("fb-1", tip.hash(), true),
            te_has_block("fb-1", tip.hash()),
            te_send("fb-1", "downstream", DownloadedBlock::new(tip, parent, BlockHeight::from(3))),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(tip)),
            te_state("fb-1", &prep.state_with_block_height(3)),
        ],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_recover_stored_blocks_at_origin_keeps_the_stage() {
    let prep = test_prep();
    let msg = FetchBlocksMsg::recover_stored_blocks(Point::Origin, ORIGIN_HASH);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let mut expected = prep.state.clone();
    expected.trace_context = Some(Default::default());
    assert_trace_no_clock(
        &running,
        &[te_state("fb-1", &prep.state), te_input("fb-1", &msg), te_state("fb-1", &expected)],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_recover_stored_blocks_origin_ledger_with_a_stored_candidate_terminates() {
    let prep = test_prep();
    let msg = FetchBlocksMsg::recover_stored_blocks(Point::Origin, prep.headers.h2.hash());

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_no_clock(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_terminate("fb-1"),
            te_terminated("fb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["blocks.recover_inconsistent", "ledger_tip_is_origin"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
    assert_trace_no_clock(
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
                DownloadedBlock::new(prep.headers.h1.point(), prep.headers.h0.point(), BlockHeight::from(3)),
            ),
            te_has_block("fb-1", prep.headers.h2.hash()),
            te_send(
                "fb-1",
                "downstream",
                DownloadedBlock::new(prep.headers.h2.point(), prep.headers.h1.point(), BlockHeight::from(3)),
            ),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h2.point())),
            te_state("fb-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["blocks.replay"])
        .assert_and_remove(Level::DEBUG, &["blocks.replay_block"])
        .assert_and_remove(Level::DEBUG, &["blocks.replay_block"])
        .assert_and_remove(Level::INFO, &["blocks.nothing_to_fetch"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
    let timers = request_timers();
    let requested_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    let mut expected = prep.state_with_request(
        MissingBlocks::new(prep.headers.h1.point(), vec![prep.headers.h2.point(), h3.point()]),
        1,
        timers.timeout,
    );
    expected.block_height = BlockHeight::from(4);
    expected.trace_context = Some(Default::default());
    expected.fetch_started_at = Some(requested_at);
    expected.awaiting_broadcast = true;
    expected.widen = Some(timers.widen[0]);
    let mut trace = vec![
        te_state("fb-1", &prep.state),
        te_input("fb-1", &msg),
        te_ancestors_between("fb-1", prep.headers.h0.point(), h3.hash()),
        te_load_header("fb-1", h3.hash(), false),
        te_has_block("fb-1", prep.headers.h1.hash()),
        te_send(
            "fb-1",
            "downstream",
            DownloadedBlock::new(prep.headers.h1.point(), prep.headers.h0.point(), BlockHeight::from(4)),
        ),
        // The tail of the replay path is the batch to fetch, so no second search of the store.
        te_has_block("fb-1", prep.headers.h2.hash()),
        te_clock_read("fb-1"),
        te_select_peers_for_fetch("fb-1", vec![prep.headers.h2.hash(), h3.hash()], 5, requested_at),
        te_send(
            "fb-1",
            "manager",
            ManagerMessage::FetchBlocks {
                from: prep.headers.h2.point(),
                through: h3.point(),
                id: 1,
                cr: prep.cleanup_replies.clone(),
                peers: None,
            },
        ),
        te_record_blocks_requested("fb-1", vec![prep.headers.h2.hash(), h3.hash()], requested_at),
        te_schedule("fb-1", FetchBlocksMsg::Timeout(1), timers.timeout),
    ];
    push_widen_arm(&mut trace, &timers);
    trace.push(te_state("fb-1", &expected));
    let mut waking = expected.clone();
    push_widen_wakeups(&mut trace, &mut waking, &timers);
    // The batch chains onto h1, so a timeout must resume from there.
    let mut done = waking;
    done.missing = None;
    done.timeout = None;
    done.trace_context = None;
    done.fetch_started_at = None;
    done.awaiting_broadcast = false;
    done.widen = None;
    done.widen_index = 0;
    trace.extend([
        te_input("fb-1", &FetchBlocksMsg::Timeout(1)),
        te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h1.point())),
        te_state("fb-1", &done),
    ]);
    assert_trace_no_clock(&running, &trace);
    logs.assert_and_remove(Level::DEBUG, &["blocks.replay"])
        .assert_and_remove(Level::DEBUG, &["blocks.replay_block"])
        .assert_and_remove(Level::DEBUG, &["blocks.request", "length=2"])
        .assert_and_remove(Level::DEBUG, &["blocks.weak_peer_selection", "weak=true"])
        .assert_and_remove(Level::WARN, &["blocks.timeout"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_new_tip_blocks_to_fetch() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.set_anchor(prep.headers.h0.hash());
    // No blocks stored - so we need to fetch

    let tip = prep.headers.h2.point();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    // run_simulation: initial clock +10s, global_epoch_offset from start_in_era; timeout is +5s.
    let timers = request_timers();
    let mut state_with_timeout = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point(), prep.headers.h2.point()]),
        1,
        timers.timeout,
    );
    state_with_timeout.block_height = BlockHeight::from(3);
    state_with_timeout.trace_context = Some(Default::default());
    let requested_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    state_with_timeout.fetch_started_at = Some(requested_at);
    state_with_timeout.awaiting_broadcast = true;
    state_with_timeout.widen = Some(timers.widen[0]);
    let mut trace = vec![
        te_state("fb-1", &prep.state),
        te_input("fb-1", &msg),
        te_find_missing_blocks("fb-1", tip.hash(), 25),
        te_clock_read("fb-1"),
        te_select_peers_for_fetch("fb-1", vec![prep.headers.h1.hash(), prep.headers.h2.hash()], 5, requested_at),
        te_send(
            "fb-1",
            "manager",
            ManagerMessage::FetchBlocks {
                from: prep.headers.h1.point(),
                through: prep.headers.h2.point(),
                id: 1,
                cr: prep.cleanup_replies.clone(),
                peers: None,
            },
        ),
        te_record_blocks_requested("fb-1", vec![prep.headers.h1.hash(), prep.headers.h2.hash()], requested_at),
        te_schedule("fb-1", FetchBlocksMsg::Timeout(1), timers.timeout),
    ];
    push_widen_arm(&mut trace, &timers);
    trace.push(te_state("fb-1", &state_with_timeout));
    let mut waking = state_with_timeout.clone();
    push_widen_wakeups(&mut trace, &mut waking, &timers);
    let state_after_timeout = {
        let mut state = waking;
        state.missing = None;
        state.timeout = None;
        state.trace_context = None;
        state.fetch_started_at = None;
        state.awaiting_broadcast = false;
        state.widen = None;
        state.widen_index = 0;
        state
    };
    trace.extend([
        te_input("fb-1", &FetchBlocksMsg::Timeout(1)),
        te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h0.point())),
        te_state("fb-1", &state_after_timeout),
    ]);
    assert_trace_no_clock(&running, &trace);
    logs.assert_and_remove(Level::DEBUG, &["blocks.fetch", "length=2"])
        .assert_and_remove(Level::DEBUG, &["blocks.fetch", "weak=true"])
        .assert_and_remove(Level::WARN, &["blocks.timeout"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_received() {
    let mut prep = test_prep();
    let requested_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    prep.state = {
        let mut state = prep.state_with_request(
            MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point(), prep.headers.h2.point()]),
            1,
            prep.schedule_at(Duration::from_secs(5)),
        );
        state.fetch_started_at = Some(requested_at);
        state
    };
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let msg = FetchBlocksMsg::Block(test_peer(), TestPrep::network_block(&prep.headers.h1));
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected = {
        let mut state = prep.state.clone();
        state.missing = Some(MissingBlocks::new(prep.headers.h1.point(), vec![prep.headers.h2.point()]));
        state.fetch_contributors.insert(test_peer());
        state
    };
    let raw = TestPrep::raw_block(&prep.headers.h1);
    let bytes = raw.len() as u64;
    assert_trace_contains(
        &running,
        &[
            te_input("fb-1", &msg).into(),
            te_clock_read("fb-1").into(),
            te_record_block_delivery(
                "fb-1",
                test_peer(),
                prep.headers.h1.hash(),
                prep.headers.h1.block_height(),
                prep.headers.h1.parent_hash(),
                requested_at,
                Duration::ZERO,
                bytes,
            )
            .into(),
            te_store_block("fb-1", prep.headers.h1.hash(), raw).into(),
            te_send(
                "fb-1",
                "downstream",
                DownloadedBlock::new(prep.headers.h1.point(), prep.headers.h0.point(), BlockHeight::from(0)),
            )
            .into(),
            te_state("fb-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["blocks.received"])
        .assert_and_remove(
            Level::DEBUG,
            &[
                "amaru::blockperf",
                "block.received",
                r#"peer="127.0.0.1:3009""#,
                "rank=1",
                &format!(r#"header_hash="{}""#, prep.headers.h1.hash()),
            ],
        )
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block2_received() {
    let mut prep = test_prep();
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    let requested_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    prep.state = {
        let mut state = prep.state_with_request(
            MissingBlocks::new(prep.headers.h1.point(), vec![prep.headers.h2.point()]),
            1,
            schedule_id,
        );
        state.fetch_started_at = Some(requested_at);
        state
    };
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let msg = FetchBlocksMsg::Block(test_peer(), TestPrep::network_block(&prep.headers.h2));
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected = {
        let mut state = prep.state.clone();
        state.missing = None;
        state.timeout = None;
        state.fetch_started_at = None;
        state
    };
    let raw = TestPrep::raw_block(&prep.headers.h2);
    let bytes = raw.len() as u64;
    assert_trace_contains(
        &running,
        &[
            te_input("fb-1", &msg).into(),
            te_clock_read("fb-1").into(),
            te_record_block_delivery(
                "fb-1",
                test_peer(),
                prep.headers.h2.hash(),
                prep.headers.h2.block_height(),
                prep.headers.h2.parent_hash(),
                requested_at,
                Duration::ZERO,
                bytes,
            )
            .into(),
            te_store_block("fb-1", prep.headers.h2.hash(), raw).into(),
            te_send(
                "fb-1",
                "downstream",
                DownloadedBlock::new(prep.headers.h2.point(), prep.headers.h1.point(), BlockHeight::from(0)),
            )
            .into(),
            te_cancel_schedule("fb-1", schedule_id).into(),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h2.point())).into(),
            te_state("fb-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["blocks.received"])
        .assert_and_remove(
            Level::DEBUG,
            &[
                "amaru::blockperf",
                "block.received",
                r#"peer="127.0.0.1:3009""#,
                "rank=1",
                &format!(r#"header_hash="{}""#, prep.headers.h2.hash()),
            ],
        )
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// Additional comprehensive stage tests using selective trace matching
// ---------------------------------------------------------------------------

#[test]
fn test_new_tip_find_missing_blocks_error() {
    let prep = test_prep();
    let tip = prep.headers.h2.point();
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

    logs.assert_and_remove(Level::ERROR, &["blocks.header_not_found"]).assert_no_remaining_at([
        Level::DEBUG,
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
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

    logs.assert_and_remove(Level::DEBUG, &["blocks.received"])
        .assert_and_remove(
            Level::DEBUG,
            &[
                "amaru::blockperf",
                "block.received",
                r#"peer="127.0.0.1:3009""#,
                "rank=1",
                &format!(r#"header_hash="{}""#, prep.headers.h1.hash()),
            ],
        )
        .assert_and_remove(Level::WARN, &["blocks.point_mismatch"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_straggler_no_outstanding_missing() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1]);
    prep.set_anchor(prep.headers.h0.hash());

    let msg = FetchBlocksMsg::Block(test_peer(), TestPrep::network_block(&prep.headers.h1));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(&running, &[te_input("fb-1", &msg).into(), te_state("fb-1", &prep.state).into()]);

    logs.assert_and_remove(Level::DEBUG, &["blocks.received"])
        .assert_and_remove(Level::DEBUG, &["blocks.straggler", r#"peer="127.0.0.1:3009""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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

    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_timeout_records_fetch_failure_for_asked_peers() {
    use std::collections::BTreeSet;

    let mut prep = test_prep();
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    let peer = test_peer();
    prep.state = {
        let mut state = prep.state_with_request(
            MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point()]),
            1,
            schedule_id,
        );
        state.fetch_peers = BTreeSet::from([peer]);
        state.fetch_started_at = Some(Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time));
        state.trace_context = Some(Default::default());
        state
    };

    let msg = FetchBlocksMsg::Timeout(1);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let failed_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    let expected = {
        let mut state = prep.state.clone();
        state.missing = None;
        state.timeout = None;
        state.trace_context = None;
        state.fetch_started_at = None;
        state.fetch_peers.clear();
        state.fetch_contributors.clear();
        state
    };
    assert_trace_contains(
        &running,
        &[
            te_input("fb-1", &msg).into(),
            te_clock_read("fb-1").into(),
            te_record_fetch_failure("fb-1", vec![peer], failed_at).into(),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h0.point())).into(),
            te_state("fb-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::WARN, &["blocks.timeout"]).assert_no_remaining_at([
        Level::DEBUG,
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_strong_selection_passes_peers_to_manager() {
    use std::collections::BTreeSet;

    use crate::performance::SelectPeersForFetchEffect;

    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.set_anchor(prep.headers.h0.hash());

    let selected = Peer::for_test(3002);
    let tip = prep.headers.h2.point();
    let parent = prep.headers.h1.point();
    let msg = FetchBlocksMsg::new_tip(tip, parent);
    let selected_clone = selected;
    let (running, _guards, mut logs) = setup_with_overrides(&prep, [msg.clone()], move |running| {
        running.override_external_effect::<SelectPeersForFetchEffect>(usize::MAX, move |_| {
            OverrideResult::handled(FetchPeerSet { peers: vec![selected_clone], weak: false })
        });
    });

    let requested_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    assert_trace_contains(
        &running,
        &[
            te_input("fb-1", &msg).into(),
            te_find_missing_blocks("fb-1", tip.hash(), 25).into(),
            te_clock_read("fb-1").into(),
            te_select_peers_for_fetch("fb-1", vec![prep.headers.h1.hash(), prep.headers.h2.hash()], 5, requested_at)
                .into(),
            te_send(
                "fb-1",
                "manager",
                ManagerMessage::FetchBlocks {
                    from: prep.headers.h1.point(),
                    through: prep.headers.h2.point(),
                    id: 1,
                    cr: prep.cleanup_replies.clone(),
                    peers: Some(vec![selected]),
                },
            )
            .into(),
            te_record_blocks_requested("fb-1", vec![prep.headers.h1.hash(), prep.headers.h2.hash()], requested_at)
                .into(),
            // fetch_peers is filled only by PeersAsked, not by selection prefill.
            te_state_match_fetch_peers(&BTreeSet::new()),
        ],
    );
    // Sim advances the 5s timeout after the request; ignore that tail.
    logs.assert_and_remove(Level::DEBUG, &["blocks.fetch", "length=2"])
        .assert_and_remove(Level::WARN, &["blocks.timeout"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

fn te_state_match_fetch_peers(
    expected: &std::collections::BTreeSet<Peer>,
) -> amaru_pure_stage::trace_match::TraceMatch<'static> {
    let expected = expected.clone();
    tm_state("fb-1", move |s: &FetchBlocks| s.fetch_peers == expected, "state with selected fetch_peers")
}

#[test]
fn test_timeout_skips_fetch_failure_for_contributors() {
    use std::collections::BTreeSet;

    let mut prep = test_prep();
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    let good = test_peer();
    let bad = Peer::for_test(3011);
    prep.state = {
        let mut state = prep.state_with_request(
            MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point()]),
            1,
            schedule_id,
        );
        state.fetch_peers = BTreeSet::from([good, bad]);
        state.fetch_contributors = BTreeSet::from([good]);
        state.fetch_started_at = Some(Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time));
        state.trace_context = Some(Default::default());
        state
    };

    let msg = FetchBlocksMsg::Timeout(1);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let failed_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    assert_trace_contains(
        &running,
        &[
            te_input("fb-1", &msg).into(),
            te_clock_read("fb-1").into(),
            te_record_fetch_failure("fb-1", vec![bad], failed_at).into(),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h0.point())).into(),
        ],
    );
    logs.assert_and_remove(Level::WARN, &["blocks.timeout"]).assert_no_remaining_at([
        Level::DEBUG,
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_no_blocks_records_fetch_failure() {
    use std::collections::BTreeSet;

    let mut prep = test_prep();
    let peer = test_peer();
    let other = Peer::for_test(3012);
    prep.state = {
        let mut state = prep.state_with_request(
            MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point()]),
            1,
            prep.schedule_at(Duration::from_secs(5)),
        );
        state.fetch_peers = BTreeSet::from([peer, other]);
        state
    };

    let msg = FetchBlocksMsg::NoBlocks(1, peer);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let failed_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    let expected = {
        let mut state = prep.state.clone();
        state.fetch_peers.remove(&peer);
        state.fetch_settled.insert(peer);
        state
    };
    assert_trace_contains(
        &running,
        &[
            te_input("fb-1", &msg).into(),
            te_clock_read("fb-1").into(),
            te_record_fetch_failure("fb-1", vec![peer], failed_at).into(),
            te_state("fb-1", &expected).into(),
        ],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_peers_asked_stores_peer_set() {
    use std::collections::BTreeSet;

    let mut prep = test_prep();
    let peer = test_peer();
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point()]),
        1,
        prep.schedule_at(Duration::from_secs(5)),
    );

    let msg = FetchBlocksMsg::PeersAsked(1, vec![peer]);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected = {
        let mut state = prep.state.clone();
        state.fetch_peers = BTreeSet::from([peer]);
        state.asked = BTreeSet::from([peer]);
        state
    };
    assert_trace_contains(&running, &[te_input("fb-1", &msg).into(), te_state("fb-1", &expected).into()]);
    logs.assert_and_remove(
        Level::DEBUG,
        &[
            "amaru::blockperf",
            "block.requested",
            &format!(r#"header_hash="{}""#, prep.headers.h1.hash()),
            r#"peers="127.0.0.1:3009""#,
        ],
    )
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

/// Regression: `NoBlocks` may arrive before `PeersAsked` (no cross-stage order). A late
/// `PeersAsked` must not put a settled peer back into the timeout set (would double-count
/// `fetch_timeouts` on batch timeout).
#[test]
fn test_later_peers_asked_keeps_the_first_wave() {
    use std::collections::BTreeSet;

    use crate::stages::fetch_blocks::test_setup::setup_preload;

    let mut prep = test_prep();
    let alice = Peer::for_test(3001);
    let bob = Peer::for_test(3002);
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point()]),
        1,
        prep.schedule_at(Duration::from_secs(5)),
    );
    prep.state.fetch_started_at = Some(Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time));

    let first = FetchBlocksMsg::PeersAsked(1, vec![alice]);
    let second = FetchBlocksMsg::PeersAsked(1, vec![bob]);
    let (running, _guards, mut logs) = setup_preload(&prep, [first, second]);
    let expected = {
        let mut state = prep.state.clone();
        state.fetch_peers = BTreeSet::from([alice, bob]);
        state.asked = BTreeSet::from([alice, bob]);
        state
    };
    assert_trace_contains(&running, &[te_state("fb-1", &expected).into()]);
    let hash = format!(r#"header_hash="{}""#, prep.headers.h1.hash());
    logs.assert_and_remove(Level::DEBUG, &["block.requested", &hash, r#"peers="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["block.requested", &hash, r#"peers="127.0.0.1:3002""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_peers_asked_does_not_resurrect_no_blocks_peer() {
    use std::collections::BTreeSet;

    use crate::stages::fetch_blocks::test_setup::setup_preload;

    let mut prep = test_prep();
    let alice = test_peer();
    let bob = Peer::for_test(3003);
    prep.state = prep.state_with_request(
        MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point()]),
        1,
        prep.schedule_at(Duration::from_secs(5)),
    );

    let no_blocks = FetchBlocksMsg::NoBlocks(1, alice);
    let peers_asked = FetchBlocksMsg::PeersAsked(1, vec![alice, bob]);
    let (running, _guards, mut logs) = setup_preload(&prep, [no_blocks.clone(), peers_asked.clone()]);
    let failed_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    let expected = {
        let mut state = prep.state.clone();
        state.fetch_settled = BTreeSet::from([alice]);
        state.fetch_peers = BTreeSet::from([bob]);
        state.asked = BTreeSet::from([bob]);
        state
    };
    assert_trace_contains(
        &running,
        &[
            te_input("fb-1", &no_blocks).into(),
            te_clock_read("fb-1").into(),
            te_record_fetch_failure("fb-1", vec![alice], failed_at).into(),
            te_input("fb-1", &peers_asked).into(),
            te_state("fb-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(
        Level::DEBUG,
        &[
            "amaru::blockperf",
            "block.requested",
            &format!(r#"header_hash="{}""#, prep.headers.h1.hash()),
            r#"peers="127.0.0.1:3003""#,
        ],
    )
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

/// Peers actually contacted are logged per header, and each distinct delivery is ranked.
#[test]
fn test_block_request_names_asked_peers_and_deliveries_are_ranked() {
    use crate::stages::fetch_blocks::test_setup::setup_preload;

    let mut prep = test_prep();
    let alice = Peer::for_test(3001);
    let bob = Peer::for_test(3002);
    let schedule_id = prep.schedule_at(Duration::from_secs(5));
    let requested_at = Instant::at_offset(Duration::from_secs(10), start_in_era().relative_time);
    prep.state = {
        let mut state = prep.state_with_request(
            MissingBlocks::new(prep.headers.h0.point(), vec![prep.headers.h1.point(), prep.headers.h2.point()]),
            1,
            schedule_id,
        );
        state.fetch_started_at = Some(requested_at);
        state
    };
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let asked = FetchBlocksMsg::PeersAsked(1, vec![bob, alice]);
    let first = FetchBlocksMsg::Block(bob, TestPrep::network_block(&prep.headers.h1));
    let second = FetchBlocksMsg::Block(alice, TestPrep::network_block(&prep.headers.h1));
    let (_running, _guards, mut logs) = setup_preload(&prep, [asked, first, second]);

    let first_hash = format!(r#"header_hash="{}""#, prep.headers.h1.hash());
    let second_hash = format!(r#"header_hash="{}""#, prep.headers.h2.hash());
    logs.assert_and_remove(Level::DEBUG, &["block.requested", &first_hash, r#"peers="127.0.0.1:3001,127.0.0.1:3002""#])
        .assert_and_remove(Level::DEBUG, &["block.requested", &second_hash, r#"peers="127.0.0.1:3001,127.0.0.1:3002""#])
        .assert_and_remove(Level::DEBUG, &["block.received", &first_hash, r#"peer="127.0.0.1:3002""#, "rank=1"])
        .assert_and_remove(Level::DEBUG, &["block.received", &first_hash, r#"peer="127.0.0.1:3001""#, "rank=2"]);
    let rest = logs.to_string();
    assert!(!rest.contains("block.received"), "only the two deliveries of this body:\n{rest}");
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

    assert_trace_no_clock(
        &running,
        &[te_state("fb-1", &prep.state), te_input("fb-1", &msg), te_state("fb-1", &state_after_pause)],
    );
    logs.assert_and_remove(Level::INFO, &["blocks.paused"]).assert_no_remaining_at([
        Level::DEBUG,
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

    assert_trace_no_clock(
        &running,
        &[
            te_state("fb-1", &prep.state),
            te_input("fb-1", &msg),
            te_send("fb-1", "upstream", SelectChainMsg::fetch_next_from(prep.headers.h0.point())),
            te_state("fb-1", &state_after_timeout),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["blocks.retry", "req_id=1"]).assert_no_remaining_at([
        Level::DEBUG,
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

    assert_trace_no_clock(
        &running,
        &[te_state("fb-1", &prep.state), te_input("fb-1", &msg), te_state("fb-1", &prep.state)],
    );

    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_first_message_wires_cleanup_replies_child() {
    let mut prep = test_prep();
    prep.state.cleanup_replies = StageRef::blackhole();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    let tip = prep.headers.h2.point();
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

struct RequestTimers {
    timeout: ScheduleId,
    widen: [ScheduleId; 3],
}

/// Schedule ids for one fetch request: the 5s timeout first, then the three widen wakeups.
fn request_timers() -> RequestTimers {
    let offset = start_in_era().relative_time;
    let start = Duration::from_secs(10);
    let ids = ScheduleIds::default();
    let timeout = ids.next_at(Instant::at_offset(start + Duration::from_secs(5), offset));
    let mut widen = [timeout; 3];
    for (index, delay) in FETCH_WIDEN_DELAYS.into_iter().enumerate() {
        widen[index] = ids.next_at(Instant::at_offset(start + delay, offset));
    }
    RequestTimers { timeout, widen }
}

fn push_widen_arm(trace: &mut Vec<amaru_pure_stage::trace_buffer::TraceEntry>, timers: &RequestTimers) {
    trace.push(te_schedule("fb-1", FetchBlocksMsg::Widen(1), timers.widen[0]));
}

fn push_widen_wakeups(
    trace: &mut Vec<amaru_pure_stage::trace_buffer::TraceEntry>,
    state: &mut FetchBlocks,
    timers: &RequestTimers,
) {
    for index in 0..timers.widen.len() {
        trace.push(te_input("fb-1", &FetchBlocksMsg::Widen(1)));
        if let Some(next) = timers.widen.get(index + 1) {
            trace.push(te_schedule("fb-1", FetchBlocksMsg::Widen(1), *next));
            state.widen = Some(*next);
            state.widen_index = (index + 1) as u8;
        } else {
            state.widen = None;
            state.widen_index = timers.widen.len() as u8;
        }
        trace.push(te_state("fb-1", state));
    }
}

fn manager_fetch_peers(running: &amaru_pure_stage::simulation::SimulationRunning) -> Vec<Option<Vec<Peer>>> {
    manager_fetches(running).into_iter().map(|(_, _, peers)| peers).collect()
}

fn manager_fetches(
    running: &amaru_pure_stage::simulation::SimulationRunning,
) -> Vec<(amaru_kernel::Point, amaru_kernel::Point, Option<Vec<Peer>>)> {
    use amaru_pure_stage::{Effect, trace_buffer::TraceEntry};
    let tb = running.trace_buffer().lock();
    tb.iter_entries()
        .filter_map(|(_, entry)| {
            let TraceEntry::Suspend(Effect::Send { msg, .. }) = entry else {
                return None;
            };
            msg.cast_ref::<ManagerMessage>().ok().and_then(|message| {
                if let ManagerMessage::FetchBlocks { from, through, peers, .. } = message {
                    Some((*from, *through, peers.clone()))
                } else {
                    None
                }
            })
        })
        .collect()
}

#[test]
fn test_widen_asks_a_peer_missing_from_the_first_selection() {
    use crate::performance::SelectPeersForFetchEffect;

    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.set_anchor(prep.headers.h0.hash());
    let alice = Peer::for_test(3001);
    let bob = Peer::for_test(3002);
    let msg = FetchBlocksMsg::new_tip(prep.headers.h2.point(), prep.headers.h1.point());
    let (running, _guards, mut logs) = setup_with_overrides(&prep, [msg], move |running| {
        running.override_external_effect::<SelectPeersForFetchEffect>(usize::MAX, move |effect| {
            let exclude = &effect.params.exclude;
            let peers = if !exclude.contains(&alice) {
                vec![alice]
            } else if !exclude.contains(&bob) {
                vec![bob]
            } else {
                Vec::new()
            };
            let weak = peers.is_empty();
            OverrideResult::handled(FetchPeerSet { peers, weak })
        });
    });

    assert_eq!(manager_fetch_peers(&running), vec![Some(vec![alice]), Some(vec![bob])]);
    logs.assert_and_remove(Level::DEBUG, &["blocks.fetch", "length=2"])
        .assert_and_remove(Level::WARN, &["blocks.timeout"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_widen_stops_once_the_batch_is_satisfied() {
    use crate::performance::SelectPeersForFetchEffect;

    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.set_anchor(prep.headers.h0.hash());
    let alice = Peer::for_test(3001);
    let bob = Peer::for_test(3002);
    let msg = FetchBlocksMsg::new_tip(prep.headers.h2.point(), prep.headers.h1.point());
    let first = FetchBlocksMsg::Block(alice, TestPrep::network_block(&prep.headers.h1));
    let second = FetchBlocksMsg::Block(alice, TestPrep::network_block(&prep.headers.h2));
    let (running, _guards, mut logs) = setup_with_overrides(&prep, [msg, first, second], move |running| {
        running.override_external_effect::<SelectPeersForFetchEffect>(usize::MAX, move |effect| {
            let exclude = &effect.params.exclude;
            let peers = if !exclude.contains(&alice) {
                vec![alice]
            } else if !exclude.contains(&bob) {
                vec![bob]
            } else {
                Vec::new()
            };
            let weak = peers.is_empty();
            OverrideResult::handled(FetchPeerSet { peers, weak })
        });
    });

    assert_eq!(manager_fetch_peers(&running), vec![Some(vec![alice])]);
    logs.assert_and_remove(Level::DEBUG, &["blocks.fetch", "length=2"])
        .assert_and_remove(Level::DEBUG, &["blocks.received"])
        .assert_and_remove(Level::DEBUG, &["amaru::blockperf", "block.received"])
        .assert_and_remove(Level::DEBUG, &["blocks.received"])
        .assert_and_remove(Level::DEBUG, &["amaru::blockperf", "block.received"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_widen_continues_when_the_first_of_two_blocks_arrives() {
    use crate::performance::SelectPeersForFetchEffect;

    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2]);
    prep.set_anchor(prep.headers.h0.hash());
    let alice = Peer::for_test(3001);
    let bob = Peer::for_test(3002);
    let msg = FetchBlocksMsg::new_tip(prep.headers.h2.point(), prep.headers.h1.point());
    // Alice returns only the first block of the two-block request.
    let partial = FetchBlocksMsg::Block(alice, TestPrep::network_block(&prep.headers.h1));
    let (running, _guards, mut logs) = setup_with_overrides(&prep, [msg, partial], move |running| {
        running.override_external_effect::<SelectPeersForFetchEffect>(usize::MAX, move |effect| {
            let exclude = &effect.params.exclude;
            let peers = if !exclude.contains(&alice) {
                vec![alice]
            } else if !exclude.contains(&bob) {
                vec![bob]
            } else {
                Vec::new()
            };
            let weak = peers.is_empty();
            OverrideResult::handled(FetchPeerSet { peers, weak })
        });
    });

    assert_eq!(
        manager_fetches(&running),
        vec![
            (prep.headers.h1.point(), prep.headers.h2.point(), Some(vec![alice])),
            (prep.headers.h2.point(), prep.headers.h2.point(), Some(vec![bob])),
        ]
    );
    logs.assert_and_remove(Level::DEBUG, &["blocks.fetch", "length=2"])
        .assert_and_remove(Level::DEBUG, &["blocks.received"])
        .assert_and_remove(Level::DEBUG, &["amaru::blockperf", "block.received"])
        .assert_and_remove(Level::WARN, &["blocks.timeout"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}
