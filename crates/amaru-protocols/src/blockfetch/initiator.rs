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
use amaru_observability::{error, warn};
use amaru_pure_stage::{
    CallAdmission, CallNotAdmitted, DeserializerGuards, Effects, StageRef, define_role, define_role_tag, make_states,
    on_receive,
    typestate::{FinishIn, prelude::*},
};

use super::{BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch, responder::MAX_FETCHED_BLOCKS};
use crate::{
    blockfetch::BLOCKFETCH_AGENCY_TIMEOUT,
    mux::{Frame, HandlerMessage, MuxMessage, Sent},
    protocol::{
        Inputs, Internal, MuxClient, NETWORK_SEND_TIMEOUT, PROTO_N2N_BLOCK_FETCH, Pipelined, Pull, ToMux, WantNext,
        drive, from_wire, ingress_limit, pipelined,
    },
};

pub const BLOCKFETCH_PIPELINE_N: NonZeroU8 = match NonZeroU8::new(2) {
    Some(n) => n,
    None => unreachable!(),
};

pub const BLOCKFETCH_MAX_BLOCK_WIRE_BYTES: usize = 96 * 1024;

pub fn blockfetch_pipeline_max_buffer(n: NonZeroU8) -> usize {
    usize::from(n.get()).saturating_mul(MAX_FETCHED_BLOCKS).saturating_mul(BLOCKFETCH_MAX_BLOCK_WIRE_BYTES)
}

/// Bulk mailbox of the block-fetch handler for pipeline depth `n`.
///
/// One local request and one network message per slot, plus `Registered`,
/// `Close`, and the one stashed newer range. The default bulk mailbox is 10,
/// and `n = 2` stays inside it.
pub fn blockfetch_handler_mailbox(n: NonZeroU8) -> usize {
    const BULK_MAILBOX: usize = 10;
    BULK_MAILBOX.max(2 * usize::from(n.get()) + 4)
}

fn pipeline_slots(n: NonZeroU8) -> NonZeroUsize {
    match NonZeroUsize::new(usize::from(n.get())) {
        Some(n) => n,
        None => unreachable!(),
    }
}

make_states!(pub Proto { Idle; Requested, Streaming, Done } switch Idle, terminal Done);

define_role_tag!(pub ToResponder);
define_role_tag!(pub ToCollector);

define_role!(CollectorOut, ToCollector, Blocks);

on_receive!(Idle as PipelineIdleIn {
    Fetch => { Call<ToResponder, RequestRange> => Requested }
    Close => { Call<ToResponder, ClientDone> | Repeat<SendAny<ToCollector>> => Done }
});
on_receive!(Requested as ClientBusyIn {
    Pull => { Send<ToMux, WantNext>, SetTimeout => Requested }
    StartBatch => { Send<ToMux, WantNext>, SetTimeout => Streaming }
    NoBlocks => { ClearTimeout, Repeat<SendAny<ToCollector>> => Idle }
});
on_receive!(Requested, Sent => Requested);
on_receive!(Requested, CallNotAdmitted => Send<ToCollector, Blocks> => Idle);
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

amaru_pure_stage::impl_label!(Fetch);

/// Local request that closes an idle initiator instance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Close;

amaru_pure_stage::impl_label!(Close);

#[derive(PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub enum Blocks {
    /// Peer responded that it has no blocks in the requested range.
    NoBlocks(u64, Peer),
    /// No initiating connection existed to attempt this request.
    ///
    /// The fetch stage pauses and the armed timeout retries. A connection that exists
    /// but does not admit the request is [`Self::NoneAccepted`], not this outcome.
    NoPeersAvailable(u64),
    /// At least one candidate connection existed, and none admitted the request.
    ///
    /// The fetch stage asks peers it has not already chosen. When it has none, it pauses
    /// on the same timeout as [`Self::NoPeersAvailable`] instead of offering the request
    /// to these connections again.
    NoneAccepted(u64),
    /// Peers whose block-fetch handler admitted this request.
    ///
    /// The connection sends one peer, itself, when that handler's mailbox accepts the range.
    /// A handler that does not accept it is absent, and is not scored for this request.
    PeersAsked(u64, Vec<Peer>),
    Block(u64, Peer, NetworkBlock),
    Done(u64),
}

