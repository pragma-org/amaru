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

//! Fused CIP-0164 BlockFetch initiator: one handler stage, N typestate instances.
//!
//! Remainders are the pipelined initiator only. This sits beside
//! [`ProtocolState`](crate::protocol::ProtocolState) / [`ProtoSpec`](crate::protocol::ProtoSpec)
//! and does not replace them.

use std::{num::NonZeroUsize, time::Duration};

use amaru_kernel::{NetworkPoint, NonEmptyBytes, Peer, RawBlock, cardano::network_block::NetworkBlock, cbor};
use amaru_pure_stage::{
    DeserializerGuards, Effects, StageRef, define_role, define_role_tag, err, make_states, on_receive,
    typestate::prelude::*,
};

use super::{Blocks, Message, responder::MAX_FETCHED_BLOCKS};
use crate::{
    mux::{Frame, HandlerMessage, MuxMessage, Sent},
    protocol::{
        Admit, CloseHint, CursorHint, Inputs, NETWORK_SEND_TIMEOUT, PROTO_N2N_BLOCK_FETCH, Pipeline, SwitchCredit,
    },
};

make_states!(pub Live { Idle; Busy, Streaming, Done });

define_role_tag!(pub ToResponder);
define_role_tag!(pub ToCollector);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequestRange {
    pub from: NetworkPoint,
    pub through: NetworkPoint,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClientDone;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StartBatch;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NoBlocks;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchDone;

impl From<RequestRange> for Message {
    fn from(value: RequestRange) -> Self {
        Message::RequestRange { from: value.from, through: value.through }
    }
}

impl From<ClientDone> for Message {
    fn from(_: ClientDone) -> Self {
        Message::ClientDone
    }
}

impl From<StartBatch> for Message {
    fn from(_: StartBatch) -> Self {
        Message::StartBatch
    }
}

impl From<NoBlocks> for Message {
    fn from(_: NoBlocks) -> Self {
        Message::NoBlocks
    }
}

impl From<Block> for Message {
    fn from(value: Block) -> Self {
        Message::Block { body: value.body }
    }
}

impl From<BatchDone> for Message {
    fn from(_: BatchDone) -> Self {
        Message::BatchDone
    }
}

pub const BLOCKFETCH_PIPELINE_N: NonZeroUsize = match NonZeroUsize::new(2) {
    Some(n) => n,
    None => unreachable!(),
};

/// Receive timeout while the responder has agency (`StBusy` / `StStreaming`).
///
/// From the Cardano Blueprint networking notes: `StIdle` has no receive timeout;
/// `StBusy` and `StStreaming` wait at most 60 seconds.
pub const BLOCKFETCH_AGENCY_TIMEOUT: Duration = Duration::from_secs(60);

pub const BLOCKFETCH_MAX_BLOCK_WIRE_BYTES: usize = 96 * 1024;

pub fn blockfetch_pipeline_max_buffer(n: NonZeroUsize) -> usize {
    n.get().saturating_mul(MAX_FETCHED_BLOCKS).saturating_mul(BLOCKFETCH_MAX_BLOCK_WIRE_BYTES)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum WireMail {
    Range(NetworkPoint, NetworkPoint),
    Done,
}

impl From<RequestRange> for WireMail {
    fn from(value: RequestRange) -> Self {
        Self::Range(value.from, value.through)
    }
}

impl From<ClientDone> for WireMail {
    fn from(_: ClientDone) -> Self {
        Self::Done
    }
}

define_role!(WireOut, ToResponder, WireMail);
define_role!(CollectorOut, ToCollector, Blocks);

/// Local request that starts an initiator fetch on one pipeline instance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fetch {
    pub from: NetworkPoint,
    pub through: NetworkPoint,
    pub id: u64,
    pub cr: StageRef<Blocks>,
}

/// Local request that closes an idle initiator instance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Close;

on_receive!(Idle as PipelineIdleIn {
    Fetch => { Send<ToResponder, RequestRange>, SetTimeout => Busy }
    Close => { Send<ToResponder, ClientDone> | Repeat<SendAny<ToCollector>> => Done }
});
on_receive!(Busy as ClientBusyIn {
    StartBatch => { SetTimeout => Streaming }
    NoBlocks => { ClearTimeout, Repeat<SendAny<ToCollector>> => Idle }
});
on_receive!(Streaming as ClientStreamingIn {
    Block => { Repeat<SendAny<ToCollector>>, SetTimeout => Streaming }
    BatchDone => { ClearTimeout, Repeat<SendAny<ToCollector>> => Idle }
});

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<Handler>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Instance>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Fetch>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Close>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Pipeline<Fetch>>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Inflight>().boxed(),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Inflight {
    id: u64,
    cr: StageRef<Blocks>,
    remaining: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Instance {
    idx: usize,
    live: Live,
    inflight: Option<Inflight>,
    peer: Peer,
}

struct Step {
    wire: Option<Message>,
    credit: Option<SwitchCredit>,
}

type Mail = Inputs<super::BlockFetchMessage>;

impl Instance {
    fn new(idx: usize, peer: Peer) -> Self {
        Self { idx, live: initial_state::<Idle>().into(), inflight: None, peer }
    }

    fn slot(&self) -> u64 {
        self.idx as u64
    }

    fn timeout_mail(&self) -> Mail {
        Inputs::Local(super::BlockFetchMessage::Timeout { slot: self.slot() })
    }

    fn waiting(&self) -> bool {
        matches!(self.live, Live::Busy(_) | Live::Streaming(_))
    }

    async fn on_fetch(&mut self, fetch: Fetch, eff: &Effects<Mail>) -> anyhow::Result<Step> {
        let Some(idle) = take_idle(&mut self.live) else {
            anyhow::bail!("fetch while instance is not Idle");
        };
        let range = RequestRange { from: fetch.from, through: fetch.through };
        self.inflight = Some(Inflight { id: fetch.id, cr: fetch.cr.clone(), remaining: MAX_FETCHED_BLOCKS });
        let dest = WireOut::new(StageRef::blackhole());
        let session = idle
            .receive(fetch, eff.clone())
            .send(&dest, range.clone())
            .await
            .set_timeout_at(self.slot(), BLOCKFETCH_AGENCY_TIMEOUT, self.timeout_mail())
            .await;
        self.live = session.finish().into();
        Ok(Step { wire: Some(range.into()), credit: Some(SwitchCredit::Left) })
    }

    async fn on_close(&mut self, eff: &Effects<Mail>) -> anyhow::Result<Step> {
        let Some(idle) = take_idle(&mut self.live) else {
            anyhow::bail!("close while instance is not Idle");
        };
        let dest = WireOut::new(StageRef::blackhole());
        let session = idle.receive(Close, eff.clone()).send(&dest, ClientDone).await;
        self.live = session.finish().into();
        self.inflight = None;
        Ok(Step { wire: Some(Message::ClientDone), credit: Some(SwitchCredit::Terminated) })
    }

    async fn on_network(&mut self, msg: Message, eff: &Effects<Mail>) -> anyhow::Result<Step> {
        match msg {
            Message::StartBatch => {
                let Some(busy) = take_busy(&mut self.live) else {
                    anyhow::bail!("StartBatch while instance is not Busy");
                };
                self.live = busy
                    .receive(StartBatch, eff.clone())
                    .set_timeout_at(self.slot(), BLOCKFETCH_AGENCY_TIMEOUT, self.timeout_mail())
                    .await
                    .finish()
                    .into();
                Ok(Step { wire: None, credit: Some(SwitchCredit::Stay) })
            }
            Message::NoBlocks => {
                let Some(busy) = take_busy(&mut self.live) else {
                    anyhow::bail!("NoBlocks while instance is not Busy");
                };
                let Some(inflight) = self.inflight.take() else {
                    anyhow::bail!("NoBlocks without inflight request");
                };
                let collector = CollectorOut::new(inflight.cr);
                let session = busy
                    .receive(NoBlocks, eff.clone())
                    .clear_timeout_at(self.slot())
                    .await
                    .send_any(&collector, Blocks::NoBlocks(inflight.id, self.peer))
                    .await;
                self.live = session.finish().into();
                Ok(Step { wire: None, credit: Some(SwitchCredit::Entered) })
            }
            Message::Block { body } => {
                let Some(st) = take_streaming(&mut self.live) else {
                    anyhow::bail!("Block while instance is not Streaming");
                };
                let Some(inflight) = self.inflight.as_mut() else {
                    anyhow::bail!("Block without inflight request");
                };
                if inflight.remaining == 0 {
                    anyhow::bail!("received more blocks than allowed for a single request");
                }
                let network_block = NetworkBlock::try_from(RawBlock::from(body.as_slice()))
                    .map_err(|_| anyhow::anyhow!("received invalid block CBOR"))?;
                inflight.remaining -= 1;
                let collector = CollectorOut::new(inflight.cr.clone());
                let id = inflight.id;
                let session = st
                    .receive(Block { body }, eff.clone())
                    .send_any(&collector, Blocks::Block(id, self.peer, network_block))
                    .await
                    .set_timeout_at(self.slot(), BLOCKFETCH_AGENCY_TIMEOUT, self.timeout_mail())
                    .await;
                self.live = session.finish().into();
                Ok(Step { wire: None, credit: Some(SwitchCredit::Stay) })
            }
            Message::BatchDone => {
                let Some(st) = take_streaming(&mut self.live) else {
                    anyhow::bail!("BatchDone while instance is not Streaming");
                };
                let Some(inflight) = self.inflight.take() else {
                    anyhow::bail!("BatchDone without inflight request");
                };
                let collector = CollectorOut::new(inflight.cr);
                let session = st
                    .receive(BatchDone, eff.clone())
                    .clear_timeout_at(self.slot())
                    .await
                    .send_any(&collector, Blocks::Done(inflight.id))
                    .await;
                self.live = session.finish().into();
                Ok(Step { wire: None, credit: Some(SwitchCredit::Entered) })
            }
            Message::RequestRange { .. } | Message::ClientDone => {
                anyhow::bail!("initiator variant")
            }
        }
    }
}

fn take_idle(live: &mut Live) -> Option<Idle> {
    if !matches!(live, Live::Idle(_)) {
        return None;
    }
    match std::mem::replace(live, Live::Idle(initial_state())) {
        Live::Idle(idle) => Some(idle),
        other @ (Live::Busy(_) | Live::Streaming(_) | Live::Done(_)) => {
            *live = other;
            None
        }
    }
}

fn take_busy(live: &mut Live) -> Option<Busy> {
    if !matches!(live, Live::Busy(_)) {
        return None;
    }
    match std::mem::replace(live, Live::Idle(initial_state())) {
        Live::Busy(busy) => Some(busy),
        other @ (Live::Idle(_) | Live::Streaming(_) | Live::Done(_)) => {
            *live = other;
            None
        }
    }
}

fn take_streaming(live: &mut Live) -> Option<Streaming> {
    if !matches!(live, Live::Streaming(_)) {
        return None;
    }
    match std::mem::replace(live, Live::Idle(initial_state())) {
        Live::Streaming(st) => Some(st),
        other @ (Live::Idle(_) | Live::Busy(_) | Live::Done(_)) => {
            *live = other;
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Handler {
    pipeline: Pipeline<Fetch>,
    instances: Vec<Instance>,
    muxer: StageRef<MuxMessage>,
}

impl Handler {
    fn new(n: NonZeroUsize, muxer: StageRef<MuxMessage>, peer: Peer) -> Self {
        Self { pipeline: Pipeline::new(n), instances: (0..n.get()).map(|i| Instance::new(i, peer)).collect(), muxer }
    }
}

async fn handler(
    mut state: Handler,
    msg: Inputs<super::BlockFetchMessage>,
    eff: Effects<Inputs<super::BlockFetchMessage>>,
) -> Handler {
    match msg {
        Inputs::Local(super::BlockFetchMessage::RequestRange { from, through, id, cr }) => {
            let fetch = Fetch { from: from.to_network_point(), through: through.to_network_point(), id, cr };
            admit_fetch(&mut state, fetch, &eff).await;
        }
        Inputs::Local(super::BlockFetchMessage::Close) => {
            apply_close(&mut state, &eff).await;
        }
        Inputs::Local(super::BlockFetchMessage::Timeout { slot }) => {
            if agency_timeout_waiting(&state, slot) {
                err("blockfetch agency timeout")(anyhow::anyhow!("blockfetch agency timeout")).await;
                return eff.terminate().await;
            }
        }
        Inputs::Network(HandlerMessage::Registered(_)) => {
            state.pipeline.mark_registered();
        }
        Inputs::Network(HandlerMessage::FromNetwork(bytes)) => {
            on_from_network(&mut state, bytes, &eff).await;
        }
    }
    state
}

fn agency_timeout_waiting(state: &Handler, slot: u64) -> bool {
    state.instances.get(slot as usize).is_some_and(Instance::waiting)
}

async fn admit_fetch(state: &mut Handler, fetch: Fetch, eff: &Effects<Inputs<super::BlockFetchMessage>>) {
    match state.pipeline.try_admit(fetch) {
        Admit::Instance(i, fetch) => run_fetch(state, i, fetch, eff).await,
        Admit::Slack => {}
        Admit::ReplacedSlack(_) => {}
        Admit::Dropped => {}
    }
}

async fn run_fetch(state: &mut Handler, i: usize, fetch: Fetch, eff: &Effects<Inputs<super::BlockFetchMessage>>) {
    let step = match state.instances[i].on_fetch(fetch, eff).await {
        Ok(step) => step,
        Err(e) => {
            err("blockfetch instance fetch")(e).await;
            return eff.terminate().await;
        }
    };
    if let Some(wire) = step.wire {
        send_wire(state, wire, eff).await;
    }
    if let Some(credit) = step.credit {
        apply_credit(state, i, credit, eff).await;
        drain_followups(state, eff).await;
    }
}

async fn run_close(state: &mut Handler, i: usize, eff: &Effects<Inputs<super::BlockFetchMessage>>) {
    let step = match state.instances[i].on_close(eff).await {
        Ok(step) => step,
        Err(e) => {
            err("blockfetch instance close")(e).await;
            return eff.terminate().await;
        }
    };
    if let Some(wire) = step.wire {
        send_wire(state, wire, eff).await;
    }
    if let Some(credit) = step.credit {
        apply_credit(state, i, credit, eff).await;
        drain_followups(state, eff).await;
    }
}

async fn apply_close(state: &mut Handler, eff: &Effects<Inputs<super::BlockFetchMessage>>) {
    match state.pipeline.on_close() {
        CloseHint::Inject(i) => run_close(state, i, eff).await,
        CloseHint::Drain | CloseHint::Already => {}
    }
}

async fn on_from_network(state: &mut Handler, bytes: NonEmptyBytes, eff: &Effects<Inputs<super::BlockFetchMessage>>) {
    state.pipeline.mark_want_consumed();
    let msg: Message = match cbor::decode(bytes.as_ref()) {
        Ok(msg) => msg,
        Err(e) => {
            err("decode")(e).await;
            return eff.terminate().await;
        }
    };
    if matches!(msg, Message::RequestRange { .. } | Message::ClientDone) {
        err("initiator variant")(anyhow::anyhow!("initiator variant")).await;
        return eff.terminate().await;
    }
    let i = state.pipeline.recv_idx();
    let step = match state.instances[i].on_network(msg, eff).await {
        Ok(step) => step,
        Err(e) => {
            err("blockfetch instance network")(e).await;
            return eff.terminate().await;
        }
    };
    if let Some(credit) = step.credit {
        apply_credit(state, i, credit, eff).await;
        drain_followups(state, eff).await;
    }
}

async fn apply_credit(
    state: &mut Handler,
    i: usize,
    credit: SwitchCredit,
    eff: &Effects<Inputs<super::BlockFetchMessage>>,
) {
    match state.pipeline.on_credit(i, credit) {
        Ok(CursorHint::WantNext) => issue_want_next(state, eff).await,
        Ok(CursorHint::None) => {}
        Err(e) => {
            err("credit mismatch")(e).await;
            return eff.terminate().await;
        }
    }
}

async fn drain_followups(state: &mut Handler, eff: &Effects<Inputs<super::BlockFetchMessage>>) {
    while let Some(fetch) = state.pipeline.take_slack_if_ready() {
        match state.pipeline.try_admit(fetch) {
            Admit::Instance(j, fetch) => {
                let step = match state.instances[j].on_fetch(fetch, eff).await {
                    Ok(step) => step,
                    Err(e) => {
                        err("blockfetch instance fetch")(e).await;
                        return eff.terminate().await;
                    }
                };
                if let Some(wire) = step.wire {
                    send_wire(state, wire, eff).await;
                }
                if let Some(credit) = step.credit {
                    apply_credit(state, j, credit, eff).await;
                }
            }
            Admit::Slack | Admit::ReplacedSlack(_) | Admit::Dropped => break,
        }
    }
    if let CloseHint::Inject(j) = state.pipeline.try_inject_close() {
        let step = match state.instances[j].on_close(eff).await {
            Ok(step) => step,
            Err(e) => {
                err("blockfetch instance close")(e).await;
                return eff.terminate().await;
            }
        };
        if let Some(wire) = step.wire {
            send_wire(state, wire, eff).await;
        }
        if let Some(credit) = step.credit {
            apply_credit(state, j, credit, eff).await;
        }
    }
}

async fn send_wire(state: &Handler, wire: Message, eff: &Effects<Inputs<super::BlockFetchMessage>>) {
    let bytes = NonEmptyBytes::encode(&wire);
    let proto = PROTO_N2N_BLOCK_FETCH.erase();
    let sent: Option<Sent> =
        eff.call(&state.muxer, NETWORK_SEND_TIMEOUT, move |cr| MuxMessage::Send(proto, bytes, cr)).await;
    if sent.is_none() {
        err("network send timeout")(anyhow::anyhow!("network send timeout")).await;
        return eff.terminate().await;
    }
}

async fn issue_want_next(state: &mut Handler, eff: &Effects<Inputs<super::BlockFetchMessage>>) {
    if !state.pipeline.should_want_next() {
        return;
    }
    eff.send(&state.muxer, MuxMessage::WantNext(PROTO_N2N_BLOCK_FETCH.erase())).await;
    state.pipeline.mark_want_sent();
}

pub async fn register_blockfetch_initiator_pipelined<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    n: NonZeroUsize,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<super::BlockFetchMessage> {
    let blockfetch = eff.stage("blockfetch-pipelined", handler).await;
    let blockfetch = eff.supervise(blockfetch, tombstone);
    let blockfetch = eff.wire_up(blockfetch, Handler::new(n, muxer.clone(), peer)).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_BLOCK_FETCH.erase(),
            frame: Frame::OneCborItem,
            handler: blockfetch.contramap(Inputs::Network),
            max_buffer: blockfetch_pipeline_max_buffer(n),
        },
    )
    .await;
    blockfetch.contramap(Inputs::Local)
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use amaru_kernel::Point;
    use amaru_pure_stage::{
        StageGraph,
        simulation::{Run, SimulationBuilder},
        typestate::{FmtPar, OnReceive, Session},
    };
    use tokio::runtime::{Builder, Runtime};

    use super::*;
    use crate::{blockfetch::BlockFetchMessage, mux::MuxMessage, protocol::Inputs};

    fn remaining<S, In>() -> String
    where
        S: OnReceive<In>,
        S::Then: FmtPar,
    {
        Session::<(), S::Then>::describe()
    }

    fn send_desc<Tag, T>() -> String {
        format!("Send<{}, {}>", std::any::type_name::<Tag>(), std::any::type_name::<T>())
    }

    fn star_any<Tag>() -> String {
        format!("Repeat<SendAny<{}>>", std::any::type_name::<Tag>())
    }

    #[test]
    fn initiator_receive_allowances() {
        assert_eq!(
            remaining::<Idle, Fetch>(),
            format!("{}, SetTimeout => Busy", send_desc::<ToResponder, RequestRange>())
        );
        assert_eq!(
            remaining::<Idle, Close>(),
            format!("{} | {} => Done", send_desc::<ToResponder, ClientDone>(), star_any::<ToCollector>())
        );
        assert_eq!(remaining::<Busy, StartBatch>(), "SetTimeout => Streaming");
        assert_eq!(remaining::<Busy, NoBlocks>(), format!("ClearTimeout, {} => Idle", star_any::<ToCollector>()));
        assert_eq!(remaining::<Streaming, Block>(), format!("{}, SetTimeout => Streaming", star_any::<ToCollector>()));
        assert_eq!(remaining::<Streaming, BatchDone>(), format!("ClearTimeout, {} => Idle", star_any::<ToCollector>()));
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
    struct MuxLog {
        sends: Vec<String>,
        wants: usize,
    }

    async fn mux_step(mut log: MuxLog, msg: MuxMessage, eff: Effects<MuxMessage>) -> MuxLog {
        match msg {
            MuxMessage::Send(_, bytes, cr) => {
                let decoded: Message = cbor::decode(bytes.as_ref()).expect("cbor");
                log.sends.push(decoded.message_type().to_string());
                eff.send(&cr, Sent).await;
            }
            MuxMessage::WantNext(_) => {
                log.wants += 1;
            }
            MuxMessage::Register { .. }
            | MuxMessage::Buffer(..)
            | MuxMessage::FromNetwork(..)
            | MuxMessage::Written
            | MuxMessage::Terminate => {}
        }
        log
    }

    fn test_runtime() -> &'static tokio::runtime::Handle {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| Builder::new_multi_thread().enable_all().build().unwrap()).handle()
    }

    #[test]
    fn two_ranges_pair_in_order() {
        let mut network = SimulationBuilder::default();
        let out = network.stage("out", async |mut inbox: Vec<Blocks>, msg: Blocks, _eff| {
            inbox.push(msg);
            inbox
        });
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let out = network.wire_up(out, Vec::new());
        let mux = network.wire_up(mux, MuxLog::default());

        let handler_b = network.stage("bf", handler);
        let handler = network.wire_up(handler_b, Handler::new(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));

        let cr = (*out).clone();
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 1,
                        cr: cr.clone(),
                    }),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 2,
                        cr,
                    }),
                ],
            )
            .unwrap();

        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();

        let log = running.get_state(&mux).cloned().unwrap();
        assert_eq!(log.sends, vec!["RequestRange", "RequestRange"]);
        assert_eq!(log.wants, 1);

        running.enqueue_msg(
            &handler,
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::NoBlocks)))],
        );
        running.run(Run::default()).assert_sleeping();
        running.enqueue_msg(
            &handler,
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::NoBlocks)))],
        );
        running.run(Run::skip_wakeups()).assert_idle();

        let collected = running.get_state(&out).cloned().unwrap();
        assert_eq!(
            collected,
            vec![Blocks::NoBlocks(1, Peer::for_test(3001)), Blocks::NoBlocks(2, Peer::for_test(3001))]
        );
    }

    #[test]
    fn single_range_does_not_want_next_after_idle() {
        let mut network = SimulationBuilder::default();
        let out = network.stage("out", async |mut inbox: Vec<Blocks>, msg: Blocks, _eff| {
            inbox.push(msg);
            inbox
        });
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let out = network.wire_up(out, Vec::new());
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", handler);
        let handler = network.wire_up(handler_b, Handler::new(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 1,
                        cr: (*out).clone(),
                    }),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();
        assert_eq!(running.get_state(&mux).unwrap().wants, 1);
        running.enqueue_msg(
            &handler,
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::NoBlocks)))],
        );
        running.run(Run::skip_wakeups()).assert_idle();
        assert_eq!(running.get_state(&mux).unwrap().wants, 1);
        assert_eq!(running.get_state(&out).cloned().unwrap(), vec![Blocks::NoBlocks(1, Peer::for_test(3001))]);
    }

    #[test]
    fn close_idle_sends_one_client_done() {
        let mut network = SimulationBuilder::default();
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", handler);
        let handler = network.wire_up(handler_b, Handler::new(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(BlockFetchMessage::Close),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::skip_wakeups()).assert_idle();
        let log = running.get_state(&mux).cloned().unwrap();
        assert_eq!(log.sends, vec!["ClientDone"]);
        assert_eq!(log.wants, 0);
    }

    #[test]
    fn close_drops_slack_and_does_not_send_second_range() {
        let mut network = SimulationBuilder::default();
        let out = network.stage("out", async |mut inbox: Vec<Blocks>, msg: Blocks, _eff| {
            inbox.push(msg);
            inbox
        });
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let out = network.wire_up(out, Vec::new());
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", handler);
        let handler = network.wire_up(handler_b, Handler::new(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        let cr = (*out).clone();
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 1,
                        cr: cr.clone(),
                    }),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 2,
                        cr: cr.clone(),
                    }),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 3,
                        cr,
                    }),
                    Inputs::Local(BlockFetchMessage::Close),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange"]);
        running.enqueue_msg(
            &handler,
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::NoBlocks)))],
        );
        running.run(Run::default()).assert_sleeping();
        running.enqueue_msg(
            &handler,
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::NoBlocks)))],
        );
        running.run(Run::skip_wakeups()).assert_idle();
        let log = running.get_state(&mux).cloned().unwrap();
        assert_eq!(log.sends, vec!["RequestRange", "RequestRange", "ClientDone"]);
        assert_eq!(running.get_state(&out).cloned().unwrap().len(), 2);
    }

    #[test]
    fn start_batch_while_idle_terminates() {
        let mut network = SimulationBuilder::default();
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let _mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", handler);
        let handler = network.wire_up(handler_b, Handler::new(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::StartBatch))),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        let blocked = running.run(Run::skip_wakeups());
        assert!(matches!(blocked, amaru_pure_stage::simulation::Blocked::Terminated(_)));
    }

    #[test]
    fn initiator_wire_variant_terminates() {
        let mut network = SimulationBuilder::default();
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let _mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", handler);
        let handler = network.wire_up(handler_b, Handler::new(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::ClientDone))),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        let blocked = running.run(Run::skip_wakeups());
        assert!(matches!(blocked, amaru_pure_stage::simulation::Blocked::Terminated(_)));
    }

    #[test]
    fn busy_timeout_terminates() {
        let mut network = SimulationBuilder::default();
        let out = network.stage("out", async |mut inbox: Vec<Blocks>, msg: Blocks, _eff| {
            inbox.push(msg);
            inbox
        });
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let out = network.wire_up(out, Vec::new());
        let _mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", handler);
        let handler = network.wire_up(handler_b, Handler::new(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 1,
                        cr: (*out).clone(),
                    }),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();
        let blocked = running.run(Run::skip_wakeups());
        assert!(matches!(blocked, amaru_pure_stage::simulation::Blocked::Terminated(_)));
    }

    #[test]
    fn stale_timeout_after_idle_is_ignored() {
        let mut network = SimulationBuilder::default();
        let out = network.stage("out", async |mut inbox: Vec<Blocks>, msg: Blocks, _eff| {
            inbox.push(msg);
            inbox
        });
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let out = network.wire_up(out, Vec::new());
        let _mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", handler);
        let handler = network.wire_up(handler_b, Handler::new(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 1,
                        cr: (*out).clone(),
                    }),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();
        running.enqueue_msg(
            &handler,
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::NoBlocks)))],
        );
        running.run(Run::skip_wakeups()).assert_idle();
        running.enqueue_msg(&handler, [Inputs::Local(BlockFetchMessage::Timeout { slot: 0 })]);
        running.run(Run::skip_wakeups()).assert_idle();
    }
}
