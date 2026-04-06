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
    test_utils::{te_input, te_state},
    validate_block::test_setup::{
        assert_trace, setup, te_get_anchor_hash, te_ledger_contains, te_ledger_tip, te_load_header,
        te_load_header_with_validity, te_record_metrics, te_rollback_ledger, te_send, te_terminate, te_terminated,
        te_validate_block, test_prep,
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_terminate("vb-1"),
            te_terminated("vb-1", TerminationReason::Voluntary),
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            te_record_metrics("vb-1"),
            te_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, true)),
            te_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))),
            te_state("vb-1", &expected_state).into(),
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_ledger_contains("vb-1", &parent),
            te_rollback_ledger("vb-1", &parent),
            te_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            te_record_metrics("vb-1"),
            te_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, true)),
            te_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))),
            te_state("vb-1", &expected_state).into(),
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_ledger_contains("vb-1", &parent),
            te_rollback_ledger("vb-1", &parent),
            te_terminate("vb-1"),
            te_terminated("vb-1", TerminationReason::Voluntary),
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_ledger_contains("vb-1", &parent),
            te_ledger_tip("vb-1"),
            te_get_anchor_hash("vb-1"),
            te_load_header("vb-1", prep.headers.h0.hash()),
            te_load_header_with_validity("vb-1", prep.headers.h2.hash()),
            te_load_header_with_validity("vb-1", prep.headers.h1.hash()),
            te_ledger_contains("vb-1", &prep.headers.h2.point()),
            te_load_header_with_validity("vb-1", prep.headers.h0.hash()),
            te_ledger_contains("vb-1", &prep.headers.h1.point()),
            te_rollback_ledger("vb-1", &prep.headers.h1.point()),
            te_validate_block("vb-1", &Peer::new("unknown"), parent),
            te_record_metrics("vb-1"),
            te_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            te_record_metrics("vb-1"),
            te_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, true)),
            te_send("vb-1", "manager", AdoptChainMsg::new(tip, BlockHeight::from(0))),
            te_state("vb-1", &expected_state).into(),
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_ledger_contains("vb-1", &parent),
            te_ledger_tip("vb-1"),
            te_get_anchor_hash("vb-1"),
            te_load_header("vb-1", prep.headers.h0.hash()),
            te_load_header_with_validity("vb-1", prep.headers.h2.hash()),
            te_load_header_with_validity("vb-1", prep.headers.h1.hash()),
            te_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, false)),
            te_state("vb-1", &prep.state).into(),
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_ledger_contains("vb-1", &parent),
            te_ledger_tip("vb-1"),
            te_get_anchor_hash("vb-1"),
            te_load_header("vb-1", prep.headers.h0.hash()),
            te_load_header_with_validity("vb-1", prep.headers.h2a.hash()),
            te_load_header_with_validity("vb-1", prep.headers.h1.hash()),
            te_ledger_contains("vb-1", &prep.headers.h2a.point()),
            te_load_header_with_validity("vb-1", prep.headers.h0.hash()),
            te_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, false)),
            te_state("vb-1", &prep.state).into(),
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            te_terminate("vb-1"),
            te_terminated("vb-1", TerminationReason::Voluntary),
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
            te_state("vb-1", &prep.state).into(),
            te_input("vb-1", &msg).into(),
            te_validate_block("vb-1", &Peer::new("unknown"), tip.point()),
            te_send("vb-1", "select_chain", SelectChainMsg::BlockValidationResult(tip, false)),
            te_state("vb-1", &prep.state).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["validating block"])
        .assert_and_remove(Level::WARN, &["invalid block"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}
