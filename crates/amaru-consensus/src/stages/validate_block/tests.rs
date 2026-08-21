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

use amaru_kernel::{IsHeader, Point};
use amaru_pure_stage::{
    TerminationReason,
    trace_match::{assert_trace_contains, assert_trace_match},
};
use tracing::Level;

use super::*;
use crate::stages::{
    block_source::BlockSourceMsg,
    select_chain::SelectChainMsg,
    test_utils::{te_input, te_state},
    validate_block::test_setup::{
        assert_trace, setup, setup_many, te_load_header, te_send, te_switch_to_fork, te_terminate, te_terminated,
        te_validate_block, test_prep, tm_record_metrics,
    },
};

#[test]
fn test_block_with_origin_parent_terminates() {
    let prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h0.point();
    let msg = ValidateBlockMsg::new(tip, Point::Origin, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("vb-1", &prep.state),
            te_input("vb-1", &msg),
            te_terminate("vb-1"),
            te_terminated("vb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["cannot start from genesis block"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_extending_the_current_tip_is_adopted() {
    let mut prep = test_prep();
    prep.set_current(prep.headers.h1.point());
    prep.store_headers(&prep.headers.main());
    prep.store_blocks(&prep.headers.main());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.point();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_contains(
        &running,
        &[
            te_input("vb-1", &msg).into(),
            te_validate_block("vb-1", tip).into(),
            tm_record_metrics("vb-1"),
            te_send("vb-1", "select_chain", SelectChainMsg::block_validation_result(tip, true, BlockHeight::from(0)))
                .into(),
            te_send("vb-1", "block_source", BlockSourceMsg::Validation { valid: true, point: tip }).into(),
            te_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_invalid_block_condemns_in_flight_descendants() {
    let mut prep = test_prep();
    prep.set_current(prep.headers.h1.point());
    prep.block_validator.with_tip(prep.headers.h1.point()).with_validate_fails(prep.headers.h2a.point());
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());

    let tip1 = prep.headers.h2a.point();
    let msg1 = ValidateBlockMsg::new(tip1, prep.headers.h1.point(), BlockHeight::from(0));
    let tip2 = prep.headers.h3a.point();
    let msg2 = ValidateBlockMsg::new(tip2, prep.headers.h2a.point(), BlockHeight::from(0));

    let after_first =
        ValidateBlock { invalid_blocks: BTreeMap::from([(tip1.hash(), tip1.block_height())]), ..prep.state.clone() };
    let after_second = ValidateBlock {
        invalid_blocks: BTreeMap::from([(tip1.hash(), tip1.block_height()), (tip2.hash(), tip2.block_height())]),
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup_many(&prep, vec![msg1.clone(), msg2.clone()]);
    assert_trace(
        &running,
        &[
            te_state("vb-1", &prep.state),
            te_input("vb-1", &msg1),
            te_validate_block("vb-1", tip1),
            te_send("vb-1", "select_chain", SelectChainMsg::block_validation_result(tip1, false, BlockHeight::from(0))),
            te_send("vb-1", "block_source", BlockSourceMsg::Validation { valid: false, point: tip1 }),
            te_state("vb-1", &after_first),
            te_input("vb-1", &msg2),
            // no ledger effect here: the parent is invalid
            te_send("vb-1", "select_chain", SelectChainMsg::block_validation_result(tip2, false, BlockHeight::from(0))),
            te_send("vb-1", "block_source", BlockSourceMsg::Validation { valid: false, point: tip2 }),
            te_state("vb-1", &after_second),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::WARN, &["failed to advance the ledger to a new tip"])
        .assert_and_remove(Level::WARN, &["refusing to validate the descendant of an invalid block"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_invalid_blocks_below_the_security_window_are_evicted() {
    let mut prep = test_prep();
    prep.set_current(prep.headers.h1.point());
    // With k = 2, an invalid block at height 1 falls out of the window once height 3 validates.
    prep.state.consensus_security_param = 2;
    let stale = (HeaderHash::from([9u8; 32]), BlockHeight::from(1));
    let kept = (prep.headers.h2a.hash(), prep.headers.h2a.block_height());
    prep.state.invalid_blocks = BTreeMap::from([stale, kept]);
    prep.store_headers(&prep.headers.main());
    prep.store_blocks(&prep.headers.main());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.point();
    let msg = ValidateBlockMsg::new(tip, prep.headers.h1.point(), BlockHeight::from(0));

    let expected = ValidateBlock {
        current: tip,
        invalid_blocks: BTreeMap::from([kept]),
        consensus_security_param: 2,
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_contains(
        &running,
        &[te_input("vb-1", &msg).into(), te_validate_block("vb-1", tip).into(), te_state("vb-1", &expected).into()],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_ledger_failure_during_validation_terminates() {
    // The mock validator returns an outer error for the tip, so or_terminate terminates the stage.
    let mut prep = test_prep();
    prep.set_current(prep.headers.h1.point());
    prep.block_validator.with_tip(prep.headers.h1.point()).with_ledger_fails(prep.headers.h2.point());
    prep.store_headers(&prep.headers.main());
    prep.store_blocks(&prep.headers.main());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.point();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("vb-1", &prep.state),
            te_input("vb-1", &msg),
            te_validate_block("vb-1", tip),
            te_terminate("vb-1"),
            te_terminated("vb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::WARN, &["failed to validate the new block"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_completed_fork_switch_adopts_the_new_tip() {
    // The ledger is on the fork h2a (height 3) and the main chain tip h3 (height 4) wins chain
    // selection: the switch replays h2 and h3 from the store, completes, and h3 is adopted.
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2a.point());
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    prep.roll_forward_chain(prep.headers.h0.point());
    prep.roll_forward_chain(prep.headers.h1.point());
    prep.set_validity(prep.headers.h1.hash(), true);

    let tip = prep.headers.h3.point();
    let parent = prep.headers.h2.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let expected = ValidateBlock { current: tip, ..prep.state.clone() };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_match(
        &running,
        &[
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            // deciding on the switch requires comparing the message tip's header with the current one
            te_load_header("vb-1", tip.hash()).into(),
            te_load_header("vb-1", prep.headers.h2a.hash()).into(),
            te_switch_to_fork("vb-1", &tip).into(),
            tm_record_metrics("vb-1"),
            te_send("vb-1", "select_chain", SelectChainMsg::block_validation_result(tip, true, BlockHeight::from(0)))
                .into(),
            te_send("vb-1", "block_source", BlockSourceMsg::Validation { valid: true, point: tip }).into(),
            te_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))).into(),
            te_state("vb-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["switching the ledger to a new fork"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_equal_height_fork_winning_the_tiebreak_is_switched_to() {
    // A fork replacing the current chain at equal length is only adopted when its tip wins the
    // chain-selection tiebreak. Here the ledger is on h2a (height 3) and h2 (height 3) wins:
    // h2 and h2a share block number 3, so their VRF outputs tie and h2's higher op-cert
    // sequence (1 vs 0) decides.
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2a.point());
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.point();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let expected = ValidateBlock { current: tip, ..prep.state.clone() };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_match(
        &running,
        &[
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            // deciding on the switch requires comparing the message tip's header with the current one
            te_load_header("vb-1", tip.hash()).into(),
            te_load_header("vb-1", prep.headers.h2a.hash()).into(),
            te_switch_to_fork("vb-1", &tip).into(),
            tm_record_metrics("vb-1"),
            te_send("vb-1", "select_chain", SelectChainMsg::block_validation_result(tip, true, BlockHeight::from(0)))
                .into(),
            te_send("vb-1", "block_source", BlockSourceMsg::Validation { valid: true, point: tip }).into(),
            te_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))).into(),
            te_state("vb-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["switching the ledger to a new fork"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_partial_fork_switch_adopts_the_applied_prefix() {
    // The ledger is on h2 (height 3) and switches to the fork h2a -> h3a (height 4): the switch
    // stops at h2a because h3a fails to apply, and the applied prefix is adopted.
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2.point());
    prep.block_validator.with_partial_switch(
        prep.headers.h3a.point(),
        prep.headers.h2a.point(),
        prep.headers.h3a.point(),
    );
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h3a.point();
    let applied_tip = prep.headers.h2a.point();
    let parent = prep.headers.h2a.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let expected = ValidateBlock {
        current: applied_tip,
        invalid_blocks: BTreeMap::from([(tip.hash(), tip.block_height())]),
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_contains(
        &running,
        &[
            te_input("vb-1", &msg).into(),
            te_switch_to_fork("vb-1", &tip).into(),
            tm_record_metrics("vb-1"),
            te_send(
                "vb-1",
                "select_chain",
                SelectChainMsg::block_validation_result(applied_tip, true, BlockHeight::from(0)),
            )
            .into(),
            te_send("vb-1", "block_source", BlockSourceMsg::Validation { valid: true, point: applied_tip }).into(),
            te_send("vb-1", "manager", AdoptChainMsg::new(applied_tip, BlockHeight::from(0))).into(),
            te_send("vb-1", "select_chain", SelectChainMsg::block_validation_result(tip, false, BlockHeight::from(0)))
                .into(),
            te_send("vb-1", "block_source", BlockSourceMsg::Validation { valid: false, point: tip }).into(),
            te_state("vb-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["switching the ledger to a new fork"])
        .assert_and_remove(Level::WARN, &["fork switch partially applied"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_rolled_back_fork_switch_reports_the_failing_block() {
    // The ledger is on h2 (height 3) and switches to the fork h2a -> h3a (height 4): the switch
    // fails at its first block h2a, so the ledger restores its pre-switch state.
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2.point());
    prep.block_validator.with_rolled_back_switch(prep.headers.h3a.point(), prep.headers.h2a.point());
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h3a.point();
    let failed = prep.headers.h2a.point();
    let parent = prep.headers.h2a.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let expected = ValidateBlock {
        invalid_blocks: BTreeMap::from([(failed.hash(), failed.block_height()), (tip.hash(), tip.block_height())]),
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("vb-1", &prep.state),
            te_input("vb-1", &msg),
            te_load_header("vb-1", tip.hash()),
            te_load_header("vb-1", prep.headers.h2.hash()),
            te_switch_to_fork("vb-1", &tip),
            te_send(
                "vb-1",
                "select_chain",
                SelectChainMsg::block_validation_result(failed, false, BlockHeight::from(0)),
            ),
            te_send("vb-1", "block_source", BlockSourceMsg::Validation { valid: false, point: failed }),
            te_state("vb-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["switching the ledger to a new fork"])
        .assert_and_remove(Level::WARN, &["failed to fork the ledger to a new tip"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_switch_to_a_fork_only_for_a_better_candidate() {
    // We set-up the validate_block stage with a current tip of h3 (height 4).
    // The stage receives a message from the fetch_blocks stage for block h2a. Since h2a is
    // not a best candidate (it is not the longest chain), the stage must skip that message.
    let mut prep = test_prep();
    prep.set_current(prep.headers.h3.point());
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    for h in &[&prep.headers.h0, &prep.headers.h1, &prep.headers.h2, &prep.headers.h3] {
        prep.roll_forward_chain(h.point());
        prep.set_validity(h.hash(), true);
    }

    let tip = prep.headers.h2a.point();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("vb-1", &prep.state),
            te_input("vb-1", &msg),
            te_load_header("vb-1", tip.hash()),
            te_load_header("vb-1", prep.headers.h3.hash()),
            // the message is dropped: no ledger effect, no downstream signal, no state change
            te_state("vb-1", &prep.state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::DEBUG, &["block.skip"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_ledger_failure_during_fork_switch_terminates() {
    // The ledger is on the fork h2a (height 3).
    // Switching back to the main chain tip h3 (height 4) fails at the rollback, which terminates the stage.
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2a.point());
    prep.block_validator.with_rollback_fails(true);
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h3.point();
    let parent = prep.headers.h2.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("vb-1", &prep.state),
            te_input("vb-1", &msg),
            te_load_header("vb-1", tip.hash()),
            te_load_header("vb-1", prep.headers.h2a.hash()),
            te_switch_to_fork("vb-1", &tip),
            te_terminate("vb-1"),
            te_terminated("vb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["switching the ledger to a new fork"])
        .assert_and_remove(Level::WARN, &["failed to switch to a new fork"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}
