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

//! Lock-step BlockFetch expressed with the opt-in typestate layer.
//!
//! This sits beside [`ProtocolState`](crate::protocol::ProtocolState) / [`ProtoSpec`]
//! and does not replace them. Receive constructors are indexed by individual
//! message variants, not by [`Message`](super::Message) or the stage mailbox
//! type. Pipelining (depth 2) is not typed here yet.

use amaru_kernel::NetworkPoint;
use amaru_pure_stage::{define_role_tag, make_states, on_receive, typestate::prelude::*};

use super::Message;

make_states!(pub Live { Idle; Busy, Streaming, Done });

define_role_tag!(pub ToInitiator);
define_role_tag!(pub ToResponder);

/// Local request that starts an initiator fetch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fetch {
    pub from: NetworkPoint,
    pub through: NetworkPoint,
}

/// Local request that closes an idle initiator.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Close;

/// Local request that streams one more block from a responder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NextBlock {
    pub body: Vec<u8>,
}

/// Local request that ends a responder batch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EndBatch;

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

// Initiator: local fetch / close, then network replies.
on_receive!(Idle as ClientIdleIn {
    Fetch => { Send<ToResponder, RequestRange> => Busy }
    Close => { Send<ToResponder, ClientDone> => Done }
});
on_receive!(Busy as ClientBusyIn {
    StartBatch => { Streaming }
    NoBlocks => { Idle }
});
on_receive!(Streaming as ClientStreamingIn {
    Block => { Streaming }
    BatchDone => { Idle }
});

// Responder: network requests, then local stream commands.
on_receive!(Idle as ServerIdleIn {
    RequestRange => { Send<ToInitiator, StartBatch> => Streaming | Send<ToInitiator, NoBlocks> => Idle }
    ClientDone => { Done }
});
on_receive!(Busy as ServerBusyIn {});
on_receive!(Streaming as ServerStreamingIn {
    NextBlock => { Send<ToInitiator, Block> => Streaming }
    EndBatch => { Send<ToInitiator, BatchDone> => Idle }
});
on_receive!(Done as DoneIn {});

#[cfg(test)]
mod tests {
    use amaru_kernel::NetworkPoint;
    use amaru_pure_stage::{
        Effects, StageGraph, StageRef, define_mailbox, define_role,
        simulation::SimulationBuilder,
        typestate::{FmtPar, OnReceive, Session},
    };

    use super::*;

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

    #[test]
    fn initiator_receive_allowances() {
        assert_eq!(remaining::<Idle, Fetch>(), format!("{} => Busy", send_desc::<ToResponder, RequestRange>()));
        assert_eq!(remaining::<Idle, Close>(), format!("{} => Done", send_desc::<ToResponder, ClientDone>()));
        assert_eq!(remaining::<Busy, StartBatch>(), "=> Streaming");
        assert_eq!(remaining::<Busy, NoBlocks>(), "=> Idle");
        assert_eq!(remaining::<Streaming, Block>(), "=> Streaming");
        assert_eq!(remaining::<Streaming, BatchDone>(), "=> Idle");
    }

    #[test]
    fn responder_receive_allowances() {
        assert_eq!(
            remaining::<Idle, RequestRange>(),
            format!(
                "{} => Streaming | {} => Idle",
                send_desc::<ToInitiator, StartBatch>(),
                send_desc::<ToInitiator, NoBlocks>()
            )
        );
        assert_eq!(remaining::<Idle, ClientDone>(), "=> Done");
        assert_eq!(remaining::<Streaming, NextBlock>(), format!("{} => Streaming", send_desc::<ToInitiator, Block>()));
        assert_eq!(remaining::<Streaming, EndBatch>(), format!("{} => Idle", send_desc::<ToInitiator, BatchDone>()));
    }

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Collected {
        NoBlocks,
        Block(Vec<u8>),
        BatchDone,
        ClientDone,
    }

    define_mailbox!(ClientIn {
        Fetch(Fetch),
        Close(Close),
        StartBatch(StartBatch),
        NoBlocks(NoBlocks),
        Block(Block),
        BatchDone(BatchDone),
    });

    define_mailbox!(ServerIn {
        RequestRange(RequestRange),
        ClientDone(ClientDone),
        NextBlock(NextBlock),
        EndBatch(EndBatch),
    });

    define_role!(ToServer, ToResponder, ServerIn);
    define_role!(ToClient, ToInitiator, ClientIn);

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Client {
        live: Live,
        peer: ToServer,
        out: StageRef<Collected>,
    }

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Server {
        live: Live,
        peer: ToClient,
        remaining: Vec<Vec<u8>>,
    }

    async fn client_step(state: Client, msg: ClientIn, eff: Effects<ClientIn>) -> Client {
        match state.live {
            Live::Idle(idle) => match idle.convert_input(msg) {
                Ok(ClientIdleIn::Fetch(fetch)) => {
                    let range = RequestRange { from: fetch.from, through: fetch.through };
                    Client { live: idle.receive(fetch, eff).send(&state.peer, range).await.finish().into(), ..state }
                }
                Ok(ClientIdleIn::Close(close)) => {
                    let s = idle.receive(close, eff).send(&state.peer, ClientDone).await;
                    s.notify(&state.out, Collected::ClientDone).await;
                    Client { live: s.finish().into(), ..state }
                }
                Err(_msg) => Client { live: idle.into(), ..state },
            },
            Live::Busy(busy) => match busy.convert_input(msg) {
                Ok(ClientBusyIn::StartBatch(start)) => {
                    Client { live: busy.receive(start, eff).finish().into(), ..state }
                }
                Ok(ClientBusyIn::NoBlocks(none)) => {
                    let s = busy.receive(none, eff);
                    s.notify(&state.out, Collected::NoBlocks).await;
                    Client { live: s.finish().into(), ..state }
                }
                Err(_msg) => Client { live: busy.into(), ..state },
            },
            Live::Streaming(st) => match st.convert_input(msg) {
                Ok(ClientStreamingIn::Block(block)) => {
                    let s = st.receive(block.clone(), eff);
                    s.notify(&state.out, Collected::Block(block.body)).await;
                    Client { live: s.finish().into(), ..state }
                }
                Ok(ClientStreamingIn::BatchDone(done)) => {
                    let s = st.receive(done, eff);
                    s.notify(&state.out, Collected::BatchDone).await;
                    Client { live: s.finish().into(), ..state }
                }
                Err(_msg) => Client { live: st.into(), ..state },
            },
            Live::Done(done) => match done.convert_input::<DoneIn, _>(msg) {
                Ok(never) => match never {},
                Err(_msg) => Client { live: done.into(), ..state },
            },
        }
    }

