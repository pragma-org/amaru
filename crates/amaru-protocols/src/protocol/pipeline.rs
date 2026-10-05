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
//! [`drive`] runs one instance and injects [`Internal::Pull`](super::Internal::Pull)
//! when that machine enters remote agency. [`pipelined`] does the same across N
//! instances with send/recv cursors. A local request that arrives while the send
//! cursor is off the switch state is stashed: the latest request replaces the
//! previous one, and a close is sticky. The stash is handed to the send cursor
//! only once that slot is idle again, and only after the receive cursor has
//! already moved off a slot that just returned to the switch state. A sticky
//! close waits until every slot is idle or finished, so it is not written while
//! a range is still in flight. Once that one `ClientDone` has been written the
//! pipeline stays shut: a later range is not sent, and a second close is not
//! written.

use std::{future::Future, num::NonZeroUsize, time::Duration};

use amaru_kernel::NonEmptyBytes;
use amaru_pure_stage::{Effects, SendData, StageRef, define_role_tag, err, typestate::prelude::*};

use super::{Erased, Inputs, Internal, ProtocolId, egress_admission_deadline};
use crate::mux::{HandlerMessage, MuxMessage, Sent};

define_role_tag!(pub ToMux);

/// Mux demand. Sent by an instance in a typestate remainder (`Send<ToMux, WantNext>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WantNext;

amaru_pure_stage::impl_label!(WantNext);

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

    /// One CBOR encoding of `msg`. The deadline is that length. The closure sends those bytes.
    pub(crate) fn call_encoded<T: amaru_kernel::cbor::Encode<()> + 'static>(
        &self,
        msg: &T,
    ) -> (Duration, impl FnOnce(StageRef<Sent>) -> MuxMessage + std::marker::Send + use<T>) {
        let bytes = NonEmptyBytes::encode(msg);
        let timeout = egress_admission_deadline(bytes.len().get());
        let proto = self.proto;
        (timeout, move |reply| MuxMessage::Send(proto, bytes, reply))
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
///
/// `stashed` is the one newer range waiting for an idle send slot.
/// `sticky_close` is a close held until every slot is idle. `closed` is set
/// once that close has been written.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pipelined<S, L> {
    machines: Vec<Option<S>>,
    send: usize,
    recv: usize,
    registered: bool,
    recv_armed: bool,
    stashed: Option<L>,
    sticky_close: Option<L>,
    /// `ClientDone` has been written. One wire protocol, one close.
    closed: bool,
}

