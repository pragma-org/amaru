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
    transitions: BTreeMap<State, BTreeMap<Message, (Role, State)>>,
    _phantom: PhantomData<R>,
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
            .or_default()
            .insert(msg.clone(), (Role::Initiator, to.clone()));
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
            .or_default()
            .insert(msg.clone(), (Role::Responder, to.clone()));
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
    pub fn check(&self, initial: State, local_msg: impl Fn(&Message) -> Option<State::Action>) {
        let role = const { R::ROLE.unwrap() };

        let states = self.transitions.keys().collect::<Vec<_>>();
        let messages = self
            .transitions
            .values()
            .flat_map(|m| m.keys())
            .collect::<BTreeSet<_>>();

        let (out, init) = initial.init().unwrap();
        match role {
            Role::Initiator => {
                if out.send.is_none() {
                    assert_eq!(initial, init);
                }
            }
            Role::Responder => {
                assert!(out.send.is_none());
                assert_eq!(initial, init);
            }
        }

        for state in states {
            for &message in &messages {
                let to = self.transitions.get(state).and_then(|m| m.get(message));
                if state == &initial && Some(message) == out.send.as_ref() {
                    assert_eq!(Some(&init), to.map(|(_, s)| s));
                    continue;
                }
                let (outcome, is_local) = if let Some(action) = local_msg(message) {
                    assert!(
                        state.network(message.clone()).is_err(),
                        "state accepts network message {message:?} while that is a local message"
                    );
                    (state.local(action).ok(), true)
                } else {
                    (
                        state
                            .network(message.clone())
                            .ok()
                            .map(|(outcome, next)| (outcome.send.into(), next)),
                        false,
                    )
                };
                let ((r, to), (send, next)) = match (to, outcome) {
                    (None, None) => continue,
                    (None, Some(_)) => panic!("extraneous transition {:?} -> {:?}", state, message),
                    (Some(_), None) => panic!(
                        "missing transition {:?} -> {:?} for {:?}",
                        state, message, to
                    ),
                    (Some(to), Some(outcome)) => (to, outcome),
                };
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
                    if let Some(send) = send.send {
                        let to2 = self.transitions.get(to).and_then(|m| m.get(&send));
                        if let Some((r2, to2)) = to2 {
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
        let mut simplified = BTreeMap::<S2, BTreeMap<Message, (Role, S2)>>::new();

        for (from, transitions) in &self.transitions {
            let from = surjection(from);
            for (message, (role, to)) in transitions {
                let to = surjection(to);
                let existing_target = simplified
                    .entry(from.clone())
                    .or_default()
                    .insert(message.clone(), (*role, to.clone()));
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
