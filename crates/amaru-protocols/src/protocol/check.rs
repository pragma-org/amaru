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

use crate::protocol::{ProtocolState, Role, RoleT};
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

pub struct ProtoSpec<State, Message, R> {
    transitions: BTreeMap<State, PerState<State, Message>>,
    _phantom: PhantomData<R>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
struct PerState<State, Message> {
    agency: Role,
    transitions: BTreeMap<Message, (Role, State, bool)>,
}

impl<State, Message> PerState<State, Message> {
    fn initiator() -> Self {
        Self {
            agency: Role::Initiator,
            transitions: Default::default(),
        }
    }

    fn responder() -> Self {
        Self {
            agency: Role::Responder,
            transitions: Default::default(),
        }
    }

    fn role(role: Role) -> Self {
        Self {
            agency: role,
            transitions: Default::default(),
        }
    }

    fn insert(&mut self, msg: Message, role: Role, to: State) -> Option<(Role, State)>
    where
        Message: std::fmt::Debug + Ord,
        State: std::fmt::Debug,
    {
        assert_eq!(self.agency, role, "inserting {msg:?}@{role:?} to {to:?}");
        self.transitions
            .insert(msg, (role, to, false))
            .map(|(r, t, _)| (r, t))
    }

    fn insert_sim_open(&mut self, msg: Message, role: Role, to: State) -> Option<(Role, State)>
    where
        Message: std::fmt::Debug + Ord,
        State: std::fmt::Debug,
    {
        assert_eq!(self.agency, role, "inserting {msg:?}@{role:?} to {to:?}");
        self.transitions
            .insert(msg, (role, to, true))
            .map(|(r, t, _)| (r, t))
    }
}

impl<State, Message, R> Default for ProtoSpec<State, Message, R> {
    fn default() -> Self {
        Self {
            transitions: Default::default(),
            _phantom: PhantomData,
        }
    }
}

impl<State, Message, R> ProtoSpec<State, Message, R>
where
    State: Ord + std::fmt::Debug + Clone + ProtocolState<R, WireMsg = Message>,
    Message: Ord + std::fmt::Debug + Clone,
    R: RoleT,
{
    /// Add a transition that can be executed by the initiator.
    pub fn init(&mut self, from: State, msg: Message, to: State) {
        let present = self
            .transitions
            .entry(from.clone())
            .or_insert_with(PerState::initiator)
            .insert(msg.clone(), Role::Initiator, to.clone());
        if let Some(present) = present {
            panic!(
                "transition {:?} -> {:?} -> {:?} already defined when inserting {:?}",
                from, msg, present, to
            );
        }
    }

    /// Add a transition that can be executed by the responder.
    pub fn resp(&mut self, from: State, msg: Message, to: State) {
        let present = self
            .transitions
            .entry(from.clone())
            .or_insert_with(PerState::responder)
            .insert(msg.clone(), Role::Responder, to.clone());
        if let Some(present) = present {
            panic!(
                "transition {:?} -> {:?} -> {:?} already defined when inserting {:?}",
                from, msg, present, to
            );
        }
    }

    pub fn sim_open(&mut self, from: State, msg: Message, to: State) {
        let present = self
            .transitions
            .entry(from.clone())
            .or_insert_with(PerState::responder)
            .insert_sim_open(msg.clone(), Role::Responder, to.clone());
        if let Some(present) = present {
            panic!(
                "transition {:?} -> {:?} -> {:?} already defined when inserting {:?}",
                from, msg, present, to
            );
        }
    }

    /// Check that the protocol implementation follows the spec.
    ///
    /// The `local_msg` function turns the network message under test
    /// into a local action so that the protocol can be tested.
    ///
    /// The `basic_msg` function can be used to canonicalize the message
    /// in case properties are modified while sending or receiving; this
    /// may be necessary because `check` uses PartialEq for comparison.
    #[expect(clippy::expect_used)]
    pub fn check(&self, initial: State, local_msg: impl Fn(&Message) -> Option<State::Action>) {
        let role = const { R::ROLE.unwrap() };

        let states = self.transitions.keys().collect::<Vec<_>>();
        let messages = self
            .transitions
            .values()
            .flat_map(|m| m.transitions.keys())
            .collect::<BTreeSet<_>>();

        let (out, init) = initial.init().unwrap();
        match role {
            Role::Initiator => {
                if let Some(_send) = out.send.as_ref() {
                    assert_ne!(
                        initial, init,
                        "initialization with send must transition to a different state"
                    );
                } else {
                    assert_eq!(
                        initial, init,
                        "initialization without send must remain in the same state"
                    );
                }
            }
            Role::Responder => {
                assert!(out.send.is_none());
                assert_eq!(
                    initial, init,
                    "initialization without send must remain in the same state"
                );
            }
        }
        assert_eq!(
            out.want_next,
            self.transitions
                .get(&init)
                .expect("init() transitions to non-existent state")
                .agency
                == role.opposite(),
            "initialization must want_next for responder and not for initiator (unless sending from init()) (got {out:?})"
        );

        for state in states {
            for &message in &messages {
                let to = self
                    .transitions
                    .get(state)
                    .and_then(|m| m.transitions.get(message));
                if state == &initial && Some(message) == out.send.as_ref() {
                    assert_eq!(Some(&init), to.map(|(_, s, _)| s));
                    continue;
                }
                let (must_be_local, is_sim_open) = to
                    .map(|(r, _, s)| (*r == role, *s))
                    .unwrap_or((false, false));

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
                        panic!(
                            "local message {message:?} not declared for {state:?} in check() arguments"
                        );
                    };
                    state.local(local_msg).ok()
                } else {
                    assert_eq!(
                        None,
                        local_msg(message).and_then(|action| state.local(action).ok()),
                        "state {state:?} allows local message {message:?} while the peer may have agency"
                    );
                    state
                        .network(message.clone())
                        .ok()
                        .map(|(outcome, next)| (outcome.without_result(), next))
                };

                let ((r, to, _), (send, next)) = match (to, outcome) {
                    (None, None) => continue,
                    (None, Some(_)) => panic!("extraneous transition {:?} -> {:?}", state, message),
                    (Some(_), None) => panic!(
                        "missing transition {:?} -> {:?} for {:?}",
                        state, message, to
                    ),
                    (Some(to), Some(outcome)) => (to, outcome),
                };
                // we only get here if `to` was `Some`, meaning that must_be_local == is_local
                let is_local = must_be_local;

                if is_local {
                    assert_eq!(*r, role, "sending {message:?} not allowed for {role:?}");
                    assert_eq!(
                        send.send.as_ref(),
                        Some(message),
                        "sending message in state {state:?}"
                    );
                    assert_eq!(
                        &next, to,
                        "final state mismatch for {state:?} -> {message:?}"
                    );
                } else {
                    assert_eq!(
                        *r,
                        role.opposite(),
                        "expecting {message:?} not allowed for {role:?}"
                    );
                    if let Some(send) = send.send.as_ref() {
                        let to2 = self
                            .transitions
                            .get(to)
                            .and_then(|m| m.transitions.get(send));
                        if let Some((r2, to2, _)) = to2 {
                            assert_eq!(*r2, role, "sending {send:?} not allowed for {role:?}");
                            assert_eq!(to2, &next, "final state mismatch for {to:?} -> {send:?}");
                        } else {
                            panic!("extraneous transition {:?} -> {:?}", to, send);
                        }
                    } else {
                        assert_eq!(
                            &next, to,
                            "final state mismatch for {state:?} -> {message:?}"
                        );
                    }
                }

                // check that want-next is called when transitioning into a state with remote agency
                // (note that transition into final state will yield None for the get())
                if let Some(s) = self.transitions.get(&next) {
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

    /// Assert that this protocol refines the other protocol.
    ///
    /// This means that this protocol has more states than the other
    /// protocol, thus the state projection must be a surjection.
    pub fn assert_refines<S2, R2>(
        &self,
        other: &ProtoSpec<S2, Message, R2>,
        surjection: impl Fn(&State) -> S2,
    ) where
        S2: Ord + std::fmt::Debug + Clone + ProtocolState<R2, WireMsg = Message>,
        R2: RoleT,
    {
        let mut simplified = BTreeMap::<S2, PerState<S2, Message>>::new();

        for (from, per_state) in &self.transitions {
            let from = surjection(from);
            for (message, (role, to, _)) in per_state.transitions.iter() {
                let to = surjection(to);
                let existing_target = simplified
                    .entry(from.clone())
                    .or_insert_with(|| PerState::role(*role))
                    .insert(message.clone(), *role, to.clone());
                if let Some((existing_role, existing_target)) = existing_target.as_ref()
                    && (existing_target != &to || existing_role != role)
                {
                    panic!(
                        "transition {:?} -> {:?} -> {:?} already defined with different target state when inserting {:?}",
                        from, message, existing_target, to
                    );
                }
            }
        }

        assert_eq!(simplified, other.transitions);
    }
}
