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

//! Typestate BlockFetch initiator.
//!
//! The lock-step `instance` machine is the protocol handler. Registration takes a
//! pipeline depth `N`: `N = 1` drives that instance directly; `N > 1` wraps N copies
//! in the CIP-0164 pipeliner.

use std::{
    num::{NonZeroU8, NonZeroUsize},
    time::Duration,
};

use amaru_kernel::{NetworkPoint, Peer, Point, RawBlock, cardano::network_block::NetworkBlock, utils::debug_bytes};
use amaru_observability::error;
use amaru_pure_stage::{
    DeserializerGuards, Effects, StageRef, define_role, define_role_tag, make_states, on_receive, typestate::prelude::*,
};

use super::{BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch, responder::MAX_FETCHED_BLOCKS};
use crate::{
    mux::{Frame, HandlerMessage, MuxMessage, Sent},
    protocol::{
        Inputs, Internal, MuxClient, NETWORK_SEND_TIMEOUT, PROTO_N2N_BLOCK_FETCH, Pipelined, Pull, ToMux, WantNext,
        drive, from_wire, pipelined,
    },
};

pub const BLOCKFETCH_PIPELINE_N: NonZeroU8 = match NonZeroU8::new(2) {
    Some(n) => n,
    None => unreachable!(),
};

/// Receive timeout while the responder has agency (`StBusy` / `StStreaming`).
///
/// From the Cardano Blueprint networking notes: `StIdle` has no receive timeout;
/// `StBusy` and `StStreaming` wait at most 60 seconds.
pub const BLOCKFETCH_AGENCY_TIMEOUT: Duration = Duration::from_secs(60);

pub const BLOCKFETCH_MAX_BLOCK_WIRE_BYTES: usize = 96 * 1024;

pub fn blockfetch_pipeline_max_buffer(n: NonZeroU8) -> usize {
    usize::from(n.get()).saturating_mul(MAX_FETCHED_BLOCKS).saturating_mul(BLOCKFETCH_MAX_BLOCK_WIRE_BYTES)
}

fn pipeline_slots(n: NonZeroU8) -> NonZeroUsize {
    match NonZeroUsize::new(usize::from(n.get())) {
        Some(n) => n,
        None => unreachable!(),
    }
}

make_states!(pub Proto { Idle; Busy, Streaming, Done } switch Idle, terminal Done);

define_role_tag!(pub ToResponder);
define_role_tag!(pub ToCollector);

define_role!(CollectorOut, ToCollector, Blocks);

on_receive!(Idle as PipelineIdleIn {
    Fetch => { Call<ToResponder, RequestRange> => Busy }
    Close => { Call<ToResponder, ClientDone> | Repeat<SendAny<ToCollector>> => Done }
});
on_receive!(Busy as ClientBusyIn {
    Pull => { Send<ToMux, WantNext>, SetTimeout => Busy }
    StartBatch => { Send<ToMux, WantNext>, SetTimeout => Streaming }
    NoBlocks => { ClearTimeout, Repeat<SendAny<ToCollector>> => Idle }
});
on_receive!(Streaming as ClientStreamingIn {
    Block => { Send<ToMux, WantNext>, Repeat<SendAny<ToCollector>>, SetTimeout => Streaming }
    BatchDone => { ClearTimeout, Repeat<SendAny<ToCollector>> => Idle }
});
on_receive!(Done as DoneIn {});

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

#[derive(PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub enum Blocks {
    /// Peer responded that it has no blocks in the requested range.
    NoBlocks(u64, Peer),
    /// Manager found no initiating connections for this request id.
    NoPeersAvailable(u64),
    /// Peers to which the manager dispatched this fetch request (for performance scoring).
    PeersAsked(u64, Vec<Peer>),
    Block(u64, Peer, NetworkBlock),
    Done(u64),
}

