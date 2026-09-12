// Copyright 2025 PRAGMA
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

#![expect(clippy::panic, clippy::unwrap_used)]

use std::{collections::BTreeSet, marker::PhantomData};

use super::session::SessionSpec;
use crate::protocol::{ProtocolState, Role, RoleT};

/// Undirected protocol table plus a [`ProtocolState`] checker.
///
/// The table is a [`SessionSpec`]; [`check`](Self::check) is the bound that
/// [`SessionSpec`] does not have.
pub struct ProtoSpec<State, Message, R> {
    inner: SessionSpec<State, Message>,
    _phantom: PhantomData<R>,
}

impl<State, Message, R> Default for ProtoSpec<State, Message, R> {
    fn default() -> Self {
        Self { inner: SessionSpec::default(), _phantom: PhantomData }
    }
}

impl<State, Message, R> ProtoSpec<State, Message, R>
where
    State: Clone + Ord + std::fmt::Debug,
    Message: Clone + Ord + std::fmt::Debug,
{
    /// Add a transition that can be executed by the initiator.
    pub fn init(&mut self, from: State, msg: Message, to: State) {
        self.inner.init(from, msg, to);
    }

    /// Add a transition that can be executed by the responder.
    pub fn resp(&mut self, from: State, msg: Message, to: State) {
        self.inner.resp(from, msg, to);
    }

    pub fn sim_open(&mut self, from: State, msg: Message, to: State) {
        self.inner.sim_open(from, msg, to);
    }

    /// Panic on mismatch. Compares the undirected table after `map`.
    /// Does **not** compare timeouts or start state (`initial`).
    #[track_caller]
    pub fn assert_refines<S2, R2>(&self, other: &ProtoSpec<S2, Message, R2>, map: impl Fn(&State) -> S2)
    where
        S2: Clone + Ord + std::fmt::Debug,
    {
        self.inner.assert_refines(&other.inner, map);
    }
}

impl<State, Message, R> ProtoSpec<State, Message, R>
where
    State: Ord + std::fmt::Debug + Clone + ProtocolState<R, WireMsg = Message>,
    Message: Ord + std::fmt::Debug + Clone,
    R: RoleT,
{
    /// Check that the protocol implementation follows the spec.
    ///
    /// The `local_msg` function turns the network message under test
    /// into a local action so that the protocol can be tested.
    #[expect(clippy::expect_used)]
    pub fn check(&self, initial: State, local_msg: impl Fn(&Message) -> Option<State::Action>) {
        let role = const { R::ROLE.unwrap() };

        let states = self.inner.transitions.keys().collect::<Vec<_>>();
        let messages = self.inner.transitions.values().flat_map(|m| m.transitions.keys()).collect::<BTreeSet<_>>();

        let (out, init) = initial.init().unwrap();
        match role {
            Role::Initiator => {
                if let Some(_send) = out.send.as_ref() {
                    assert_ne!(initial, init, "initialization with send must transition to a different state");
                } else {
                    assert_eq!(initial, init, "initialization without send must remain in the same state");
                }
            }
            Role::Responder => {
                assert!(out.send.is_none());
                assert_eq!(initial, init, "initialization without send must remain in the same state");
            }
        }
        assert_eq!(
            out.want_next,
            self.inner.transitions.get(&init).expect("init() transitions to non-existent state").agency
                == role.opposite(),
            "initialization must want_next for responder and not for initiator (unless sending from init()) (got {out:?})"
        );

        for state in states {
            for &message in &messages {
                let per = self.inner.transitions.get(state);
                let edge = per.and_then(|m| m.transitions.get(message));
                if state == &initial && Some(message) == out.send.as_ref() {
                    assert_eq!(Some(&init), edge.map(|e| &e.to));
                    continue;
                }
                let (must_be_local, is_sim_open) = match (per, edge) {
                    (Some(per), Some(e)) => (per.agency == role, e.sim_open),
                    _ => (false, false),
                };

                let outcome = if must_be_local {
                    assert_eq!(
                        None,
                        state.network(message.clone()).ok(),
                        "state {state:?} allows network message {message:?} while local node has agency"
                    );
                    let Some(local_msg) = local_msg(message) else {
                        if is_sim_open {
                            continue;
                        }
                        panic!("local message {message:?} not declared for {state:?} in check() arguments");
                    };
                    state.local(local_msg).ok()
                } else {
                    assert_eq!(
                        None,
                        local_msg(message).and_then(|action| state.local(action).ok()),
                        "state {state:?} allows local message {message:?} while the peer may have agency"
                    );
                    state.network(message.clone()).ok().map(|(outcome, next)| (outcome.without_result(), next))
                };

                let (edge, (send, next)) = match (edge, outcome) {
                    (None, None) => continue,
                    (None, Some(_)) => panic!("extraneous transition {:?} -> {:?}", state, message),
                    (Some(_), None) => panic!("missing transition {:?} -> {:?} for {:?}", state, message, edge),
                    (Some(edge), Some(outcome)) => (edge, outcome),
                };
                // we only get here if `edge` was `Some`, meaning that must_be_local == is_local
                let is_local = must_be_local;
                let to = &edge.to;

                if is_local {
                    assert_eq!(
                        per.expect("edge implies per-state").agency,
                        role,
                        "sending {message:?} not allowed for {role:?}"
                    );
                    assert_eq!(send.send.as_ref(), Some(message), "sending message in state {state:?}");
                    assert_eq!(&next, to, "final state mismatch for {state:?} -> {message:?}");
                } else {
                    assert_eq!(
                        per.expect("edge implies per-state").agency,
                        role.opposite(),
                        "expecting {message:?} not allowed for {role:?}"
                    );
                    if let Some(send) = send.send.as_ref() {
                        let to_per = self.inner.transitions.get(to);
                        let to2 = to_per.and_then(|m| m.transitions.get(send));
                        if let Some(edge2) = to2 {
                            assert_eq!(
                                to_per.expect("edge implies per-state").agency,
                                role,
                                "sending {send:?} not allowed for {role:?}"
                            );
                            assert_eq!(&edge2.to, &next, "final state mismatch for {to:?} -> {send:?}");
                        } else {
                            panic!("extraneous transition {:?} -> {:?}", to, send);
                        }
                    } else {
                        assert_eq!(&next, to, "final state mismatch for {state:?} -> {message:?}");
                    }
                }

                // check that want-next is called when transitioning into a state with remote agency
                // (note that transition into final state will yield None for the get())
                if let Some(s) = self.inner.transitions.get(&next) {
                    if s.agency == role.opposite() {
                        assert!(
                            send.want_next,
                            "transition into state with remote agency requires want_next: {state:?} -> {message:?} -> {to:?} (got {send:?})"
                        );
                    } else {
                        assert!(
                            !send.want_next,
                            "transition into state with local agency should not want_next: {state:?} -> {message:?} -> {to:?} (got {send:?})"
                        );
                    }
                }
            }
        }
    }
}