impl<S, L> Pipelined<S, L> {
    pub fn new(n: NonZeroUsize, machine: impl FnMut(usize) -> S) -> Self {
        let n = n.get();
        Self {
            machines: (0..n).map(machine).map(Some).collect(),
            send: 0,
            recv: 0,
            registered: false,
            recv_armed: false,
            stashed: None,
            sticky_close: None,
            closed: false,
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

/// Drive a single lock-step instance: pass each mailbox value to `step`, and
/// inject [`Internal::Pull`] when the machine enters remote agency.
pub async fn drive<S, L, F, Fut>(inst: S, mail: Inputs<L>, eff: Effects<Inputs<L>>, step: F) -> S
where
    S: OccupancyOf,
    L: SendData,
    F: Fn(S, Inputs<L>, Effects<Inputs<L>>) -> Fut,
    Fut: Future<Output = S>,
{
    if matches!(mail, Inputs::Internal(Internal::Pull)) {
        err("pipeline")("Pull is injected by the pipeline driver, not received from the mailbox").await;
        return eff.terminate().await;
    }
    let before = inst.occupancy();
    let inst = step(inst, mail, eff.clone()).await;
    if before.is_switch() && inst.occupancy().is_remote() {
        step(inst, Inputs::Internal(Internal::Pull), eff).await
    } else {
        inst
    }
}

/// Drive one mailbox value through the cursor mux, calling `step` on the
/// selected instance. `step` is the lock-step machine; this function does not
/// send on the mux.
///
/// `is_sticky_close` picks the local message that must outlive a newer range.
/// Any other local message is the one stashed range: a later one replaces it.
pub async fn pipelined<S, L, F, Fut, C>(
    mut p: Pipelined<S, L>,
    mail: Inputs<L>,
    eff: Effects<Inputs<L>>,
    step: F,
    is_sticky_close: C,
) -> Pipelined<S, L>
where
    S: OccupancyOf,
    L: SendData,
    F: Fn(S, Inputs<L>, Effects<Inputs<L>>) -> Fut,
    Fut: Future<Output = S>,
    C: Fn(&L) -> bool,
{
    match mail {
        Inputs::Network(HandlerMessage::Registered(_)) => {
            p.registered = true;
        }
        Inputs::Network(HandlerMessage::FromNetwork(_)) => {
            let i = p.recv;
            let before = p.machine(i).occupancy();
            let inst = step(p.take(i), mail, eff.clone()).await;
            p.put(i, inst);
            after_network(&mut p, i, before);
        }
        Inputs::Internal(Internal::Timeout) => {
            let i = p.recv;
            let before = p.machine(i).occupancy();
            let inst = step(p.take(i), mail, eff.clone()).await;
            p.put(i, inst);
            after_network(&mut p, i, before);
        }
        Inputs::Internal(Internal::Pull) => {
            err("pipeline")("Pull is injected by the pipeline driver, not received from the mailbox").await;
            return eff.terminate().await;
        }
        Inputs::Local(msg) => {
            if p.closed {
                // Agency was already given up. A later range is not a new request.
            } else if is_sticky_close(&msg) {
                // Close wins over a range that has not been sent yet.
                p.sticky_close = Some(msg);
                p.stashed = None;
            } else if p.sticky_close.is_some() {
                // A close is already waiting. A later range is not sent.
            } else if !p.machine(p.send).in_switch() {
                p.stashed = Some(msg);
            } else {
                deliver_local(&mut p, msg, &eff, &step).await;
            }
        }
    }
    // Move the receive cursor before offering the stashed range, so the range
    // that just finished is not the slot that receives the next body.
    arm_recv(&mut p, &eff, &step).await;
    flush_waiting(&mut p, &eff, &step).await;
    arm_recv(&mut p, &eff, &step).await;
    p
}

fn after_send<S, L>(p: &mut Pipelined<S, L>, i: usize, before: Occupancy)
where
    S: OccupancyOf,
{
    let after = p.machine(i).occupancy();
    if before.is_switch() && !after.is_switch() {
        p.send = (p.send + 1) % p.n();
    }
    if i == p.recv && before.is_switch() && after.is_remote() {
        p.recv_armed = false;
    }
}

fn after_network<S, L>(p: &mut Pipelined<S, L>, i: usize, before: Occupancy)
where
    S: OccupancyOf,
{
    let after = p.machine(i).occupancy();
    if i == p.recv && !before.is_switch() && after.is_switch() {
        p.recv = (p.recv + 1) % p.n();
        p.recv_armed = false;
    }
}

async fn arm_recv<S, L, F, Fut>(p: &mut Pipelined<S, L>, eff: &Effects<Inputs<L>>, step: &F)
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

fn quiescent<S: OccupancyOf, L>(p: &Pipelined<S, L>) -> bool {
    (0..p.n()).all(|i| {
        let occupancy = p.machine(i).occupancy();
        occupancy.is_switch() || occupancy.is_terminal()
    })
}

async fn deliver_local<S, L, F, Fut>(p: &mut Pipelined<S, L>, msg: L, eff: &Effects<Inputs<L>>, step: &F)
where
    S: OccupancyOf,
    L: SendData,
    F: Fn(S, Inputs<L>, Effects<Inputs<L>>) -> Fut,
    Fut: Future<Output = S>,
{
    let i = p.send;
    let before = p.machine(i).occupancy();
    let inst = step(p.take(i), Inputs::Local(msg), eff.clone()).await;
    p.put(i, inst);
    after_send(p, i, before);
}

/// Offer one waiting local message when the send cursor is idle.
///
/// A slot that stays idle because the mux never admitted the range
/// (`NotAdmitted`) is the send cursor too, so the same path runs after that
/// attempt. One message is offered per turn: a range the mux did not admit is
/// not put back, and a close is not retried here.
async fn flush_waiting<S, L, F, Fut>(p: &mut Pipelined<S, L>, eff: &Effects<Inputs<L>>, step: &F)
where
    S: OccupancyOf,
    L: SendData,
    F: Fn(S, Inputs<L>, Effects<Inputs<L>>) -> Fut,
    Fut: Future<Output = S>,
{
    if p.closed || !p.machine(p.send).in_switch() {
        return;
    }
    if p.sticky_close.is_some() {
        // `ClientDone` is legal from Idle. Hold it while any other slot is
        // still in remote agency so it is not written ahead of that body.
        if !quiescent(p) {
            return;
        }
        let Some(close) = p.sticky_close.take() else {
            return;
        };
        p.stashed = None;
        deliver_local(p, close, eff, step).await;
        // The send cursor has moved on to another idle slot. Leave the
        // pipeline shut so that slot cannot start a range after `ClientDone`.
        p.closed = true;
        return;
    }
    let Some(fetch) = p.stashed.take() else {
        return;
    };
    deliver_local(p, fetch, eff, step).await;
}

#[cfg(test)]
mod tests {
    use super::Pipelined;

    #[test]
    fn pipelined_round_trips() {
        let state = Pipelined {
            machines: vec![Some(1u8), None],
            send: 1,
            recv: 0,
            registered: true,
            recv_armed: true,
            stashed: Some(4u8),
            sticky_close: Some(9u8),
            closed: true,
        };
        let bytes = amaru_pure_stage::serde::to_cbor(&state);
        let back: Pipelined<u8, u8> = amaru_pure_stage::serde::from_cbor(&bytes).expect("cbor");
        assert_eq!(back, state);
    }
}
