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

use amaru_kernel::{HeaderHash, ORIGIN_HASH, Point, Slot};
use amaru_ouroboros_traits::{StoreError, overriding_consensus_store::OverridingChainStore};
use pure_stage::trace_buffer::TerminationReason;
use tracing::Level;

use super::*;
use crate::stages::{
    select_chain_new::test_setup::{
        assert_trace, setup, te_get_anchor_hash, te_get_best_chain_hash, te_has_header, te_load_header, te_send,
        te_set_block_valid, te_terminate, te_terminated, test_prep,
    },
    test_utils::{te_input, te_state},
};

#[test]
fn test_tip_not_found() {
    let prep = test_prep();
    let state = prep.state.clone();
    // Tip for h3 but store only has h0, h1 - not h3
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1]);
    let tip = prep.headers.h3.tip();

    let msg = SelectChainMsg::TipFromUpstream(tip);

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
    let msg = SelectChainMsg::TipFromUpstream(tip);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_state("sc-1", &prep.state),
        ],
    );
    logs.assert_and_remove(Level::WARN, &["got tip from upstream that was already validated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_extends_from_origin() {
    let prep = test_prep();
    prep.store_headers(&[&prep.headers.h0]);
    let tip = prep.headers.h0.tip();
    let msg = SelectChainMsg::TipFromUpstream(tip);

    let expected = SelectChain {
        best_tip: tip,
        tips: BTreeMap::from_iter([(tip.hash(), vec![tip.hash()])]),
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_send("sc-1", "downstream", tip),
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
    prep.state.best_tip = prep.headers.h1.tip();
    prep.store_headers(&prep.headers.main());
    let tip = prep.headers.h2.tip();
    let msg = SelectChainMsg::TipFromUpstream(tip);

    let expected = SelectChain {
        best_tip: tip,
        tips: BTreeMap::from_iter([(
            tip.hash(),
            vec![prep.headers.h0.hash(), prep.headers.h1.hash(), prep.headers.h2.hash()],
        )]),
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_get_anchor_hash("sc-1"),
            te_load_header("sc-1", ORIGIN_HASH, false),
            te_load_header("sc-1", prep.headers.h1.hash(), true),
            te_load_header("sc-1", prep.headers.h0.hash(), true),
            te_send("sc-1", "downstream", tip),
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
    let msg = SelectChainMsg::TipFromUpstream(tip);

    let expected = SelectChain {
        best_tip: tip,
        tips: BTreeMap::from_iter([(tip.hash(), vec![prep.headers.h2.hash(), tip.hash()])]),
        ..prep.state.clone()
    };

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_get_anchor_hash("sc-1"),
            te_load_header("sc-1", prep.headers.h2.hash(), false),
            te_load_header("sc-1", prep.headers.h2.hash(), true),
            te_send("sc-1", "downstream", tip),
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
    prep.state.best_tip = prep.headers.h3a.tip();
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()])]);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    let tip = prep.headers.h3.tip();
    let msg = SelectChainMsg::TipFromUpstream(tip);

    let expected = SelectChain {
        best_tip: tip,
        tips: BTreeMap::from_iter([
            (
                tip.hash(),
                vec![prep.headers.h0.hash(), prep.headers.h1.hash(), prep.headers.h2.hash(), prep.headers.h3.hash()],
            ),
            (prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()]),
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
            te_get_anchor_hash("sc-1"),
            te_load_header("sc-1", prep.headers.h0.hash(), false),
            te_load_header("sc-1", prep.headers.h2.hash(), true),
            te_load_header("sc-1", prep.headers.h1.hash(), true),
            te_load_header("sc-1", prep.headers.h0.hash(), true),
            te_send("sc-1", "downstream", tip),
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
    prep.state.best_tip = prep.headers.h3.tip();
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]);
    prep.set_validity(prep.headers.h1.hash(), true);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    let tip = prep.headers.h3a.tip();
    let msg = SelectChainMsg::TipFromUpstream(tip);

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
            te_get_anchor_hash("sc-1"),
            te_load_header("sc-1", prep.headers.h0.hash(), false),
            te_load_header("sc-1", prep.headers.h2a.hash(), true),
            te_load_header("sc-1", prep.headers.h1.hash(), true),
            te_load_header("sc-1", prep.headers.h0.hash(), true),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["got new tip from upstream"])
        .assert_and_remove(Level::DEBUG, &["new chain"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_tip_h3_extends_with_best_chain_h2a() {
    let mut prep = test_prep();
    prep.state.best_tip = prep.headers.h2a.tip();
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h2a.hash(), vec![prep.headers.h1.hash(), prep.headers.h2a.hash()])]);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h1.hash());
    let tip = prep.headers.h3.tip();
    let msg = SelectChainMsg::TipFromUpstream(tip);

    let expected = SelectChain {
        best_tip: tip,
        tips: BTreeMap::from_iter([
            (tip.hash(), vec![prep.headers.h1.hash(), prep.headers.h2.hash(), prep.headers.h3.hash()]),
            (prep.headers.h2a.hash(), vec![prep.headers.h1.hash(), prep.headers.h2a.hash()]),
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
            te_get_anchor_hash("sc-1"),
            te_load_header("sc-1", prep.headers.h1.hash(), false),
            te_load_header("sc-1", prep.headers.h2.hash(), true),
            te_load_header("sc-1", prep.headers.h1.hash(), true),
            te_send("sc-1", "downstream", tip),
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
    let msg = SelectChainMsg::TipFromUpstream(tip);

    // Invalid chains are ignored: no send, best_tip stays Origin.
    let expected = SelectChain::new(prep.downstream.clone(), Tip::origin());
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_load_header("sc-1", tip.hash(), true),
            te_get_anchor_hash("sc-1"),
            te_load_header("sc-1", prep.headers.h0.hash(), false),
            te_load_header("sc-1", prep.headers.h2.hash(), true),
            te_load_header("sc-1", prep.headers.h1.hash(), true),
            te_load_header("sc-1", prep.headers.h0.hash(), true),
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
    prep.state.best_tip = prep.headers.h3.tip();
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]);
    prep.store_headers(&prep.headers.main());
    let point = prep.headers.h2.point();
    let msg = SelectChainMsg::BlockValidationResult(point, true);

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
            te_has_header("sc-1", point.hash()),
            te_set_block_valid("sc-1", point.hash(), true),
            te_state("sc-1", &expected),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_validation_result_invalid_best_tip_invalidated() {
    let mut prep = test_prep();
    prep.state.best_tip = prep.headers.h3.tip();
    prep.state.tips =
        BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]);
    prep.store_headers(&prep.headers.main());
    prep.set_validity(prep.headers.h0.hash(), true);
    prep.set_validity(prep.headers.h1.hash(), true);
    prep.set_best_chain(prep.headers.h1.hash());
    let point = prep.headers.h2.point();
    let msg = SelectChainMsg::BlockValidationResult(point, false);

    // Fallback uses get_best_chain_hash; we set best_tip but tips stays empty (we don't reconstruct).
    let expected = SelectChain::new(prep.downstream.clone(), prep.headers.h1.tip());
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", point.hash()),
            te_set_block_valid("sc-1", point.hash(), false),
            te_get_best_chain_hash("sc-1"),
            te_load_header("sc-1", prep.headers.h1.hash(), false),
            te_send("sc-1", "downstream", prep.headers.h1.tip()),
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
    prep.state.best_tip = prep.headers.h3.tip();
    prep.state.tips = BTreeMap::from_iter([
        (prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()]),
        (prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()]),
    ]);
    prep.store_headers(&prep.headers.all());
    prep.set_validity(prep.headers.h0.hash(), true);
    prep.set_validity(prep.headers.h1.hash(), true);
    prep.set_best_chain(prep.headers.h1.hash());
    let point = prep.headers.h2.point();
    let msg = SelectChainMsg::BlockValidationResult(point, false);

    // Fallback uses get_best_chain_hash; we set best_tip but tips stays empty (we don't reconstruct).
    let expected = SelectChain {
        best_tip: prep.headers.h3a.tip(),
        tips: BTreeMap::from_iter([(prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()])]),
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", point.hash()),
            te_set_block_valid("sc-1", point.hash(), false),
            te_load_header("sc-1", prep.headers.h3a.hash(), false),
            te_send("sc-1", "downstream", prep.headers.h3a.tip()),
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
    prep.state.best_tip = prep.headers.h3.tip();
    prep.state.tips = BTreeMap::from_iter([
        (prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()]),
        (prep.headers.h3a.hash(), vec![prep.headers.h2a.hash(), prep.headers.h3a.hash()]),
    ]);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    let point = prep.headers.h2a.point();
    let msg = SelectChainMsg::BlockValidationResult(point, false);

    let expected = SelectChain {
        best_tip: prep.headers.h3.tip(),
        tips: BTreeMap::from_iter([(prep.headers.h3.hash(), vec![prep.headers.h2.hash(), prep.headers.h3.hash()])]),
        ..prep.state.clone()
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", point.hash()),
            te_set_block_valid("sc-1", point.hash(), false),
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
    prep.state.best_tip = prep.headers.h3.tip();
    prep.store_headers(&prep.headers.main());
    let unknown_hash = HeaderHash::from([99u8; 32]);
    let point = Point::Specific(Slot::from(999), unknown_hash);
    let msg = SelectChainMsg::BlockValidationResult(point, false);

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
    prep.state.best_tip = prep.headers.h3.tip();
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
    let point = prep.headers.h2.point();
    let msg = SelectChainMsg::BlockValidationResult(point, true);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("sc-1", &prep.state),
            te_input("sc-1", &msg),
            te_has_header("sc-1", point.hash()),
            te_set_block_valid("sc-1", point.hash(), true),
            te_terminate("sc-1"),
            te_terminated("sc-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["failed to store block validation result", "injected fault"])
        .assert_and_remove(Level::INFO, &["terminated", "stage=sc-1"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}