impl std::fmt::Debug for Blocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBlocks(id, peer) => f.debug_tuple("NoBlocks").field(id).field(peer).finish(),
            Self::NoPeersAvailable(id) => f.debug_tuple("NoPeersAvailable").field(id).finish(),
            Self::PeersAsked(id, peers) => f.debug_tuple("PeersAsked").field(id).field(peers).finish(),
            Self::Block(id, peer, block) => {
                f.debug_tuple("Block").field(id).field(peer).field(&debug_bytes(block.as_slice(), 80)).finish()
            }
            Self::Done(id) => f.debug_tuple("Done").field(id).finish(),
        }
    }
}

/// Message that can be sent by an internal stage to request blocks for a range of points.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockFetchMessage {
    RequestRange {
        from: Point,
        through: Point,
        id: u64,
        cr: StageRef<Blocks>,
    },
    /// Terminal close. An idle instance sends one wire `ClientDone`.
    Close,
}

impl<T> IntoRoleCall<ToResponder, T> for MuxClient
where
    Message: From<T>,
{
    type Reply = Sent;
    const TIMEOUT: Duration = NETWORK_SEND_TIMEOUT;

    fn encode(&self, msg: T, reply: StageRef<Sent>) -> MuxMessage {
        self.encode_send(Message::from(msg), reply)
    }
}

impl FromMailbox<Mail> for Fetch {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn from_mailbox(msg: Mail) -> Result<Self, Mail> {
        match msg {
            Inputs::Local(BlockFetchMessage::RequestRange { from, through, id, cr }) => {
                Ok(Fetch { from: from.to_network_point(), through: through.to_network_point(), id, cr })
            }
            other => Err(other),
        }
    }
}

impl FromMailbox<Mail> for Close {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn from_mailbox(msg: Mail) -> Result<Self, Mail> {
        match msg {
            Inputs::Local(BlockFetchMessage::Close) => Ok(Close),
            other => Err(other),
        }
    }
}

impl FromMailbox<Mail> for StartBatch {
    fn from_mailbox(msg: Mail) -> Result<Self, Mail> {
        from_wire::<_, Message, _>(msg)
    }
}

impl FromMailbox<Mail> for NoBlocks {
    fn from_mailbox(msg: Mail) -> Result<Self, Mail> {
        from_wire::<_, Message, _>(msg)
    }
}

impl FromMailbox<Mail> for Block {
    fn from_mailbox(msg: Mail) -> Result<Self, Mail> {
        from_wire::<_, Message, _>(msg)
    }
}

