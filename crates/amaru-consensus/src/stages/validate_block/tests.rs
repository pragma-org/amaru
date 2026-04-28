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

use amaru_kernel::Point;
use pure_stage::TerminationReason;
use tracing::Level;

use super::*;
use crate::stages::{
    select_chain::SelectChainMsg,
    test_utils::{assert_trace, tm_input, tm_send, tm_state, tm_terminate, tm_terminated},
    validate_block::test_setup::{
        setup, test_prep, tm_get_anchor_hash, tm_ledger_contains, tm_ledger_tip, tm_load_header,
        tm_load_header_with_validity, tm_record_metrics, tm_rollback_ledger, tm_validate_block,
    },
};

#[test]
fn test_genesis_block_skips_validation() {
    let prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.store_block(&prep.headers.h0);
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h0.tip();
    let msg = ValidateBlockMsg::new(tip, Point::Origin, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_terminate("vb-1"),
            tm_terminated("vb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["cannot start from genesis block"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_parent_equals_current_validates_tip_only() {
    let mut prep = test_prep();
    prep.set_current(prep.headers.h1.point());
    prep.store_headers(&prep.headers.main());
    prep.store_blocks(&prep.headers.main());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected_state = ValidateBlock::new(prep.manager.clone(), prep.select_chain.clone(), tip.point());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            tm_record_metrics("vb-1"),
            tm_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, true)),
            tm_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))),
            tm_state("vb-1", &expected_state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_parent_in_ledger_skips_roll_forward() {
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2a.point());
    prep.block_validator
        .with_tip(prep.headers.h0.point())
        .with_contains(prep.headers.h1.point())
        .with_contains(prep.headers.h2a.point());
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected_state = ValidateBlock::new(prep.manager.clone(), prep.select_chain.clone(), tip.point());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_ledger_contains("vb-1", &parent),
            tm_rollback_ledger("vb-1", &parent),
            tm_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            tm_record_metrics("vb-1"),
            tm_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, true)),
            tm_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))),
            tm_state("vb-1", &expected_state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["rolling back ledger to common ancestor point"])
        .assert_and_remove(Level::INFO, &["rolling forward ledger to reach parent"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_mock_rollback_fails_terminates() {
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2a.point());
    prep.block_validator
        .with_tip(prep.headers.h0.point())
        .with_contains(prep.headers.h1.point())
        .with_contains(prep.headers.h2a.point())
        .with_rollback_fails(true);
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_ledger_contains("vb-1", &parent),
            tm_rollback_ledger("vb-1", &parent),
            tm_terminate("vb-1"),
            tm_terminated("vb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["rolling back ledger to common ancestor point"])
        .assert_and_remove(Level::ERROR, &["ledger inconsistency: contains_point was true but rollback failed"])
        .assert_and_remove(Level::INFO, &["terminated stage"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_grand_parent_in_ledger() {
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2a.point());
    prep.block_validator
        .with_tip(prep.headers.h0.point())
        .with_contains(prep.headers.h1.point())
        .with_contains(prep.headers.h2a.point());
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h3.tip();
    let parent = prep.headers.h2.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    let expected_state = ValidateBlock::new(prep.manager.clone(), prep.select_chain.clone(), tip.point());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_ledger_contains("vb-1", &parent),
            tm_ledger_tip("vb-1"),
            tm_get_anchor_hash("vb-1"),
            tm_load_header("vb-1", prep.headers.h0.hash()),
            tm_load_header_with_validity("vb-1", prep.headers.h2.hash()),
            tm_load_header_with_validity("vb-1", prep.headers.h1.hash()),
            tm_ledger_contains("vb-1", &prep.headers.h2.point()),
            tm_load_header_with_validity("vb-1", prep.headers.h0.hash()),
            tm_ledger_contains("vb-1", &prep.headers.h1.point()),
            tm_rollback_ledger("vb-1", &prep.headers.h1.point()),
            tm_validate_block("vb-1", &Peer::new("unknown"), parent),
            tm_record_metrics("vb-1"),
            tm_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            tm_record_metrics("vb-1"),
            tm_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, true)),
            tm_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))),
            tm_state("vb-1", &expected_state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["rolling back ledger to common ancestor point"])
        .assert_and_remove(Level::INFO, &["rolling forward ledger to reach parent"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_rollback_fails_when_ancestor_invalid() {
    // this tests the case where a chain is built on an invalid block and the rest of the chain comes in from
    // fetch_blocks while the invalidity travels to the select_chain stage
    let mut prep = test_prep();
    prep.set_current(prep.headers.h1.point());
    prep.store_headers(&prep.headers.main());
    prep.store_blocks(&prep.headers.main());
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_validity(prep.headers.h2.hash(), false);

    let tip = prep.headers.h3.tip();
    let parent = prep.headers.h2.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_ledger_contains("vb-1", &parent),
            tm_ledger_tip("vb-1"),
            tm_get_anchor_hash("vb-1"),
            tm_load_header("vb-1", prep.headers.h0.hash()),
            tm_load_header_with_validity("vb-1", prep.headers.h2.hash()),
            tm_load_header_with_validity("vb-1", prep.headers.h1.hash()),
            tm_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, false)),
            tm_state("vb-1", &prep.state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["rolling back ledger to common ancestor point"])
        .assert_and_remove(Level::WARN, &["failed to rollback ledger"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_rollback_fails_when_rollback_point_not_in_volatile_db() {
    // this tests the case where a rollback would travel into the immutable db
    let mut prep = test_prep();
    prep.set_current(prep.headers.h2.point());
    prep.store_headers(&prep.headers.all());
    prep.store_blocks(&prep.headers.all());
    // normally, the anchor would be set to the tip of the chain, but that doesn’t happen
    // at the same place and time, so we pessimistically place the anchor further in the past
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h3a.tip();
    let parent = prep.headers.h2a.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::new(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_ledger_contains("vb-1", &parent),
            tm_ledger_tip("vb-1"),
            tm_get_anchor_hash("vb-1"),
            tm_load_header("vb-1", prep.headers.h0.hash()),
            tm_load_header_with_validity("vb-1", prep.headers.h2a.hash()),
            tm_load_header_with_validity("vb-1", prep.headers.h1.hash()),
            tm_ledger_contains("vb-1", &prep.headers.h2a.point()),
            tm_load_header_with_validity("vb-1", prep.headers.h0.hash()),
            tm_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, false)),
            tm_state("vb-1", &prep.state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::INFO, &["rolling back ledger to common ancestor point"])
        .assert_and_remove(Level::WARN, &["cannot rollback into the immutable db"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_ledger_fails_terminates_after_sending_false() {
    // Mock validator returns Err for tip -> or_terminate runs, stage terminates after sending
    // BlockValidationResult(tip, false)
    let mut prep = test_prep();
    prep.set_current(prep.headers.h1.point());
    prep.block_validator.with_tip(prep.headers.h1.point()).with_ledger_fails(prep.headers.h2.point());
    prep.store_headers(&prep.headers.main());
    prep.store_blocks(&prep.headers.main());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            tm_terminate("vb-1"),
            tm_terminated("vb-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::ERROR, &["failed to validate block"])
        .assert_and_remove(Level::INFO, &["terminated stage"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_validation_fails_terminates_after_sending_false() {
    // Mock validator returns Err for tip -> or_terminate runs, stage terminates after sending
    // BlockValidationResult(tip, false)
    let mut prep = test_prep();
    prep.set_current(prep.headers.h1.point());
    prep.block_validator.with_tip(prep.headers.h1.point()).with_validate_fails(prep.headers.h2.point());
    prep.store_headers(&prep.headers.main());
    prep.store_blocks(&prep.headers.main());
    prep.set_anchor(prep.headers.h0.hash());

    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = ValidateBlockMsg::new(tip, parent, BlockHeight::from(0));

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            tm_state("vb-1", &prep.state),
            tm_input("vb-1", &msg),
            tm_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            tm_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, false)),
            tm_state("vb-1", &prep.state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::WARN, &["invalid block"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}
