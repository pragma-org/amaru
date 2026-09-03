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

//! CIP-0164 pipelining as a cursor multiplexer over lock-step instances.
//!
//! Each instance is a complete mini-protocol machine (including mux sends).
//! This module only picks which machine sees the next mailbox value, injects
//! [`Internal::Pull`](super::Internal::Pull) when the recv cursor lands on a
//! remote-agency instance, and treats a local request while the send cursor is
//! off the switch state as an error.

use std::{future::Future, num::NonZeroUsize};

use amaru_kernel::NonEmptyBytes;
use amaru_pure_stage::{Effects, SendData, StageRef, define_role_tag, err, typestate::prelude::*};

use super::{Erased, Inputs, Internal, ProtocolId};
use crate::mux::{HandlerMessage, MuxMessage};

define_role_tag!(pub ToMux);

/// Mux demand. Sent by an instance in a typestate remainder (`Send<ToMux, WantNext>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WantNext;

/// Destination for instance mux I/O. Holds the protocol id so [`WantNext`] and
/// wire payloads can become [`MuxMessage`] values.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MuxClient {
    muxer: StageRef<MuxMessage>,
    proto: ProtocolId<Erased>,
}

impl MuxClient {
    pub fn new(muxer: StageRef<MuxMessage>, proto: ProtocolId<Erased>) -> Self {
        Self { muxer, proto }
    }

    pub(crate) fn encode_send<T: amaru_kernel::cbor::Encode<()>>(&self, msg: T) -> MuxMessage {
        MuxMessage::Send(self.proto, NonEmptyBytes::encode(&msg), StageRef::blackhole())
    }
}

impl<Tag: RoleTag> Role<Tag> for MuxClient {
    type Mailbox = MuxMessage;

    fn mailbox(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl IntoRoleMail<ToMux, WantNext> for MuxClient {
    fn encode(&self, _: WantNext) -> MuxMessage {
        MuxMessage::WantNext(self.proto)
    }
}

/// N lock-step machines plus send/recv cursors.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pipelined<S> {
    machines: Vec<Option<S>>,
    send: usize,
    recv: usize,
    registered: bool,
    recv_armed: bool,
}

impl<S> Pipelined<S> {
    pub fn new(n: NonZeroUsize, machine: impl FnMut(usize) -> S) -> Self {
        let n = n.get();
        Self {
            machines: (0..n).map(machine).map(Some).collect(),
            send: 0,
            recv: 0,
            registered: false,
            recv_armed: false,
        }
    }

    fn n(&self) -> usize {
        self.machines.len()
    }

    fn machine(&self, i: usize) -> &S {
        #[expect(clippy::expect_used)]
        self.machines[i].as_ref().expect("pipeline slot empty")
    }

    fn take(&mut self, i: usize) -> S {
        #[expect(clippy::expect_used)]
        self.machines[i].take().expect("pipeline slot empty")
    }

    fn put(&mut self, i: usize, machine: S) {
        debug_assert!(self.machines[i].is_none());
        self.machines[i] = Some(machine);
    }
}

/// Drive one mailbox value through the cursor mux, calling `step` on the
/// selected instance. `step` is the lock-step machine; this function does not
/// send on the mux.
pub async fn pipelined<S, L, F, Fut>(
    mut p: Pipelined<S>,
    mail: Inputs<L>,
    eff: Effects<Inputs<L>>,
    step: F,
) -> Pipelined<S>
where
    S: OccupancyOf,
    L: SendData,
    F: Fn(S, Inputs<L>, Effects<Inputs<L>>) -> Fut,
    Fut: Future<Output = S>,
{
    match mail {
        Inputs::Network(HandlerMessage::Registered(_)) => {
            p.registered = true;
            arm_recv(&mut p, &eff, &step).await;
        }
        Inputs::Network(HandlerMessage::FromNetwork(_)) => {
            let i = p.recv;
            let before = p.machine(i).occupancy();
            let inst = step(p.take(i), mail, eff.clone()).await;
            p.put(i, inst);
            after_network(&mut p, i, before);
            arm_recv(&mut p, &eff, &step).await;
        }
        Inputs::Internal(Internal::Timeout) => {
            let i = p.recv;
            let inst = step(p.take(i), mail, eff.clone()).await;
            p.put(i, inst);
        }
        Inputs::Internal(Internal::Pull) => {
            err("pipeline")("Pull is injected by the pipeline driver, not received from the mailbox").await;
            return eff.terminate().await;
        }
        Inputs::Local(_) => {
            let i = p.send;
            if !p.machine(i).in_switch() {
                err("pipeline")("pipeline full: local request while send cursor is not in switch state").await;
                return eff.terminate().await;
            }
            let before = p.machine(i).occupancy();
            let inst = step(p.take(i), mail, eff.clone()).await;
            p.put(i, inst);
            after_send(&mut p, i, before);
            arm_recv(&mut p, &eff, &step).await;
        }
    }
    p
}

fn after_send<S: OccupancyOf>(p: &mut Pipelined<S>, i: usize, before: Occupancy) {
    let after = p.machine(i).occupancy();
    if before.is_switch() && !after.is_switch() {
        p.send = (p.send + 1) % p.n();
    }
    if i == p.recv && before.is_switch() && after.is_remote() {
        p.recv_armed = false;
    }
}

fn after_network<S: OccupancyOf>(p: &mut Pipelined<S>, i: usize, before: Occupancy) {
    let after = p.machine(i).occupancy();
    if i == p.recv && !before.is_switch() && after.is_switch() {
        p.recv = (p.recv + 1) % p.n();
        p.recv_armed = false;
    }
}

async fn arm_recv<S, L, F, Fut>(p: &mut Pipelined<S>, eff: &Effects<Inputs<L>>, step: &F)
where
    S: OccupancyOf,
    L: SendData,
    F: Fn(S, Inputs<L>, Effects<Inputs<L>>) -> Fut,
    Fut: Future<Output = S>,
{
    if !p.registered || p.recv_armed || !p.machine(p.recv).is_remote() {
        return;
    }
    let i = p.recv;
    let inst = step(p.take(i), Inputs::Internal(Internal::Pull), eff.clone()).await;
    p.put(i, inst);
    p.recv_armed = true;
}