impl FromMailbox<Mail> for BatchDone {
    fn from_mailbox(msg: Mail) -> Result<Self, Mail> {
        from_wire::<_, Message, _>(msg)
    }
}

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<Pipelined<Instance>>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Instance>().boxed(),
        amaru_pure_stage::register_data_deserializer::<BlockFetchMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Blocks>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Fetch>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Close>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Inflight>().boxed(),
        amaru_pure_stage::register_data_deserializer::<MuxClient>().boxed(),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Inflight {
    id: u64,
    cr: StageRef<Blocks>,
    remaining: usize,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Instance {
    proto: Proto,
    mux: MuxClient,
    inflight: Option<Inflight>,
    peer: Peer,
    pending_close: bool,
    /// Next range from fetch-blocks, issued at last body (before `BatchDone`).
    pending_fetch: Option<Fetch>,
}

type Mail = Inputs<BlockFetchMessage>;
type Handler = Pipelined<Instance>;

impl OccupancyOf for Instance {
    fn occupancy(&self) -> Occupancy {
        self.proto.occupancy()
    }
}

impl Instance {
    fn new(mux: MuxClient, peer: Peer) -> Self {
        Self {
            proto: initial_state::<Idle>().into(),
            mux,
            inflight: None,
            peer,
            pending_close: false,
            pending_fetch: None,
        }
    }

    fn timeout_mail() -> Mail {
        Inputs::Internal(Internal::Timeout)
    }
}

async fn instance(inst: Instance, mail: Mail, eff: Effects<Mail>) -> Instance {
    if matches!(mail, Inputs::Network(HandlerMessage::Registered(_))) {
        return inst;
    }
    let follow_eff = eff.clone();
    let pull_eff = eff.clone();
    let Instance { proto, mux, mut inflight, peer, mut pending_close, mut pending_fetch } = inst;
    let proto = match proto {
        Proto::Idle(idle) => match idle.convert_input(mail) {
            Ok(PipelineIdleIn::Fetch(fetch)) => {
                let range = RequestRange { from: fetch.from, through: fetch.through };
                inflight = Some(Inflight { id: fetch.id, cr: fetch.cr.clone(), remaining: MAX_FETCHED_BLOCKS });
                let (_, s) = idle.receive(fetch, eff).call(&mux, range).await;
                s.finish().into()
            }
            Ok(PipelineIdleIn::Close(close)) => {
                inflight = None;
                pending_close = false;
                let (_, s) = idle.receive(close, eff).call(&mux, ClientDone).await;
                s.finish().into()
            }
            // TODO: handle timeouts generically to make mistakes impossible
            Err(Inputs::Internal(Internal::Timeout)) => idle.into(),
            Err(mail) => return invalid(peer, idle.name(), mail, eff).await,
        },
        Proto::Busy(busy) => match busy.convert_input(mail) {
            Ok(ClientBusyIn::Pull(pull)) => busy
                .receive(pull, eff)
                .send(&mux, WantNext)
                .await
                .set_timeout(BLOCKFETCH_AGENCY_TIMEOUT, Instance::timeout_mail())
                .await
                .finish()
                .into(),
            Ok(ClientBusyIn::StartBatch(start)) => busy
                .receive(start, eff)
                .send(&mux, WantNext)
                .await
                .set_timeout(BLOCKFETCH_AGENCY_TIMEOUT, Instance::timeout_mail())
                .await
                .finish()
                .into(),
            Ok(ClientBusyIn::NoBlocks(no_blocks)) => {
                let Some(flight) = inflight.take() else {
                    return invalid(peer, busy.name(), no_blocks, eff).await;
                };
                let collector = CollectorOut::new(flight.cr);
                busy.receive(no_blocks, eff)
                    .clear_timeout()
                    .await
                    .send_any(&collector, Blocks::NoBlocks(flight.id, peer))
                    .await
                    .finish()
                    .into()
            }
            Err(Inputs::Local(BlockFetchMessage::Close)) => {
                pending_close = true;
                busy.into()
            }
            Err(mail) => match Fetch::from_mailbox(mail) {
                Ok(fetch) => {
                    pending_fetch = Some(fetch);
                    busy.into()
                }
                Err(mail) => return invalid(peer, busy.name(), mail, eff).await,
            },
        },
        Proto::Streaming(streaming) => match streaming.convert_input(mail) {
            Ok(ClientStreamingIn::Block(block)) => {
                let Some(flight) = inflight.as_mut() else {
                    return invalid(peer, streaming.name(), &block, eff).await;
                };
                if flight.remaining == 0 {
                    return invalid(peer, streaming.name(), "too many blocks", eff).await;
                }
                let Ok(network_block) = NetworkBlock::try_from(RawBlock::from(block.body.as_slice())) else {
                    return invalid(peer, streaming.name(), "invalid block CBOR", eff).await;
                };
                flight.remaining -= 1;
                let collector = CollectorOut::new(flight.cr.clone());
                let id = flight.id;
                streaming
                    .receive(block, eff)
                    .send(&mux, WantNext)
                    .await
                    .send_any(&collector, Blocks::Block(id, peer, network_block))
                    .await
                    .set_timeout(BLOCKFETCH_AGENCY_TIMEOUT, Instance::timeout_mail())
                    .await
                    .finish()
                    .into()
            }
            Ok(ClientStreamingIn::BatchDone(done)) => {
                let Some(flight) = inflight.take() else {
                    return invalid(peer, streaming.name(), done, eff).await;
                };
                let collector = CollectorOut::new(flight.cr);
                streaming
                    .receive(done, eff)
                    .clear_timeout()
                    .await
                    .send_any(&collector, Blocks::Done(flight.id))
                    .await
                    .finish()
                    .into()
            }
            Err(Inputs::Local(BlockFetchMessage::Close)) => {
                pending_close = true;
                streaming.into()
            }
            Err(mail) => match Fetch::from_mailbox(mail) {
                Ok(fetch) => {
                    pending_fetch = Some(fetch);
                    streaming.into()
                }
                Err(mail) => return invalid(peer, streaming.name(), mail, eff).await,
            },
        },
        Proto::Done(done) => match mail {
            Inputs::Internal(Internal::Timeout) => done.into(),
            mail @ (Inputs::Local(_) | Inputs::Network(_) | Inputs::Internal(Internal::Pull)) => {
                return invalid(peer, done.name(), mail, eff).await;
            }
        },
    };
    let proto = match (pending_close, pending_fetch.take(), proto) {
        (true, _, Proto::Idle(idle)) => {
            pending_close = false;
            inflight = None;
            let (_, s) = idle.receive(Close, follow_eff).call(&mux, ClientDone).await;
            s.finish().into()
        }
        (false, Some(fetch), Proto::Idle(idle)) => {
            let range = RequestRange { from: fetch.from, through: fetch.through };
            inflight = Some(Inflight { id: fetch.id, cr: fetch.cr.clone(), remaining: MAX_FETCHED_BLOCKS });
            let (_, s) = idle.receive(fetch, follow_eff).call(&mux, range).await;
            s.finish()
                .receive(Pull, pull_eff)
                .send(&mux, WantNext)
                .await
                .set_timeout(BLOCKFETCH_AGENCY_TIMEOUT, Instance::timeout_mail())
                .await
                .finish()
                .into()
        }
        (_, fetch, proto) => {
            pending_fetch = fetch;
            proto
        }
    };
    Instance { proto, mux, inflight, peer, pending_close, pending_fetch }
}

async fn invalid(peer: Peer, state: &str, input: impl std::fmt::Debug, eff: Effects<Mail>) -> Instance {
    error!(
        protocols::INVALID_INPUT,
        proto = "block_fetch",
        peer,
        state = state.to_string(),
        input = format!("{input:?}")
    );
    eff.terminate().await
}

impl Pipelined<Instance> {
    fn for_peer(n: NonZeroU8, muxer: StageRef<MuxMessage>, peer: Peer) -> Self {
        let mux = MuxClient::new(muxer, PROTO_N2N_BLOCK_FETCH.erase());
        Pipelined::new(pipeline_slots(n), |_| Instance::new(mux.clone(), peer))
    }
}

async fn lock_step(state: Instance, msg: Mail, eff: Effects<Mail>) -> Instance {
    drive(state, msg, eff, instance).await
}

async fn handler(state: Handler, msg: Mail, eff: Effects<Mail>) -> Handler {
    pipelined(state, msg, eff, instance).await
}

pub async fn register_blockfetch_initiator<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    n: NonZeroU8,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<BlockFetchMessage> {
    let blockfetch = if n.get() == 1 {
        let mux = MuxClient::new(muxer.clone(), PROTO_N2N_BLOCK_FETCH.erase());
        let blockfetch = eff.stage("blockfetch", lock_step).await;
        let blockfetch = eff.supervise(blockfetch, tombstone);
        eff.wire_up(blockfetch, Instance::new(mux, peer)).await
    } else {
        let blockfetch = eff.stage("blockfetch", handler).await;
        let blockfetch = eff.supervise(blockfetch, tombstone);
        eff.wire_up(blockfetch, Handler::for_peer(n, muxer.clone(), peer)).await
    };
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
    use std::{sync::OnceLock, time::Duration};

    use amaru_kernel::{NonEmptyBytes, Point, cbor};
    use amaru_pure_stage::{
        StageGraph,
        simulation::{Run, SimulationBuilder},
        typestate::State,
        typestate_graph,
    };
    use tokio::runtime::{Builder, Runtime};

    use super::{
        super::spec::{assert_message_alphabet_covered, assert_wire_inputs_cover_receives, session_spec},
        *,
    };
    use crate::{
        mux::{MuxMessage, Sent},
        protocol::{Inputs, ProjectionConfig, Role, StateId, check_timeouts, check_want_next, project},
    };

    fn map_i(state: &StateId) -> StateId {
        match state {
            StateId::Named("Idle" | "Busy" | "Streaming" | "Done") => state.clone(),
            StateId::Named(other) => panic!("unexpected named state {other}"),
            StateId::Synthetic { parent, path } => panic!("unexpected synthetic {parent}#{path:?}"),
        }
    }

    #[test]
    fn initiator_projects_to_table_3_7() {
        let g = typestate_graph! {
            proto: Proto,
            receiving: { Idle, Busy, Streaming },
            empty: { Done },
        };
        let cfg = ProjectionConfig::blockfetch_initiator();
        let spec = session_spec();
        assert_eq!(spec.timeout(&Idle::NAME), None);
        assert_eq!(spec.timeout(&Done::NAME), None);
        assert_eq!(spec.timeout(&Busy::NAME), Some(Duration::from_secs(60)));
        assert_eq!(spec.timeout(&Streaming::NAME), Some(Duration::from_secs(60)));
        check_want_next(&g, &cfg).unwrap();
        check_timeouts(&g, &cfg, &spec).unwrap();
        let projected = project(&g, &cfg).unwrap();
        projected.assert_refines(&spec.project(Role::Initiator), map_i);
        assert_eq!(BLOCKFETCH_AGENCY_TIMEOUT, Duration::from_secs(60));
        assert_message_alphabet_covered(&spec, &[]);
        assert_wire_inputs_cover_receives(&g, &cfg);
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
            | MuxMessage::Terminate
            | MuxMessage::SetSduTimeout(_) => {}
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
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));

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
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::from(NoBlocks))))],
        );
        running.run(Run::default()).assert_sleeping();
        running.enqueue_msg(
            &handler,
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::from(NoBlocks))))],
        );
        running.run(Run::skip_wakeups()).assert_idle();

        let collected = running.get_state(&out).cloned().unwrap();
        assert_eq!(
            collected,
            vec![Blocks::NoBlocks(1, Peer::for_test(3001)), Blocks::NoBlocks(2, Peer::for_test(3001))]
        );
        assert_eq!(running.get_state(&mux).unwrap().wants, 2);
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
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
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
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::from(NoBlocks))))],
        );
        running.run(Run::skip_wakeups()).assert_idle();
        assert_eq!(running.get_state(&mux).unwrap().wants, 1);
        assert_eq!(running.get_state(&out).cloned().unwrap(), vec![Blocks::NoBlocks(1, Peer::for_test(3001))]);
    }

    #[test]
    fn lock_step_single_range_does_not_want_next_after_idle() {
        let mut network = SimulationBuilder::default();
        let out = network.stage("out", async |mut inbox: Vec<Blocks>, msg: Blocks, _eff| {
            inbox.push(msg);
            inbox
        });
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let out = network.wire_up(out, Vec::new());
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", lock_step);
        let mux_client = MuxClient::new(mux_ref, PROTO_N2N_BLOCK_FETCH.erase());
        let handler = network.wire_up(handler_b, Instance::new(mux_client, Peer::for_test(3001)));
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
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::from(NoBlocks))))],
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
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
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
    fn extra_fetch_while_full_terminates() {
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
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
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
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        let blocked = running.run(Run::skip_wakeups());
        assert!(matches!(blocked, amaru_pure_stage::simulation::Blocked::Terminated(_)));
    }

    #[test]
    fn start_batch_while_idle_terminates() {
        let mut network = SimulationBuilder::default();
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let _mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", handler);
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::from(StartBatch)))),
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
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::from(ClientDone)))),
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
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
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
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
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
            [Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::from(NoBlocks))))],
        );
        running.run(Run::skip_wakeups()).assert_idle();
        running.enqueue_msg(&handler, [Inputs::Internal(Internal::Timeout)]);
        running.run(Run::skip_wakeups()).assert_idle();
    }
}