impl std::fmt::Debug for Blocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBlocks(id, peer) => f.debug_tuple("NoBlocks").field(id).field(peer).finish(),
            Self::NoPeersAvailable(id) => f.debug_tuple("NoPeersAvailable").field(id).finish(),
            Self::NoneAccepted(id) => f.debug_tuple("NoneAccepted").field(id).finish(),
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

    fn into_call(self, msg: T) -> (Duration, impl FnOnce(StageRef<Sent>) -> MuxMessage + std::marker::Send + 'static) {
        self.call_encoded(&Message::from(msg))
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
        amaru_pure_stage::register_data_deserializer::<Pipelined<Instance, BlockFetchMessage>>().boxed(),
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
type Handler = Pipelined<Instance, BlockFetchMessage>;

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
                let (admission, s) = idle.receive(&fetch, eff.clone()).call(&mux, range).await;
                match admission {
                    CallAdmission::Reply(sent) => {
                        inflight = Some(Inflight { id: fetch.id, cr: fetch.cr.clone(), remaining: MAX_FETCHED_BLOCKS });
                        s.finish().receive(&sent, eff.clone()).finish().into()
                    }
                    CallAdmission::NotAdmitted(token) => idle_no_blocks(s, token, fetch.id, fetch.cr, peer, eff).await,
                    CallAdmission::TimedOut(_token) => {
                        return fault_egress(peer, "range_deadline", eff).await;
                    }
                }
            }
            Ok(PipelineIdleIn::Close(close)) => {
                inflight = None;
                pending_close = false;
                let (admission, s) = idle.receive(&close, eff.clone()).call(&mux, ClientDone).await;
                match admission {
                    CallAdmission::Reply(_) => s.finish().into(),
                    CallAdmission::NotAdmitted(_token) => {
                        return fault_egress(peer, "client_done_not_admitted", eff).await;
                    }
                    CallAdmission::TimedOut(_token) => {
                        return fault_egress(peer, "client_done_deadline", eff).await;
                    }
                }
            }
            // TODO: handle timeouts generically to make mistakes impossible
            Err(Inputs::Internal(Internal::Timeout)) => idle.into(),
            Err(mail) => return invalid(peer, idle.name(), mail, eff).await,
        },
        Proto::Requested(requested) => match requested.convert_input(mail) {
            Ok(ClientBusyIn::Pull(pull)) => requested
                .receive(&pull, eff)
                .send(&mux, WantNext)
                .await
                .set_timeout(BLOCKFETCH_AGENCY_TIMEOUT, Instance::timeout_mail())
                .await
                .finish()
                .into(),
            Ok(ClientBusyIn::StartBatch(start)) => requested
                .receive(&start, eff)
                .send(&mux, WantNext)
                .await
                .set_timeout(BLOCKFETCH_AGENCY_TIMEOUT, Instance::timeout_mail())
                .await
                .finish()
                .into(),
            Ok(ClientBusyIn::NoBlocks(no_blocks)) => {
                let Some(flight) = inflight.take() else {
                    return invalid(peer, requested.name(), no_blocks, eff).await;
                };
                let collector = CollectorOut::new(flight.cr);
                requested
                    .receive(&no_blocks, eff)
                    .clear_timeout()
                    .await
                    .send_any(&collector, Blocks::NoBlocks(flight.id, peer))
                    .await
                    .finish()
                    .into()
            }
            Err(Inputs::Local(BlockFetchMessage::Close)) => {
                pending_close = true;
                requested.into()
            }
            Err(mail) => match Fetch::from_mailbox(mail) {
                Ok(fetch) => {
                    pending_fetch = Some(fetch);
                    requested.into()
                }
                Err(mail) => return invalid(peer, requested.name(), mail, eff).await,
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
                    .receive(&block, eff)
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
                    .receive(&done, eff)
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
            let (admission, s) = idle.receive(&Close, follow_eff.clone()).call(&mux, ClientDone).await;
            match admission {
                CallAdmission::Reply(_) => s.finish().into(),
                CallAdmission::NotAdmitted(_token) => {
                    return fault_egress(peer, "client_done_not_admitted", follow_eff).await;
                }
                CallAdmission::TimedOut(_token) => {
                    return fault_egress(peer, "client_done_deadline", follow_eff).await;
                }
            }
        }
        (false, Some(fetch), Proto::Idle(idle)) => {
            let range = RequestRange { from: fetch.from, through: fetch.through };
            let (admission, s) = idle.receive(&fetch, follow_eff.clone()).call(&mux, range).await;
            match admission {
                CallAdmission::Reply(sent) => {
                    inflight = Some(Inflight { id: fetch.id, cr: fetch.cr.clone(), remaining: MAX_FETCHED_BLOCKS });
                    let requested: Requested = s.finish().receive(&sent, follow_eff.clone()).finish();
                    requested
                        .receive(&Pull, pull_eff)
                        .send(&mux, WantNext)
                        .await
                        .set_timeout(BLOCKFETCH_AGENCY_TIMEOUT, Instance::timeout_mail())
                        .await
                        .finish()
                        .into()
                }
                CallAdmission::NotAdmitted(token) => {
                    idle_no_blocks(s, token, fetch.id, fetch.cr, peer, follow_eff).await
                }
                CallAdmission::TimedOut(_token) => return fault_egress(peer, "range_deadline", follow_eff).await,
            }
        }
        (_, fetch, proto) => {
            pending_fetch = fetch;
            proto
        }
    };
    Instance { proto, mux, inflight, peer, pending_close, pending_fetch }
}

