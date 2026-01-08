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

    pub fn check(
        &self,
        initial: State,
        role: Role,
        local_msg: impl Fn(&Message) -> Option<State::Action>,
        basic_msg: impl Fn(&Message) -> Message,
    ) {
        let states = self.transitions.keys().collect::<Vec<_>>();
        let messages = self
            .transitions
            .values()
            .flat_map(|m| m.keys())
            .collect::<BTreeSet<_>>();

        let (out, init) = initial.init().unwrap();
        match role {
            Role::Initiator => {
                assert!(out.send.is_some() || out.result.is_some());
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
                        let send = basic_msg(&send);
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
                simplified
                    .entry(from.clone())
                    .or_default()
                    .insert(message.clone(), (*role, to.clone()));
            }
        }

        assert_eq!(simplified, other.transitions);
    }
}
