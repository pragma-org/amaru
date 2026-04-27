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

use amaru_kernel::{HeaderHash, NonEmptyVec};
use pure_stage::trace_buffer::TerminationReason;
use test_setup::{
    assert_trace, setup, te_find_ancestor_on_best_chain, te_load_header, te_terminate, te_terminated, test_prep,
};
use tracing::Level;

use super::*;
use crate::stages::{
    adopt_chain::test_setup::{
        te_clock, te_find_anchor_at_height, te_roll_forward_chain, te_send, te_set_anchor_hash, te_switch_to_fork,
    },
    test_utils::{te_input, te_state},
};

/// Incoming tip not in store -> terminate.
#[test]
fn test_incoming_tip_not_in_store() {
    let mut prep = test_prep(2);
    prep.store_headers(&[&prep.headers.h0, &prep.headers.h1]);
    prep.set_best_chain(prep.headers.h1.clone());

    let msg = prep.headers.h3.tip(); // h3 not in store
    let (running, _guards, mut logs) = setup(&prep, msg);
    assert_trace(
        &running,
        &[
            te_state("ac-1", &prep.state),
            te_input("ac-1", &AdoptChainMsg::new(msg, BlockHeight::new(0))),
            te_load_header("ac-1", msg.hash()),
            te_terminate("ac-1"),
            te_terminated("ac-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::WARN, &["failed to load incoming tip"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

/// Current best not loadable (best_chain_hash points to missing header) -> terminate.
#[test]
fn test_current_best_not_loadable() {
    let mut prep = test_prep(2);
    prep.store_headers(&prep.headers.main_chain());
    let missing_current_best = Tip::new(Point::Specific(4u64.into(), HeaderHash::from([0u8; 32])), BlockHeight::new(4));
    prep.state.current_best_tip = missing_current_best;

    let msg = prep.headers.h3.tip();
    let (running, _guards, mut logs) = setup(&prep, msg);
    assert_trace(
        &running,
        &[
            te_state("ac-1", &prep.state),
            te_input("ac-1", &AdoptChainMsg::new(msg, BlockHeight::new(0))),
            te_load_header("ac-1", msg.hash()),
            te_load_header("ac-1", missing_current_best.hash()),
            te_terminate("ac-1"),
            te_terminated("ac-1", TerminationReason::Voluntary),
        ],
    );
    logs.assert_and_remove(Level::WARN, &["failed to load current best"])
        .assert_and_remove(Level::INFO, &["terminated"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

/// Incoming tip not better than current best (h3a loses to h3 on op_cert_seq) -> don't adopt, no send.
#[test]
fn test_incoming_not_better_than_current_best() {
    let mut prep = test_prep(2);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_best_chain(prep.headers.h3.clone());

    let msg = prep.headers.h3a.tip(); // h3a has same height as h3 but lower op_cert_seq
    let (running, _guards, mut logs) = setup(&prep, msg);
    assert_trace(
        &running,
        &[
            te_state("ac-1", &prep.state),
            te_input("ac-1", &AdoptChainMsg::new(msg, BlockHeight::new(0))),
            te_load_header("ac-1", msg.hash()),
            te_load_header("ac-1", prep.headers.h3.hash()),
            te_state("ac-1", &prep.state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["incoming tip not better than current best"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

/// Extension: h3 extends h2 -> roll_forward, drag anchor, send.
#[test]
fn test_extension_adopts_and_sends() {
    let mut prep = test_prep(2);
    prep.store_headers(&prep.headers.main_chain());
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_best_chain(prep.headers.h2.clone());

    let msg = prep.headers.h3.tip();
    let (running, _guards, mut logs) = setup(&prep, msg);

    let mut expected = prep.state.clone();
    expected.current_best_tip = msg;
    expected.suppressed = 1;
    assert_trace(
        &running,
        &[
            te_state("ac-1", &prep.state),
            te_input("ac-1", &AdoptChainMsg::new(msg, BlockHeight::new(0))),
            te_load_header("ac-1", msg.hash()),
            te_load_header("ac-1", prep.headers.h2.hash()),
            te_roll_forward_chain("ac-1", msg.point()),
            te_find_anchor_at_height("ac-1", BlockHeight::new(2)),
            te_set_anchor_hash("ac-1", prep.headers.h1.hash()),
            te_clock("ac-1"),
            te_send("ac-1", "downstream", ManagerMessage::NewTip(msg)),
            te_state("ac-1", &expected),
        ],
    );

    // Verify store state: best chain is h3, anchor was dragged forward
    assert_eq!(prep.store.get_best_chain_hash(), prep.headers.h3.hash());
    assert_eq!(prep.store.get_anchor_hash(), prep.headers.h1.hash());

    logs.assert_and_remove(Level::DEBUG, &["adopted tip"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

/// Fork switch: best was h2, adopt h3a (longer fork) -> rollback to h1, roll_forward h2a, h3a, set_best_chain, drag anchor, send.
#[test]
fn test_fork_switch_adopts_and_sends() {
    let mut prep = test_prep(2);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_best_chain(prep.headers.h2.clone());

    let msg = prep.headers.h3a.tip(); // h3a has height 4 > h2's 3, so it wins
    let (running, _guards, mut logs) = setup(&prep, msg);

    let mut expected = prep.state.clone();
    expected.current_best_tip = msg;
    expected.suppressed = 1;
    assert_trace(
        &running,
        &[
            te_state("ac-1", &prep.state),
            te_input("ac-1", &AdoptChainMsg::new(msg, BlockHeight::new(0))),
            te_load_header("ac-1", msg.hash()),
            te_load_header("ac-1", prep.headers.h2.hash()),
            te_find_ancestor_on_best_chain("ac-1", msg.hash()),
            te_switch_to_fork(
                "ac-1",
                prep.headers.h1.point(),
                NonEmptyVec::try_from(vec![prep.headers.h2a.point(), prep.headers.h3a.point()]).unwrap(),
            ),
            te_find_anchor_at_height("ac-1", BlockHeight::new(2)),
            te_set_anchor_hash("ac-1", prep.headers.h1.hash()),
            te_clock("ac-1"),
            te_send("ac-1", "downstream", ManagerMessage::NewTip(msg)),
            te_state("ac-1", &expected),
        ],
    );

    // Verify store state: best chain switched to fork ending at h3a
    assert_eq!(prep.store.get_best_chain_hash(), msg.hash());
    assert_eq!(prep.store.get_anchor_hash(), prep.headers.h1.hash());

    logs.assert_and_remove(Level::DEBUG, &["adopted tip"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_fork_switch_opcert_hacked() {
    let mut prep = test_prep(2);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_best_chain(prep.headers.h2a.clone());

    let msg = prep.headers.h2.tip(); // h2 is newer opcert seq no
    let (running, _guards, mut logs) = setup(&prep, msg);

    let mut expected = prep.state.clone();
    expected.current_best_tip = msg;
    expected.suppressed = 1;
    assert_trace(
        &running,
        &[
            te_state("ac-1", &prep.state),
            te_input("ac-1", &AdoptChainMsg::new(msg, BlockHeight::new(0))),
            te_load_header("ac-1", msg.hash()),
            te_load_header("ac-1", prep.headers.h2a.hash()),
            te_find_ancestor_on_best_chain("ac-1", msg.hash()),
            te_switch_to_fork("ac-1", prep.headers.h1.point(), NonEmptyVec::singleton(prep.headers.h2.point())),
            te_find_anchor_at_height("ac-1", BlockHeight::new(1)),
            te_clock("ac-1"),
            te_send("ac-1", "downstream", ManagerMessage::NewTip(msg)),
            te_state("ac-1", &expected),
        ],
    );

    // Verify store state: best chain switched to fork ending at h3a
    assert_eq!(prep.store.get_best_chain_hash(), msg.hash());
    assert_eq!(prep.store.get_anchor_hash(), prep.headers.h0.hash());

    logs.assert_and_remove(Level::DEBUG, &["adopted tip"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_fork_not_better_no_switch() {
    let mut prep = test_prep(2);
    prep.store_headers(&prep.headers.all());
    prep.set_anchor(prep.headers.h0.hash());
    prep.set_best_chain(prep.headers.h2.clone());

    let msg = prep.headers.h2.tip();
    let (running, _guards, mut logs) = setup(&prep, msg);

    assert_trace(
        &running,
        &[
            te_state("ac-1", &prep.state),
            te_input("ac-1", &AdoptChainMsg::new(msg, BlockHeight::new(0))),
            te_load_header("ac-1", msg.hash()),
            te_load_header("ac-1", prep.headers.h2.hash()),
            te_state("ac-1", &prep.state),
        ],
    );

    // Verify store state: best chain switched to fork ending at h3a
    assert_eq!(prep.store.get_best_chain_hash(), prep.headers.h2.hash());
    assert_eq!(prep.store.get_anchor_hash(), prep.headers.h0.hash());

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}