async fn idle_no_blocks<Rem, I>(
    session: Session<Mail, Rem>,
    token: CallNotAdmitted,
    id: u64,
    cr: StageRef<Blocks>,
    peer: Peer,
    eff: Effects<Mail>,
) -> Proto
where
    Rem: FinishIn<Requested, I, Out = Requested>,
{
    let collector = CollectorOut::new(cr);
    session.finish().receive(&token, eff).send(&collector, Blocks::NoBlocks(id, peer)).await.finish().into()
}

async fn fault_egress(peer: Peer, reason: &'static str, eff: Effects<Mail>) -> Instance {
    warn!(protocols::EGRESS_DEADLINE, proto = "block_fetch", peer, reason = reason);
    eff.terminate().await
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

impl Pipelined<Instance, BlockFetchMessage> {
    fn for_peer(n: NonZeroU8, muxer: StageRef<MuxMessage>, peer: Peer) -> Self {
        let mux = MuxClient::new(muxer, PROTO_N2N_BLOCK_FETCH.erase());
        Pipelined::new(pipeline_slots(n), |_| Instance::new(mux.clone(), peer))
    }
}

async fn lock_step(state: Instance, msg: Mail, eff: Effects<Mail>) -> Instance {
    drive(state, msg, eff, instance).await
}

async fn handler(state: Handler, msg: Mail, eff: Effects<Mail>) -> Handler {
    pipelined(state, msg, eff, instance, |msg| matches!(msg, BlockFetchMessage::Close)).await
}

pub async fn register_blockfetch_initiator<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    n: NonZeroU8,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<BlockFetchMessage> {
    let mailbox = blockfetch_handler_mailbox(n);
    let blockfetch = if n.get() == 1 {
        let mux = MuxClient::new(muxer.clone(), PROTO_N2N_BLOCK_FETCH.erase());
        let blockfetch = eff.stage_with_mailbox_size("blockfetch", lock_step, mailbox).await;
        let blockfetch = eff.supervise(blockfetch, tombstone);
        eff.wire_up(blockfetch, Instance::new(mux, peer)).await
    } else {
        let blockfetch = eff.stage_with_mailbox_size("blockfetch", handler, mailbox).await;
        let blockfetch = eff.supervise(blockfetch, tombstone);
        eff.wire_up(blockfetch, Handler::for_peer(n, muxer.clone(), peer)).await
    };
    let protocol = PROTO_N2N_BLOCK_FETCH.erase();
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol,
            frame: Frame::OneCborItem,
            handler: blockfetch.contramap(Inputs::Network),
            max_buffer: ingress_limit(protocol).max(blockfetch_pipeline_max_buffer(n)),
        },
    )
    .await;
    blockfetch.contramap(Inputs::Local)
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use amaru_kernel::{NonEmptyBytes, Point, cbor};
    use amaru_pure_stage::{
        Effect, StageGraph,
        simulation::{Run, SimulationBuilder},
    };
    use tokio::runtime::{Builder, Runtime};

    use super::*;
    use crate::{
        mux::{MuxMessage, Sent},
        protocol::Inputs,
    };

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
            | MuxMessage::SetSduTimeout(_)
            | MuxMessage::IngressRetry
            | MuxMessage::EgressRetry => {}
        }
        log
    }

    fn no_blocks_reply() -> Inputs<BlockFetchMessage> {
        Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&Message::from(NoBlocks))))
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

    /// Probe: `Close` on an idle pipeline, then a range. `ClientDone` ends the
    /// one wire protocol. The range is not sent, and a second `Close` is not
    /// written either.
    #[test]
    fn close_on_idle_then_fetch_is_not_sent() {
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
                    Inputs::Local(BlockFetchMessage::Close),
                    Inputs::Local(BlockFetchMessage::RequestRange {
                        from: Point::Origin,
                        through: Point::Origin,
                        id: 9,
                        cr: (*out).clone(),
                    }),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::skip_wakeups()).assert_idle();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["ClientDone"]);
        assert!(running.get_state(&out).unwrap().is_empty());

        running.enqueue_msg(&handler, [Inputs::Local(BlockFetchMessage::Close)]);
        running.enqueue_msg(
            &handler,
            [Inputs::Local(BlockFetchMessage::RequestRange {
                from: Point::Origin,
                through: Point::Origin,
                id: 10,
                cr: (*out).clone(),
            })],
        );
        running.run(Run::skip_wakeups()).assert_idle();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["ClientDone"]);
        assert!(running.get_state(&out).unwrap().is_empty());
    }

    /// The lock-step instance (`N = 1`) is already in `Done` after `ClientDone`.
    /// A later range is not written; the instance faults that input.
    #[test]
    fn fetch_after_client_done_lock_step_terminates() {
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
                    Inputs::Local(BlockFetchMessage::Close),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::skip_wakeups()).assert_idle();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["ClientDone"]);

        running.enqueue_msg(
            &handler,
            [Inputs::Local(BlockFetchMessage::RequestRange {
                from: Point::Origin,
                through: Point::Origin,
                id: 9,
                cr: (*out).clone(),
            })],
        );
        let blocked = running.run(Run::skip_wakeups());
        assert!(matches!(blocked, amaru_pure_stage::simulation::Blocked::Terminated(_)), "{blocked:?}");
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["ClientDone"]);
        assert!(running.get_state(&out).unwrap().is_empty());
    }

    #[test]
    fn third_fetch_while_full_is_sent_when_a_slot_idles() {
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
        let range = |id, cr| BlockFetchMessage::RequestRange { from: Point::Origin, through: Point::Origin, id, cr };
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(range(1, cr.clone())),
                    Inputs::Local(range(2, cr.clone())),
                    Inputs::Local(range(3, cr)),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        let blocked = running.run(Run::default());
        assert!(
            matches!(blocked, amaru_pure_stage::simulation::Blocked::Sleeping { .. }),
            "a third range must not drop the handler: {blocked:?}"
        );
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange"]);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange", "RequestRange"]);
        assert_eq!(running.get_state(&out).cloned().unwrap(), vec![Blocks::NoBlocks(1, Peer::for_test(3001))]);
    }

    /// Two ranges occupy both slots. A third is held until a slot returns to `Idle`.
    /// The three `NoBlocks` replies must come back as ids 1, 2, 3. Applying the held
    /// range inside the finishing slot, before the receive cursor moves, would hand
    /// the second range's reply to the new id.
    #[test]
    fn stashed_fetch_keeps_wire_order() {
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
        let range = |id, cr| BlockFetchMessage::RequestRange { from: Point::Origin, through: Point::Origin, id, cr };
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(range(1, cr.clone())),
                    Inputs::Local(range(2, cr.clone())),
                    Inputs::Local(range(3, cr)),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();

        let log = running.get_state(&mux).cloned().unwrap();
        assert_eq!(log.sends, vec!["RequestRange", "RequestRange"]);
        assert_eq!(log.wants, 1);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange", "RequestRange"]);
        assert_eq!(running.get_state(&out).cloned().unwrap(), vec![Blocks::NoBlocks(1, Peer::for_test(3001))]);
        assert_eq!(running.get_state(&mux).unwrap().wants, 2);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(
            running.get_state(&out).cloned().unwrap(),
            vec![Blocks::NoBlocks(1, Peer::for_test(3001)), Blocks::NoBlocks(2, Peer::for_test(3001))]
        );
        assert_eq!(running.get_state(&mux).unwrap().wants, 3);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::skip_wakeups()).assert_idle();
        assert_eq!(
            running.get_state(&out).cloned().unwrap(),
            vec![
                Blocks::NoBlocks(1, Peer::for_test(3001)),
                Blocks::NoBlocks(2, Peer::for_test(3001)),
                Blocks::NoBlocks(3, Peer::for_test(3001)),
            ]
        );
    }

    #[test]
    fn newer_fetch_replaces_the_stash() {
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
        let range = |id, cr| BlockFetchMessage::RequestRange { from: Point::Origin, through: Point::Origin, id, cr };
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(range(1, cr.clone())),
                    Inputs::Local(range(2, cr.clone())),
                    Inputs::Local(range(3, cr.clone())),
                    Inputs::Local(range(4, cr)),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange"]);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::default()).assert_sleeping();
        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::default()).assert_sleeping();
        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::skip_wakeups()).assert_idle();
        let peer = Peer::for_test(3001);
        assert_eq!(
            running.get_state(&out).cloned().unwrap(),
            vec![Blocks::NoBlocks(1, peer), Blocks::NoBlocks(2, peer), Blocks::NoBlocks(4, peer)]
        );
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange", "RequestRange"]);
    }

    #[test]
    fn close_while_busy_waits_for_inflight_slots() {
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
        let range = |id, cr| BlockFetchMessage::RequestRange { from: Point::Origin, through: Point::Origin, id, cr };
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(range(1, cr.clone())),
                    Inputs::Local(range(2, cr)),
                    Inputs::Local(BlockFetchMessage::Close),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        let blocked = running.run(Run::default());
        assert!(
            matches!(blocked, amaru_pure_stage::simulation::Blocked::Sleeping { .. }),
            "Close while both slots are busy must not drop the handler: {blocked:?}"
        );
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange"]);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(running.get_state(&out).cloned().unwrap(), vec![Blocks::NoBlocks(1, Peer::for_test(3001))]);
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange"]);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::skip_wakeups()).assert_idle();
        let peer = Peer::for_test(3001);
        assert_eq!(
            running.get_state(&out).cloned().unwrap(),
            vec![Blocks::NoBlocks(1, peer), Blocks::NoBlocks(2, peer)]
        );
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange", "ClientDone"]);
    }

    /// Two ranges in flight, a stashed third, `Close`, and a fourth while both
    /// slots are busy. The stash and the fourth are not sent. After `ClientDone`
    /// a further range is not sent either.
    #[test]
    fn fetch_after_client_done_is_not_sent() {
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
        let range = |id, cr| BlockFetchMessage::RequestRange { from: Point::Origin, through: Point::Origin, id, cr };
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(range(1, cr.clone())),
                    Inputs::Local(range(2, cr.clone())),
                    Inputs::Local(range(3, cr.clone())),
                    Inputs::Local(BlockFetchMessage::Close),
                    Inputs::Local(range(4, cr.clone())),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange"]);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::default()).assert_sleeping();
        let peer = Peer::for_test(3001);
        assert_eq!(running.get_state(&out).cloned().unwrap(), vec![Blocks::NoBlocks(1, peer)]);
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange"]);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::skip_wakeups()).assert_idle();
        assert_eq!(
            running.get_state(&out).cloned().unwrap(),
            vec![Blocks::NoBlocks(1, peer), Blocks::NoBlocks(2, peer)]
        );
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange", "ClientDone"]);

        running.enqueue_msg(&handler, [Inputs::Local(range(5, cr))]);
        running.run(Run::skip_wakeups()).assert_idle();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange", "ClientDone"]);
        assert_eq!(
            running.get_state(&out).cloned().unwrap(),
            vec![Blocks::NoBlocks(1, peer), Blocks::NoBlocks(2, peer)]
        );
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

    #[test]
    fn request_that_never_reaches_the_mux_is_no_blocks_without_want_next() {
        use amaru_pure_stage::{
            DEFAULT_MAILBOX_SIZE, Effect,
            simulation::Blocked,
            trace_buffer::{TraceBuffer, TraceEntry},
            trace_match::{TraceMatch, assert_trace_does_not_contain, tm_send_match},
        };

        // Park on every message, including the priming `IngressRetry`. A hold that
        // ignores non-`Send` traffic returns Idle, so the mailbox never fills and
        // the later call is admitted instead of `NotAdmitted`.
        async fn hold(_state: u8, _msg: MuxMessage, eff: Effects<MuxMessage>) -> u8 {
            eff.wait(Duration::from_secs(3600)).await;
            0
        }

        let _mux = crate::mux::register_deserializers();
        let _bf = super::register_deserializers();
        let trace = TraceBuffer::new_shared(400, 1_000_000);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace);
        let out = network.stage("out", async |mut inbox: Vec<Blocks>, msg: Blocks, _eff| {
            inbox.push(msg);
            inbox
        });
        let mux = network.stage("mux", hold);
        let mux_ref = mux.sender();
        let out = network.wire_up(out, Vec::new());
        let mux = network.wire_up(mux, 0u8);
        let handler_b = network.stage("bf", lock_step);
        let peer = Peer::for_test(3001);
        let handler =
            network.wire_up(handler_b, Instance::new(MuxClient::new(mux_ref, PROTO_N2N_BLOCK_FETCH.erase()), peer));
        let mut running = network.run(test_runtime());

        running.enqueue_msg(&mux, [MuxMessage::IngressRetry]);
        assert!(matches!(running.run(Run::default()), Blocked::Sleeping { .. }));
        for _ in 0..DEFAULT_MAILBOX_SIZE {
            running.enqueue_msg(&mux, [MuxMessage::IngressRetry]);
        }
        assert_eq!(running.mailbox_len(&mux), DEFAULT_MAILBOX_SIZE);

        running.enqueue_msg(
            &handler,
            [Inputs::Local(BlockFetchMessage::RequestRange {
                from: Point::Origin,
                through: Point::Origin,
                id: 7,
                cr: (*out).clone(),
            })],
        );
        let Blocked::Sleeping { next_wakeup } = running.run(Run::default()) else {
            panic!("call should be waiting on its deadline");
        };
        assert!(running.get_state(&out).unwrap().is_empty());
        let early =
            running.now() + next_wakeup.saturating_since(running.now()).saturating_sub(Duration::from_millis(1));
        assert!(matches!(running.run(Run::until(early)), Blocked::Sleeping { .. }));
        assert!(running.get_state(&out).unwrap().is_empty());
        let _alive = running.mailbox_len(&handler);

        let blocked = running.run(Run::until(next_wakeup));
        assert!(matches!(blocked, Blocked::Sleeping { .. } | Blocked::Idle), "{blocked:?}");
        assert_eq!(running.get_state(&out).unwrap().as_slice(), &[Blocks::NoBlocks(7, peer)]);
        assert!(matches!(running.get_state(&handler).unwrap().proto, Proto::Idle(_)));

        let from = handler.name().to_string();
        let entries: Vec<TraceEntry> = running.trace_buffer().lock().iter_entries().map(|(_, e)| e).collect();
        let no_blocks = tm_send_match::<Blocks>(&from, "out", |msg| matches!(msg, Blocks::NoBlocks(7, _)));
        assert!(entries.iter().any(|entry| entry == &no_blocks), "expected NoBlocks via tm_send_match");
        let from_timeout = from.clone();
        assert_trace_does_not_contain(
            &running,
            &[
                tm_send_match::<MuxMessage>(&from, "mux", |msg| matches!(msg, MuxMessage::WantNext(_))),
                TraceMatch::Property(
                    Box::new(move |src| {
                        matches!(
                            src.suspend(),
                            Some(Effect::SetTimeout { at_stage, .. }) if at_stage.as_str() == from_timeout
                        )
                    }),
                    "SetTimeout".into(),
                ),
            ],
        );
    }

    /// Both slots are busy and one range is stashed. The slot that finishes is idle
    /// again, so the stash is offered. The mux is not receiving, the offer is
    /// `NotAdmitted`, and the handler reports that id and stays up. The other
    /// in-flight body is still attributed to its own id.
    #[test]
    fn stash_delivered_when_not_admitted_frees_the_slot() {
        use amaru_pure_stage::simulation::Blocked;

        #[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        struct Gate {
            admitted: u8,
            wants: usize,
            sends: Vec<String>,
        }

        async fn gate(mut gate: Gate, msg: MuxMessage, eff: Effects<MuxMessage>) -> Gate {
            match msg {
                MuxMessage::Send(_, bytes, cr) if gate.admitted < 2 => {
                    let decoded: Message = cbor::decode(bytes.as_ref()).expect("cbor");
                    gate.sends.push(decoded.message_type().to_string());
                    gate.admitted += 1;
                    eff.send(&cr, Sent).await;
                }
                MuxMessage::WantNext(_) if gate.wants == 0 => {
                    gate.wants = 1;
                }
                // The next pull is accepted, then this stage stops receiving. The
                // stashed range's call finds a rendezvous mailbox that is not waiting
                // and comes back `NotAdmitted`.
                MuxMessage::WantNext(_) => {
                    eff.wait(std::time::Duration::from_secs(3600)).await;
                }
                MuxMessage::Send(_, _, _)
                | MuxMessage::Register { .. }
                | MuxMessage::Buffer(..)
                | MuxMessage::FromNetwork(..)
                | MuxMessage::Written
                | MuxMessage::Terminate
                | MuxMessage::SetSduTimeout(_)
                | MuxMessage::IngressRetry
                | MuxMessage::EgressRetry => {}
            }
            gate
        }

        let mut network = SimulationBuilder::default();
        let out = network.stage("out", async |mut inbox: Vec<Blocks>, msg: Blocks, _eff| {
            inbox.push(msg);
            inbox
        });
        let mux = network.stage_with_mailbox_size("mux", gate, 0);
        let mux_ref = mux.sender();
        let out = network.wire_up(out, Vec::new());
        let mux = network.wire_up(mux, Gate::default());
        let handler_b = network.stage("bf", handler);
        let handler =
            network.wire_up(handler_b, Handler::for_peer(BLOCKFETCH_PIPELINE_N, mux_ref, Peer::for_test(3001)));
        let cr = (*out).clone();
        let range = |id, cr| BlockFetchMessage::RequestRange { from: Point::Origin, through: Point::Origin, id, cr };
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(PROTO_N2N_BLOCK_FETCH.erase())),
                    Inputs::Local(range(1, cr.clone())),
                    Inputs::Local(range(2, cr.clone())),
                    Inputs::Local(range(3, cr)),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::default()).assert_sleeping();
        assert_eq!(running.get_state(&mux).unwrap().sends, vec!["RequestRange", "RequestRange"]);

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        let Blocked::Sleeping { next_wakeup } = running.run(Run::default()) else {
            panic!("stashed range should be waiting on the admission deadline");
        };
        let peer = Peer::for_test(3001);
        assert_eq!(running.get_state(&out).cloned().unwrap(), vec![Blocks::NoBlocks(1, peer)]);

        let blocked = running.run(Run::until(next_wakeup));
        assert!(matches!(blocked, Blocked::Sleeping { .. } | Blocked::Idle), "{blocked:?}");
        assert_eq!(
            running.get_state(&out).cloned().unwrap(),
            vec![Blocks::NoBlocks(1, peer), Blocks::NoBlocks(3, peer)]
        );
        // The mux is inside the pull that made its mailbox refuse the stashed
        // range, so its state is not readable. The handler is: it did not fault.
        assert!(running.get_state(&handler).is_some(), "handler must still be running");

        running.enqueue_msg(&handler, [no_blocks_reply()]);
        running.run(Run::default()).assert_sleeping();
        assert_eq!(
            running.get_state(&out).cloned().unwrap(),
            vec![Blocks::NoBlocks(1, peer), Blocks::NoBlocks(3, peer), Blocks::NoBlocks(2, peer)]
        );
    }

    #[test]
    fn blockfetch_handler_mailbox_is_max_10_or_2n_plus_4() {
        let cases = [(1u8, 10usize), (2, 10), (4, 12)];
        for (n, expected) in cases {
            let depth = NonZeroU8::new(n).unwrap();
            assert_eq!(blockfetch_handler_mailbox(depth), expected);

            let mut network = SimulationBuilder::default();
            let boot = network.stage("boot", async |_state: u8, depth: u8, eff: Effects<u8>| {
                let mux = eff.stage("mux", async |s: u8, _msg: MuxMessage, _eff: Effects<MuxMessage>| s).await;
                let mux = eff.wire_up(mux, 0u8).await;
                let depth = NonZeroU8::new(depth).unwrap();
                let _handler = register_blockfetch_initiator(&mux, Peer::for_test(1), depth, &eff, 0u8).await;
                0
            });
            let boot = network.wire_up(boot, 0u8);
            let mut running = network.run(test_runtime());
            running.breakpoint(
                "bf-mail",
                |eff| matches!(eff, Effect::WireStage { name, .. } if name.as_str().starts_with("blockfetch")),
            );
            running.enqueue_msg(&boot, [n]);
            running.run(Run::default()).assert_breakpoint("bf-mail");
            let hit = running.breakpoint_effect();
            let Effect::WireStage { mailbox_size, .. } = hit.effect() else {
                panic!("expected the block-fetch handler to be wired");
            };
            assert_eq!(*mailbox_size, expected, "N={n}");
        }
    }
}