    async fn server_step(mut state: Server, msg: ServerIn, eff: Effects<ServerIn>) -> Server {
        match state.live {
            Live::Idle(idle) => match idle.convert_input(msg) {
                Ok(ServerIdleIn::RequestRange(range)) => {
                    let s = idle.receive(range, eff);
                    if state.remaining.is_empty() {
                        Server { live: s.send(&state.peer, NoBlocks).await.finish().into(), ..state }
                    } else {
                        let first = NextBlock { body: state.remaining.remove(0) };
                        let s = s.send(&state.peer, StartBatch).await;
                        s.notify(&s.me(), first.into()).await;
                        Server { live: s.finish().into(), ..state }
                    }
                }
                Ok(ServerIdleIn::ClientDone(done)) => {
                    Server { live: idle.receive(done, eff).finish().into(), ..state }
                }
                Err(_msg) => Server { live: idle.into(), ..state },
            },
            Live::Busy(busy) => match busy.convert_input::<ServerBusyIn, _>(msg) {
                Ok(never) => match never {},
                Err(_msg) => Server { live: busy.into(), ..state },
            },
            Live::Streaming(st) => match st.convert_input(msg) {
                Ok(ServerStreamingIn::NextBlock(next)) => {
                    let s = st.receive(next.clone(), eff).send(&state.peer, Block { body: next.body }).await;
                    if state.remaining.is_empty() {
                        s.notify(&s.me(), EndBatch.into()).await;
                    } else {
                        let body = state.remaining.remove(0);
                        s.notify(&s.me(), NextBlock { body }.into()).await;
                    }
                    Server { live: s.finish().into(), ..state }
                }
                Ok(ServerStreamingIn::EndBatch(end)) => {
                    Server { live: st.receive(end, eff).send(&state.peer, BatchDone).await.finish().into(), ..state }
                }
                Err(_msg) => Server { live: st.into(), ..state },
            },
            Live::Done(done) => match done.convert_input::<DoneIn, _>(msg) {
                Ok(never) => match never {},
                Err(_msg) => Server { live: done.into(), ..state },
            },
        }
    }

    fn run_pair(server_blocks: Vec<Vec<u8>>, client_msgs: Vec<ClientIn>) -> (Live, Live, Vec<Collected>) {
        let mut network = SimulationBuilder::default();
        let out = network.stage("out", async |mut inbox: Vec<Collected>, msg: Collected, _eff| {
            inbox.push(msg);
            inbox
        });
        let client_b = network.stage("client", client_step);
        let server_b = network.stage("server", server_step);

        let client_ref = client_b.sender();
        let server_ref = server_b.sender();

        let out = network.wire_up(out, Vec::new());
        let client = network.wire_up(
            client_b,
            Client { live: initial_state::<Idle>().into(), peer: ToServer::new(server_ref), out: (*out).clone() },
        );
        let server = network.wire_up(
            server_b,
            Server { live: initial_state::<Idle>().into(), peer: ToClient::new(client_ref), remaining: server_blocks },
        );

        network.preload(&client, client_msgs).unwrap();
        let mut running = network.run();
        running.run_until_blocked().assert_idle();
        (
            running.get_state(&client).unwrap().live.clone(),
            running.get_state(&server).unwrap().live.clone(),
            running.get_state(&out).cloned().unwrap(),
        )
    }

    #[test]
    fn lockstep_fetch_streams_blocks_then_returns_to_idle() {
        let (client, server, collected) = run_pair(
            vec![b"one".to_vec(), b"two".to_vec()],
            vec![Fetch { from: NetworkPoint::Origin, through: NetworkPoint::Origin }.into()],
        );
        assert!(matches!(client, Live::Idle(_)));
        assert!(matches!(server, Live::Idle(_)));
        assert_eq!(
            collected,
            vec![Collected::Block(b"one".to_vec()), Collected::Block(b"two".to_vec()), Collected::BatchDone]
        );
    }

    #[test]
    fn lockstep_fetch_without_blocks() {
        let (client, server, collected) =
            run_pair(vec![], vec![Fetch { from: NetworkPoint::Origin, through: NetworkPoint::Origin }.into()]);
        assert!(matches!(client, Live::Idle(_)));
        assert!(matches!(server, Live::Idle(_)));
        assert_eq!(collected, vec![Collected::NoBlocks]);
    }

    #[test]
    fn lockstep_client_done() {
        let (client, server, collected) = run_pair(vec![], vec![Close.into()]);
        assert!(matches!(client, Live::Done(_)));
        assert!(matches!(server, Live::Done(_)));
        assert_eq!(collected, vec![Collected::ClientDone]);
    }
}
