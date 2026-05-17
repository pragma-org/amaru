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

use std::{collections::BTreeMap, sync::Arc};

use amaru_kernel::{BlockHeight, HeaderHash, Point, Slot};
use amaru_ouroboros_traits::{StoreError, overriding_consensus_store::OverridingChainStore};
use pure_stage::{
    assert_trace_contains,
    trace_buffer::{TerminationReason, TraceEntry},
};
use tracing::Level;

use super::*;
use crate::stages::{
    select_chain::test_setup::{
        setup, te_get_anchor_hash, te_get_children, te_has_header, te_load_header, te_load_tip, te_set_block_valid,
        te_unvalidated_ancestor_hashes, test_prep,
    },
    test_utils::{assert_trace, te_input, te_send, te_state, te_terminate, te_terminated},
};

#[test]
fn test_tip_not_found() {
    let prep = test_prep();
    let state = prep.state.clone();
    // Tip for h3 but store only has h0, h1 - not h3
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1]);
    let tip = prep.headers.h3.tip();
    let parent = prep.headers.h2.point();

    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_terminate("sc-1"),
            te_terminated("sc-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["tip not found"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_already_validated() {
    let prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.set_validity(prep.headers.h2.hash(), true);
    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_terminate("sc-1"),
            te_terminated("sc-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["got tip from upstream that was already validated"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_extends_from_origin() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0]);
    let tip = prep.headers.h0.tip();
    let parent = Point::Origin;
    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    let expected = SelectChain {
        best_tip: Some(prep.header(tip.hash())),
        tips: BTreeMap::from_iter([(tip.hash(), vec![tip.hash()])]),
        may_fetch_blocks: false,
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_send("sc-1", "downstream", (tip, parent)),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["got new tip from upstream"])
        .assert_and_remove(Level::DEBUG, &["new chain from origin"])
        .assert_and_remove(Level::DEBUG, &["new best tip candidate"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_extends_from_h1() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h1.clone());
    prep.store_headers(&prep.headers.main());
    let tip = prep.headers.h2.tip();
    let parent = prep.headers.h1.point();
    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    let expected = SelectChain {
        best_tip: Some(prep.header(tip.hash())),
        tips: BTreeMap::from_iter([(
            tip.hash(),
            vec![prep.headers.h0.hash(), prep.headers.h1.hash(), prep.headers.h2.hash()],
        )]),
        may_fetch_blocks: false,
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_unvalidated_ancestor_hashes("sc-1", parent.hash()),
            te_send("sc-1", "downstream", (tip, parent)),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["got new tip from upstream"])
        .assert_and_remove(Level::DEBUG, &["new chain"])
        .assert_and_remove(Level::DEBUG, &["new best tip candidate"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_h3_extends_with_anchor_at_h2() {
    let prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.set_anchor(prep.headers.h2.hash());
    let tip = prep.headers.h3.tip();
    let parent = prep.headers.h2.point();
    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    let expected = SelectChain {
        best_tip: Some(prep.header(tip.hash())),
        tips: BTreeMap::from_iter([(tip.hash(), vec![prep.headers.h2.hash(), tip.hash()])]),
        may_fetch_blocks: false,
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_unvalidated_ancestor_hashes("sc-1", parent.hash()),
            te_send("sc-1", "downstream", (tip, parent)),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["got new tip from upstream"])
        .assert_and_remove(Level::DEBUG, &["new chain"])
        .assert_and_remove(Level::DEBUG, &["new best tip candidate"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_h3_extends_with_best_chain_h3a() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h3a.clone());
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()])]);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    let tip = prep.headers.h3.tip();
    let parent = prep.headers.h2.point();
    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    let expected = SelectChain {
        best_tip: Some(prep.header(tip.hash())),
        tips: BTreeMap::from_iter([
            (
                tip.hash(),
                vec![prep.headers.h0.hash(), prep.headers.h1.hash(), prep.headers.h2.hash(), prep.headers.h3.hash()],
            ),
            (prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()]),
        ]),
        may_fetch_blocks: false,
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_unvalidated_ancestor_hashes("sc-1", parent.hash()),
            te_send("sc-1", "downstream", (tip, parent)),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["got new tip from upstream"])
        .assert_and_remove(Level::DEBUG, &["new chain"])
        .assert_and_remove(Level::DEBUG, &["new best tip candidate"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_h3a_extends_with_best_chain_h3() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]);
    prep.set_validity(prep.headers.h1.hash(), true);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    let tip = prep.headers.h3a.tip();
    let parent = prep.headers.h2a.point();
    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    let expected = SelectChain {
        tips: BTreeMap::from_iter([
            (prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()]),
            (tip.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()]),
        ]),
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_unvalidated_ancestor_hashes("sc-1", parent.hash()),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["got new tip from upstream"])
        .assert_and_remove(Level::DEBUG, &["new chain"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_h3a_extends_with_best_chain_h2() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h2.clone());
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h2.hash(), vec![prep.headers.h1.hash(), prep.headers.h2.hash()])]);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h1.hash());
    let tip = prep.headers.h3a.tip();
    let parent = prep.headers.h2a.point();
    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    let expected = SelectChain {
        best_tip: Some(prep.header(tip.hash())),
        tips: BTreeMap::from_iter([
            (tip.hash(), vec![prep.headers.h1.hash(), prep.headers.h2a.hash(), prep.headers.h3a.hash()]),
            (prep.headers.h2.hash(), vec![prep.headers.h1.hash(), prep.headers.h2.hash()]),
        ]),
        may_fetch_blocks: false,
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_unvalidated_ancestor_hashes("sc-1", parent.hash()),
            te_send("sc-1", "downstream", (tip, parent)),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["got new tip from upstream"])
        .assert_and_remove(Level::DEBUG, &["new chain"])
        .assert_and_remove(Level::DEBUG, &["new best tip candidate"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_upstream_tip_depends_on_invalid_block() {
    let prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.set_validity(prep.headers.h1.hash(), false);
    prep.set_anchor(prep.headers.h0.hash());
    let tip = prep.headers.h3.tip();
    let parent = prep.headers.h2.point();
    let msg = SelectChainMsg::TipFromUpstream(tip, parent);

    // Invalid chains are ignored: no send, best_tip stays Origin.
    let mut expected = SelectChain::new(prep.downstream.clone());
    expected.may_fetch_blocks = true;
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_unvalidated_ancestor_hashes("sc-1", parent.hash()),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["got new tip from upstream"])
        .assert_and_remove(Level::INFO, &["upstream tip depends on invalid block"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_validation_result_valid() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]);
    prep.store_headers(&prep.headers.main());
    let tip = prep.headers.h2.tip();
    let msg = SelectChainMsg::BlockValidationResult(tip, true);

    let expected = SelectChain {
        tips: BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h3.hash()])]),
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", tip.hash()),
            te_set_block_valid("sc-1", tip.hash(), true),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_validation_result_invalid_best_tip_invalidated() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]);
    prep.store_headers(&prep.headers.main());
    prep.set_anchor(prep.headers.h0.parent_hash().unwrap_or(ORIGIN_HASH));
    prep.set_validity(prep.headers.h0.hash(), true);
    prep.set_validity(prep.headers.h1.hash(), true);
    prep.set_best_chain(prep.headers.h1.hash());
    let tip = prep.headers.h2.tip();
    let msg = SelectChainMsg::BlockValidationResult(tip, false);

    // Fallback uses get_anchor_hash; we set best_tip but tips stays empty (we don't reconstruct).
    let expected = SelectChain {
        best_tip: Some(prep.headers.h1.clone()),
        tips: BTreeMap::from_iter([(prep.headers.h1.hash(), vec![])]),
        may_fetch_blocks: false,
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", tip.hash()),
            te_set_block_valid("sc-1", tip.hash(), false),
            te_get_anchor_hash("sc-1"),
            te_get_children("sc-1", ORIGIN_HASH),
            te_load_header("sc-1", prep.headers.h0.hash(), true),
            te_get_children("sc-1", prep.headers.h0.hash()),
            te_load_header("sc-1", prep.headers.h1.hash(), true),
            te_get_children("sc-1", prep.headers.h1.hash()),
            te_load_header("sc-1", prep.headers.h2.hash(), true),
            te_unvalidated_ancestor_hashes("sc-1", prep.headers.h1.hash()),
            te_load_tip("sc-1", prep.headers.h0.hash()),
            te_send("sc-1", "downstream", (prep.headers.h1.tip(), prep.headers.h0.point())),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["best tip candidate invalidated"])
        .assert_and_remove(Level::DEBUG, &["new best tip candidate"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_validation_result_invalid_best_tip_invalidated_switch_fork() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.tips = BTreeMap::from_iter([
        (prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()]),
        (prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()]),
    ]);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.parent_hash().unwrap_or(ORIGIN_HASH));
    prep.set_validity(prep.headers.h0.hash(), true);
    prep.set_validity(prep.headers.h1.hash(), true);
    prep.set_best_chain(prep.headers.h1.hash());
    let tip = prep.headers.h2.tip();
    let msg = SelectChainMsg::BlockValidationResult(tip, false);

    // Fallback uses get_anchor_hash; we set best_tip but tips stays empty (we don't reconstruct).
    let expected = SelectChain {
        best_tip: Some(prep.headers.h3a.clone()),
        tips: BTreeMap::from_iter([(prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()])]),
        may_fetch_blocks: false,
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", tip.hash()),
            te_set_block_valid("sc-1", tip.hash(), false),
            te_get_anchor_hash("sc-1"),
            te_get_children("sc-1", ORIGIN_HASH),
            te_load_header("sc-1", prep.headers.h0.hash(), true),
            te_get_children("sc-1", prep.headers.h0.hash()),
            te_load_header("sc-1", prep.headers.h1.hash(), true),
            te_get_children("sc-1", prep.headers.h1.hash()),
            te_load_header("sc-1", prep.headers.h2a.hash(), true),
            te_get_children("sc-1", prep.headers.h2a.hash()),
            te_load_header("sc-1", prep.headers.h3a.hash(), true),
            te_get_children("sc-1", prep.headers.h3a.hash()),
            te_load_header("sc-1", prep.headers.h2.hash(), true),
            te_unvalidated_ancestor_hashes("sc-1", prep.headers.h3a.hash()),
            te_load_tip("sc-1", prep.headers.h2a.hash()),
            te_send("sc-1", "downstream", (prep.headers.h3a.tip(), prep.headers.h2a.point())),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["best tip candidate invalidated"])
        .assert_and_remove(Level::DEBUG, &["new best tip candidate"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_validation_result_invalid_removes_tips() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.tips = BTreeMap::from_iter([
        (prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()]),
        (prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()]),
    ]);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    let tip = prep.headers.h2a.tip();
    let msg = SelectChainMsg::BlockValidationResult(tip, false);

    let expected = SelectChain {
        best_tip: Some(prep.headers.h3.clone()),
        tips: BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]),
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", tip.hash()),
            te_set_block_valid("sc-1", tip.hash(), false),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::WARN, &["chain fork(s) removed due to invalid block"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_block_validation_result_invalid_for_unknown_hash() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.store_headers(&prep.headers.main());
    let unknown_hash = HeaderHash::from([99u8; 32]);
    let tip = Tip::new(Point::Specific(Slot::from(999), unknown_hash), BlockHeight::from(0));
    let msg = SelectChainMsg::BlockValidationResult(tip, false);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", unknown_hash),
            te_terminate("sc-1"),
            te_terminated("sc-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["header not found"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_fault_set_block_valid_returns_err_failed_to_store_block_validation_result() {
    let mut prep = test_prep();
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]);
    prep.store_headers(&prep.headers.main());
    prep.store = Arc::new(
        OverridingChainStore::builder(prep.store)
            .with_set_block_valid(|_store, _hash, _valid| {
                Err(StoreError::WriteError { error: "injected fault".into() })
            })
            .build(),
    );
    let tip = prep.headers.h2.tip();
    let msg = SelectChainMsg::BlockValidationResult(tip, true);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", tip.hash()),
            te_set_block_valid("sc-1", tip.hash(), true),
            te_terminate("sc-1"),
            te_terminated("sc-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["failed to store block validation result", "injected fault"])
        .assert_and_remove(Level::INFO, &["terminated", "stage=sc-1"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_startup_with_non_empty_store() {
    let mut prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.may_fetch_blocks = false;
    let msg = SelectChainMsg::FetchNextFrom(Point::Origin);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", prep.headers.h3.hash(), false),
            te_load_tip("sc-1", prep.headers.h2.hash()),
            te_send("sc-1", "downstream", (prep.headers.h3.tip(), prep.headers.h2.point())),
            te_state("sc-1", &prep.state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["resuming block fetching"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

// ---------------------------------------------------------------------------
// New stage tests covering previously weak coverage areas
// ---------------------------------------------------------------------------

#[test]
fn test_fetch_next_from_resumes_best_candidate() {
    let mut prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.may_fetch_blocks = false;

    let msg = SelectChainMsg::FetchNextFrom(prep.headers.h1.point());

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(
        &running,
        &[
            te_input("sc-1", &msg).into(),
            te_load_header("sc-1", prep.headers.h3.hash(), false).into(),
            te_load_tip("sc-1", prep.headers.h2.hash()).into(),
            te_send("sc-1", "downstream", (prep.headers.h3.tip(), prep.headers.h2.point())).into(),
        ],
    );

    logs.assert_and_remove(Level::DEBUG, &["resuming block fetching"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_fetch_next_from_enables_may_fetch_blocks() {
    let mut prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.may_fetch_blocks = false;

    // Asking for the current best tip point → just enable fetching
    let msg = SelectChainMsg::FetchNextFrom(prep.headers.h3.point());

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    // Inspect the final state in the trace
    let final_state = running
        .trace_buffer()
        .lock()
        .iter_entries()
        .filter_map(|(_, e)| {
            if let TraceEntry::State { stage, state } = e {
                if stage.as_str() == "sc-1" { state.cast::<SelectChain>().ok() } else { None }
            } else {
                None
            }
        })
        .last()
        .expect("expected final state");

    assert!(final_state.may_fetch_blocks, "expected may_fetch_blocks to be enabled");

    logs.assert_no_remaining_at([Level::ERROR]);
}

#[test]
fn test_last_best_tip_invalidated_falls_back_to_origin() {
    let mut prep = test_prep();
    prep.store_headers(&prep.headers.main());
    prep.set_anchor(prep.headers.h3.hash()); // Make h3 the anchor so that when it is invalidated, nothing is left
    prep.state.best_tip = Some(prep.headers.h3.clone());
    prep.state.tips.insert(prep.headers.h3.hash(), vec![prep.headers.h3.hash()]);

    // Invalidate the last remaining best tip candidate (the anchor)
    let msg = SelectChainMsg::BlockValidationResult(prep.headers.h3.tip(), false);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(
        &running,
        &[
            te_input("sc-1", &msg).into(),
            te_has_header("sc-1", prep.headers.h3.hash()).into(),
            te_set_block_valid("sc-1", prep.headers.h3.hash(), false).into(),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["best tip candidate invalidated"]).assert_no_remaining_at([Level::ERROR]);
}

#[cfg(test)]
mod best_tip_from_store_tests {
    use std::sync::Arc;

    use amaru_kernel::{
        BlockHeader, HeaderHash, IsHeader, any_headers_chain, any_headers_chain_with_root, make_header,
        utils::tests::run_strategy,
    };
    use amaru_ouroboros::{ChainStore, in_memory_consensus_store::InMemConsensusStore};
    use amaru_protocols::store_effects::Store;
    use pure_stage::simulation::simulation_builder::run_function_with_resource;

    use crate::stages::select_chain::best_tip_from_store;

    #[test]
    fn falls_back_to_best_chain_when_descendant_has_invalid_ancestry() {
        let chain = run_strategy(any_headers_chain(4));
        let [a, b, c, d] = chain.try_into().unwrap();

        let store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(InMemConsensusStore::default());
        for header in [&a, &b, &c, &d] {
            store.store_header(header).unwrap();
        }
        store.set_anchor_hash(&a.hash()).unwrap();
        for header in [&a, &b] {
            store.roll_forward_chain(&header.point()).unwrap();
        }
        store.set_block_valid(&b.hash(), true).unwrap();
        store.set_block_valid(&c.hash(), false).unwrap();

        let candidate = run_best_tip_from_store(store.clone());
        assert_eq!(candidate, Some((b, vec![])));
    }

    #[test]
    fn keeps_candidate_with_valid_ancestry() {
        let chain = run_strategy(any_headers_chain(4));
        let [a, b, c, d] = chain.try_into().unwrap();

        let store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(InMemConsensusStore::default());
        for header in [&a, &b, &c, &d] {
            store.store_header(header).unwrap();
        }
        store.set_anchor_hash(&a.hash()).unwrap();
        for header in [&a, &b] {
            store.roll_forward_chain(&header.point()).unwrap();
        }
        store.set_block_valid(&b.hash(), true).unwrap();
        let d_hash = d.hash();

        let candidate = run_best_tip_from_store(store.clone());
        assert_eq!(candidate, Some((d, vec![c.hash(), d_hash])));
    }

    #[test]
    fn falls_back_to_valid_candidate_when_best_candidate_has_invalid_ancestry() {
        let store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(InMemConsensusStore::default());

        // a -- b -- c (invalid) -- d
        //       \
        //        e
        let chain = run_strategy(any_headers_chain(2));
        let [a, b] = chain.try_into().unwrap();
        let invalid_branch = run_strategy(any_headers_chain_with_root(2, b.point()));
        let [c, d] = invalid_branch.try_into().unwrap();
        let e = BlockHeader::from(make_header(3, c.slot().as_u64() + 10, Some(b.hash())));

        for header in [&a, &b, &c, &d, &e] {
            store.store_header(header).unwrap();
        }
        store.set_anchor_hash(&a.hash()).unwrap();
        for header in [&a, &b] {
            store.roll_forward_chain(&header.point()).unwrap();
        }
        store.set_block_valid(&b.hash(), true).unwrap();
        store.set_block_valid(&c.hash(), false).unwrap();

        let candidate = run_best_tip_from_store(store.clone());
        assert_eq!(candidate, Some((e.clone(), vec![e.hash()])));
    }

    #[test]
    fn returns_the_best_chain_tip_when_it_has_no_descendants() {
        let chain = run_strategy(any_headers_chain(2));
        let [a, b] = chain.clone().try_into().unwrap();

        let store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(InMemConsensusStore::default());
        for header in [&a, &b] {
            store.store_header(header).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
        }
        store.set_anchor_hash(&a.hash()).unwrap();
        store.set_block_valid(&a.hash(), true).unwrap();
        store.set_block_valid(&b.hash(), true).unwrap();

        let candidate = run_best_tip_from_store(store.clone());
        assert_eq!(candidate, Some((b, vec![])));
    }

    // HELPERS

    fn run_best_tip_from_store(store: Arc<dyn ChainStore<BlockHeader>>) -> Option<(BlockHeader, Vec<HeaderHash>)> {
        run_function_with_resource(store.clone(), |eff| async { best_tip_from_store(&Store::new(eff)).await.unwrap() })
    }
}

/// Traditional unit tests for `cmp_tip` (outside the simulation stage harness).
///
/// These tests construct headers directly (which are not valid for consensus)
/// because we need fine control over `op_cert_seq` and the VRF output to
/// exercise all branches of `cmp_tip`.
#[cfg(test)]
mod cmp_tip_unit_tests {
    use std::cmp::Ordering;

    use amaru_kernel::{BlockHeader, Bytes, Hasher, HeaderHash, size::BLOCK_BODY};
    use amaru_ouroboros::OperationalCert;
    use pallas_primitives::{
        VrfCert,
        babbage::{HeaderBody, PseudoHeader},
    };

    fn make_test_header(
        block_number: u64,
        slot: u64,
        prev_hash: Option<HeaderHash>,
        op_cert_seq: u64,
        vrf: &[u8],
    ) -> BlockHeader {
        let block_hash = Hasher::<{ BLOCK_BODY * 8 }>::hash_cbor(&vec![block_number, slot]);

        let header = PseudoHeader {
            header_body: HeaderBody {
                block_number,
                slot,
                prev_hash,
                issuer_vkey: Bytes::from(vec![]),
                vrf_vkey: Bytes::from(vec![]),
                vrf_result: VrfCert(Bytes::from(vrf.to_vec()), Bytes::from(vec![])),
                block_body_size: 0,
                block_body_hash: block_hash,
                operational_cert: OperationalCert {
                    operational_cert_hot_vkey: Bytes::from(vec![]),
                    operational_cert_sequence_number: op_cert_seq,
                    operational_cert_kes_period: 0,
                    operational_cert_sigma: Bytes::from(vec![]),
                },
                protocol_version: (1, 2),
            },
            body_signature: Bytes::from(vec![]),
        };

        BlockHeader::from(header)
    }

    fn make_h(height: u64, slot: u64, opcert: u64, vrf: u8) -> BlockHeader {
        make_test_header(height, slot, None, opcert, &[vrf; 32])
    }

    #[test]
    fn test_both_none() {
        assert_eq!(super::cmp_tip(None, None), Ordering::Equal);
    }

    #[test]
    fn test_none_vs_some() {
        let a = make_h(1, 1, 0, 0);
        assert_eq!(super::cmp_tip(None, Some(&a)), Ordering::Less);
        assert_eq!(super::cmp_tip(Some(&a), None), Ordering::Greater);
    }

    #[test]
    fn test_height_wins() {
        let taller = make_h(10, 100, 0, 0);
        let shorter = make_h(5, 200, 0, 0);
        assert_eq!(super::cmp_tip(Some(&taller), Some(&shorter)), Ordering::Greater);
        assert_eq!(super::cmp_tip(Some(&shorter), Some(&taller)), Ordering::Less);
    }

    #[test]
    fn test_same_height_higher_opcert_wins() {
        let higher = make_h(5, 10, 42, 0);
        let lower = make_h(5, 10, 7, 0);
        assert_eq!(super::cmp_tip(Some(&higher), Some(&lower)), Ordering::Greater);
        assert_eq!(super::cmp_tip(Some(&lower), Some(&higher)), Ordering::Less);
    }

    #[test]
    fn test_same_height_same_opcert_equal() {
        let a = make_h(5, 10, 42, 0);
        let b = make_h(5, 10, 42, 0);
        assert_eq!(super::cmp_tip(Some(&a), Some(&b)), Ordering::Equal);
    }

    #[test]
    fn test_same_height_different_vrf_slot_diff_4() {
        // slot distance = 4 <= 5 → VRF decides (lower VRF wins)
        let a = make_h(5, 10, 0, 10); // higher VRF
        let b = make_h(5, 14, 0, 5); // lower VRF, distance 4
        assert_eq!(super::cmp_tip(Some(&a), Some(&b)), Ordering::Less);
        assert_eq!(super::cmp_tip(Some(&b), Some(&a)), Ordering::Greater);
    }

    #[test]
    fn test_same_height_different_vrf_slot_diff_5() {
        // slot distance = 5 <= 5 → VRF decides
        let a = make_h(5, 10, 0, 10);
        let b = make_h(5, 15, 0, 5);
        assert_eq!(super::cmp_tip(Some(&a), Some(&b)), Ordering::Less);
        assert_eq!(super::cmp_tip(Some(&b), Some(&a)), Ordering::Greater);
    }

    #[test]
    fn test_same_height_different_vrf_slot_diff_6() {
        // slot distance = 6 > 5 → Equal, even with different VRF
        let a = make_h(5, 10, 0, 10);
        let b = make_h(5, 16, 0, 5);
        assert_eq!(super::cmp_tip(Some(&a), Some(&b)), Ordering::Equal);
    }

    #[test]
    fn test_same_height_opcert_takes_precedence_when_vrf_leaders_match() {
        // When VRF leaders are identical, opcert wins regardless of slot distance
        let higher_opcert = make_h(5, 10, 100, 42);
        let lower_opcert = make_h(5, 100, 50, 42);
        assert_eq!(super::cmp_tip(Some(&higher_opcert), Some(&lower_opcert)), Ordering::Greater);
        assert_eq!(super::cmp_tip(Some(&lower_opcert), Some(&higher_opcert)), Ordering::Less);
    }
}
