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

//! BlockFetch responder.
//!
//! Lock-step `instance` with no pipelining. Idle waits for the client (`Pull` /
//! `WantNext`). A served `RequestRange` is one remainder: `StartBatch`, then
//! `Repeat<Call<Block>>` for the bodies, then `BatchDone` and `WantNext`.

use std::time::Duration;

use amaru_kernel::{NetworkPoint, Peer};
use amaru_metrics::protocol::ServedBlockCountMetrics;
use amaru_observability::{debug, error};
use amaru_ouroboros::{ConnectionId, RemoteProtocol};
use amaru_pure_stage::{
    DeserializerGuards, Effects, StageRef, Void, define_role_tag, make_states, on_receive, typestate::prelude::*,
};

use super::{BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch};
use crate::{
    metrics_effects::{Metrics, MetricsOps},
    mux::{Frame, HandlerMessage, MuxMessage, Sent},
    peer_tracking_effects::PeerTrack,
    protocol::{
        Inputs, Internal, MuxClient, NETWORK_SEND_TIMEOUT, PROTO_N2N_BLOCK_FETCH, Pull, ToMux, WantNext, from_wire,
        ingress_limit,
    },
    store_effects::{BlockPointsPage, RangeRequest, Store},
};

/// Number of block points read from the store at a time while serving a range.
///
/// It bounds the memory used to serve a range, not the length of the range: a longer range is served
/// page after page.
const BLOCK_POINTS_PAGE_SIZE: usize = 1000;

make_states!(pub Proto { Idle; Done });

define_role_tag!(pub ToInitiator);

on_receive!(Idle as ServerIdleIn {
    Pull => { Send<ToMux, WantNext> => Idle }
    RequestRange => {
        Call<ToInitiator, StartBatch>, Repeat<Call<ToInitiator, Block>>, Call<ToInitiator, BatchDone>, Send<ToMux, WantNext> => Idle
        | Call<ToInitiator, NoBlocks>, Send<ToMux, WantNext> => Idle
    }
    ClientDone => { Send<ToMux, WantNext> => Done }
});
on_receive!(Done as DoneIn {});

type Mail = Inputs<Void>;

impl<T> IntoRoleCall<ToInitiator, T> for MuxClient
where
    Message: From<T>,
{
    type Reply = Sent;
    const TIMEOUT: Duration = NETWORK_SEND_TIMEOUT;

    fn into_call(self, msg: T) -> (Duration, impl FnOnce(StageRef<Sent>) -> MuxMessage + std::marker::Send + 'static) {
        let message = Message::from(msg);
        (NETWORK_SEND_TIMEOUT, move |reply| self.encode_send(message, reply))
    }
}

impl FromMailbox<Mail> for RequestRange {
    fn from_mailbox(msg: Mail) -> Result<Self, Mail> {
        from_wire::<_, Message, _>(msg)
    }
}

impl FromMailbox<Mail> for ClientDone {
    fn from_mailbox(msg: Mail) -> Result<Self, Mail> {
        from_wire::<_, Message, _>(msg)
    }
}

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<Instance>().boxed(),
        amaru_pure_stage::register_data_deserializer::<MuxClient>().boxed(),
    ]
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Instance {
    proto: Proto,
    mux: MuxClient,
    peer: Peer,
    conn_id: ConnectionId,
}

impl Instance {
    fn new(mux: MuxClient, peer: Peer, conn_id: ConnectionId) -> Self {
        Self { proto: initial_state::<Idle>().into(), mux, peer, conn_id }
    }
}

async fn instance(inst: Instance, mail: Mail, eff: Effects<Mail>) -> Instance {
    let mail = match mail {
        Inputs::Network(HandlerMessage::Registered(_)) => Inputs::Internal(Internal::Pull),
        mail @ (Inputs::Local(_) | Inputs::Network(HandlerMessage::FromNetwork(_)) | Inputs::Internal(_)) => mail,
    };
    let Instance { proto, mux, peer, conn_id } = inst;
    let proto = match proto {
        Proto::Idle(idle) => match idle.convert_input(mail) {
            Ok(ServerIdleIn::Pull(pull)) => idle.receive(&pull, eff).send(&mux, WantNext).await.finish().into(),
            Ok(ServerIdleIn::RequestRange(range)) => {
                let store = Store::new(eff.clone());
                let (from, through) = (range.from, range.through);
                let mut page = if from > through {
                    debug!(protocols::blockfetch::responder::RANGE_REFUSED, from, through, reason = "inverted_range");
                    BlockPointsPage::empty()
                } else {
                    store.get_block_points_range(from, through, RangeRequest::First(BLOCK_POINTS_PAGE_SIZE)).await
                };
                if page.points.is_empty() {
                    let note = eff.clone();
                    let (_, session) = idle.receive(&range, eff).call(&mux, NoBlocks).await;
                    PeerTrack::new(&note).note_remote_protocol(peer, conn_id, RemoteProtocol::BlockFetch, true).await;
                    session.send(&mux, WantNext).await.finish().into()
                } else {
                    let metrics_eff = eff.clone();
                    let for_err = eff.clone();
                    let metrics = Metrics::new(&metrics_eff);
                    let (_, mut session) = idle.receive(&range, eff).call(&mux, StartBatch).await;
                    loop {
                        for point in &page.points {
                            let block = match store.load_block(&point.hash()).await {
                                Ok(Some(block)) => block,
                                Ok(None) => {
                                    let err = anyhow::anyhow!("block {} was pruned", point.hash());
                                    return invalid(peer, "Streaming", err, for_err).await;
                                }
                                Err(err) => return invalid(peer, "Streaming", err, for_err).await,
                            };
                            metrics.record(ServedBlockCountMetrics { count: 1 }.into()).await;
                            (_, session) = session.call(&mux, Block { body: block.to_vec() }).await;
                        }
                        let Some(next) = page.next else { break };
                        let request = RangeRequest::Next { page_size: BLOCK_POINTS_PAGE_SIZE, anchor: page.anchor };
                        page = store.get_block_points_range(NetworkPoint::from(next), through, request).await;
                        if page.points.is_empty() {
                            let err = anyhow::anyhow!("the range ended before {next}");
                            return invalid(peer, "Streaming", err, for_err).await;
                        }
                    }
                    let (_, session) = session.discard_repeat().call(&mux, BatchDone).await;
                    PeerTrack::new(&metrics_eff)
                        .note_remote_protocol(peer, conn_id, RemoteProtocol::BlockFetch, true)
                        .await;
                    session.send(&mux, WantNext).await.finish().into()
                }
            }
            Ok(ServerIdleIn::ClientDone(done)) => {
                // Remainder dest is spec Done; live token restarts Idle on this mux registration.
                let note = eff.clone();
                let _: Done = idle.receive(&done, eff).send(&mux, WantNext).await.finish();
                PeerTrack::new(&note).note_remote_protocol(peer, conn_id, RemoteProtocol::BlockFetch, false).await;
                initial_state::<Idle>().into()
            }
            Err(Inputs::Internal(Internal::Timeout)) => idle.into(),
            Err(mail) => return invalid(peer, idle.name(), mail, eff).await,
        },
        Proto::Done(done) => match mail {
            Inputs::Internal(Internal::Timeout) => done.into(),
            mail @ (Inputs::Local(_) | Inputs::Network(_) | Inputs::Internal(Internal::Pull)) => {
                return invalid(peer, done.name(), mail, eff).await;
            }
        },
    };
    Instance { proto, mux, peer, conn_id }
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

pub async fn register_blockfetch_responder<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<Void> {
    let mux = MuxClient::new(muxer.clone(), PROTO_N2N_BLOCK_FETCH.responder().erase());
    let blockfetch = eff.stage("blockfetch-responder", instance).await;
    let blockfetch = eff.supervise(blockfetch, tombstone);
    let blockfetch = eff.wire_up(blockfetch, Instance::new(mux, peer, conn_id)).await;
    let protocol = PROTO_N2N_BLOCK_FETCH.responder().erase();
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol,
            frame: Frame::OneCborItem,
            handler: blockfetch.contramap(Inputs::Network),
            max_buffer: ingress_limit(protocol),
        },
    )
    .await;
    blockfetch.contramap(Inputs::Local)
}

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, OnceLock};

    use amaru_kernel::{
        BlockHeight, EraHistory, EraName, IsHeader, NetworkPoint, NonEmptyBytes, Point, any_headers_chain_with_root,
        cardano::network_block::{EncodedTestBlock, NetworkBlock, make_encoded_chain},
        cbor,
        utils::tests::run_strategy,
    };
    use amaru_ouroboros_traits::{WriteChainStore, in_memory_chain_store::InMemoryChainStore};
    use amaru_pure_stage::{
        StageGraph,
        simulation::{Run, SimulationBuilder},
        typestate::{FmtPar, OnReceive, Session},
    };
    use tokio::runtime::{Builder, Runtime};

    use super::*;
    use crate::{
        mux::{MuxMessage, Sent},
        protocol::Inputs,
        store_effects::ResourceHeaderStore,
    };

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

    fn call_desc<Tag, T>() -> String {
        format!("Call<{}, {}>", std::any::type_name::<Tag>(), std::any::type_name::<T>())
    }

    fn star_call<Tag, T>() -> String {
        format!("Repeat<{}>", call_desc::<Tag, T>())
    }

    #[test]
    fn responder_receive_allowances() {
        assert_eq!(remaining::<Idle, Pull>(), format!("{} => Idle", send_desc::<ToMux, WantNext>()));
        assert_eq!(
            remaining::<Idle, RequestRange>(),
            format!(
                "{}, {}, {}, {} => Idle | {}, {} => Idle",
                call_desc::<ToInitiator, StartBatch>(),
                star_call::<ToInitiator, Block>(),
                call_desc::<ToInitiator, BatchDone>(),
                send_desc::<ToMux, WantNext>(),
                call_desc::<ToInitiator, NoBlocks>(),
                send_desc::<ToMux, WantNext>()
            )
        );
        assert_eq!(remaining::<Idle, ClientDone>(), format!("{} => Done", send_desc::<ToMux, WantNext>()));
    }

    #[test]
    fn responder_does_not_pipeline() {
        assert!(super::Proto::type_graph().occupancy.is_empty());
    }

    #[test]
    fn decode_network_block() {
        let as_hex = "820785828a1a002cc8f51a04994d195820f27eddec5e782552e6ef408cff7c4a27e505fe54c20a717027d97e1c91da9d7c5820064effe4fa426184a911159fa803a9c1092459cd0b8f3e584ef9513955be0f5558201e5d0dcf77643d89a94353493859a21b47672015fb652b51f922617e4b27da8982584042d0edd71e6cac29e45f61eabbcce4f803f2ff78bce9fa295d11cb7c3cddb60f7694faaea787183fd604267d8114b57453493c963c7485405838cd79a261013a5850bc8672b4ff2db478e5b21364bfa9f0a2f5265e5ac56b261ce3dcb7ac57301a8362573eef2ae23eb2540915704534d1c0af8eace59a25c130629af7600b175b5e234b376961e2fd12b37de5213e8eff0304582029571d16f081709b3c48651860077bebf9340abb3fc7133443c54f1f5a5edcf1845820ee1d7c2bd6978e3bc8a47fc478424a9efd797f16813164db292320e3728f6de5091902465840f69f8974108be5df23dd0dad2f0e888e5c1702c35c678f3b7a2802f272666ea8a7c9b9f6e786e761d4cb747159d68b7d8f43bceae6ab4e543795d8aded59c302820a005901c06063a37f6f01765b34bceb2651e40a69e3bc31b35fd6c952415175844132250cdcbafd19c39952f471f7318a5cc3e45f54dadc9067bb6d25dac8b76f0bea5106c2f45235fac710d3e78d259af37fd617ed9e372626c5b080359ba1bf5150df764365e0faedfe66ab7e338f7aec558e0a192f4f744b473fbe669013ade2cd144c7742c3ff1d78002af59b0f1b45807bce21f592d23596c54d37095b52a8f942c763f5f014aa161fc18123054a618e8ecb9256c392c3bebcb30e10b2c4bef64f4c3b0aea29a4378a53b6d061c9000b510c0bf76d87171fb357faeb54087718fea0ee33e048d4a1aa8a831f7f9148ebbbb2d79f58c61268e1e1369ae88e2369e65e57169cc477726944790423f9dee584fb9eceeee79a447c075ada7bceb6a28699f0721415d3d0ab8f20b77410bc5faf296ce126cb73b9aaab208b9844d95d127ccaefac37c323cc1957aad3350c2d176916593aa854be50e7c36857adcf51800d490ce082908c5a1aceb8fd51fffc67abaf2c09c1f957bc2e009b8a76394402211eac5ff26c2e5d69aa2c6f4a0e4f2ac28c1482b4706916a0c876d56952b1db18af64658f6249db7fe7e7e366fd2a0f869472d38edb6145404f556025ea0066228080a080";
        let bytes = hex::decode(as_hex).expect("valid hex");
        let network_block: NetworkBlock = minicbor::decode(&bytes).expect("a valid network block");
        assert_eq!(network_block.era_tag(), EraName::Conway);
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

    fn install_tracking(network: &mut SimulationBuilder) {
        use amaru_ouroboros::PeerTrackingResource;

        use crate::peer_tracking::InMemoryPeerTracking;

        network.resources().put::<PeerTrackingResource>(Arc::new(InMemoryPeerTracking::new()));
    }

    fn proto() -> crate::protocol::ProtocolId<crate::protocol::Erased> {
        PROTO_N2N_BLOCK_FETCH.responder().erase()
    }

    fn wire(msg: impl Into<Message>) -> Inputs<Void> {
        Inputs::Network(HandlerMessage::FromNetwork(NonEmptyBytes::encode(&msg.into())))
    }

    #[test]
    fn serve_range_sends_blocks_then_batch_done() {
        let (store, chain) = make_store_with_chain(3);
        store_blocks(store.clone(), &chain);
        let mut network = SimulationBuilder::default();
        install_tracking(&mut network);
        network.resources().put::<ResourceHeaderStore>(store);
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(
            handler_b,
            Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001), ConnectionId::initial()),
        );
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(proto())),
                    wire(RequestRange {
                        from: chain[0].header.point().into(),
                        through: chain[2].header.point().into(),
                    }),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::skip_wakeups()).assert_idle();
        let log = running.get_state(&mux).cloned().unwrap();
        assert_eq!(log.sends, vec!["StartBatch", "Block", "Block", "Block", "BatchDone"]);
        assert_eq!(log.wants, 2);
        assert!(matches!(running.get_state(&handler).unwrap().proto, Proto::Idle(_)));
    }

    #[test]
    fn missing_range_sends_no_blocks() {
        let (store, chain) = make_store_with_chain(3);
        let mut network = SimulationBuilder::default();
        install_tracking(&mut network);
        network.resources().put::<ResourceHeaderStore>(store);
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(
            handler_b,
            Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001), ConnectionId::initial()),
        );
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(proto())),
                    wire(RequestRange {
                        from: chain[0].header.point().into(),
                        through: chain[2].header.point().into(),
                    }),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::skip_wakeups()).assert_idle();
        let log = running.get_state(&mux).cloned().unwrap();
        assert_eq!(log.sends, vec!["NoBlocks"]);
        assert_eq!(log.wants, 2);
        assert!(matches!(running.get_state(&handler).unwrap().proto, Proto::Idle(_)));
    }

    /// Run a `RequestRange` through the responder and return what it sent to the mux.
    fn serve(store: Arc<InMemoryChainStore>, from: Point, through: Point) -> MuxLog {
        let mut network = SimulationBuilder::default();
        install_tracking(&mut network);
        network.resources().put::<ResourceHeaderStore>(store);
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(
            handler_b,
            Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001), ConnectionId::initial()),
        );
        network
            .preload(
                &handler,
                [
                    Inputs::Network(HandlerMessage::Registered(proto())),
                    wire(RequestRange { from: from.into(), through: through.into() }),
                ],
            )
            .unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::skip_wakeups()).assert_idle();
        running.get_state(&mux).cloned().unwrap()
    }

    #[test]
    fn a_range_longer_than_one_page_is_served_entirely() {
        let length = 2 * BLOCK_POINTS_PAGE_SIZE + 1;
        let (store, chain) = make_store_with_chain(length);
        store_blocks(store.clone(), &chain);

        let log = serve(store, chain[0].header.point(), chain[length - 1].header.point());

        assert_eq!(log.sends.first().map(String::as_str), Some("StartBatch"));
        assert_eq!(log.sends.iter().filter(|m| *m == "Block").count(), length);
        assert_eq!(log.sends.last().map(String::as_str), Some("BatchDone"));
        assert_eq!(log.sends.len(), length + 2);
    }

    #[test]
    fn an_inverted_range_sends_no_blocks() {
        let (store, chain) = make_store_with_chain(5);
        store_blocks(store.clone(), &chain);

        let log = serve(store, chain[3].header.point(), chain[1].header.point());

        assert_eq!(log.sends, vec!["NoBlocks"]);
    }

    #[test]
    fn close_idle_resets() {
        let mut network = SimulationBuilder::default();
        install_tracking(&mut network);
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(
            handler_b,
            Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001), ConnectionId::initial()),
        );
        network.preload(&handler, [Inputs::Network(HandlerMessage::Registered(proto())), wire(ClientDone)]).unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::skip_wakeups()).assert_idle();
        let log = running.get_state(&mux).cloned().unwrap();
        assert!(log.sends.is_empty());
        assert_eq!(log.wants, 2);
        assert_eq!(remaining::<Idle, ClientDone>(), format!("{} => Done", send_desc::<ToMux, WantNext>()));
        assert!(matches!(running.get_state(&handler).unwrap().proto, Proto::Idle(_)));
    }

    #[test]
    fn start_batch_while_idle_terminates() {
        let mut network = SimulationBuilder::default();
        install_tracking(&mut network);
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let _mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(
            handler_b,
            Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001), ConnectionId::initial()),
        );
        network.preload(&handler, [Inputs::Network(HandlerMessage::Registered(proto())), wire(StartBatch)]).unwrap();
        let mut running = network.run(test_runtime());
        let blocked = running.run(Run::skip_wakeups());
        assert!(matches!(blocked, amaru_pure_stage::simulation::Blocked::Terminated(_)));
    }

    // HELPERS

    fn make_store_with_chain(n: usize) -> (Arc<InMemoryChainStore>, Vec<EncodedTestBlock>) {
        make_store_with_chain_starting_from(n, Point::Origin)
    }

    fn make_store_with_chain_starting_from(
        n: usize,
        point: impl Into<NetworkPoint>,
    ) -> (Arc<InMemoryChainStore>, Vec<EncodedTestBlock>) {
        let chain = make_encoded_chain(
            run_strategy(any_headers_chain_with_root(n, Point::new(point.into(), BlockHeight::from(0)))),
            &EraHistory::default(),
        );
        let store = Arc::new(InMemoryChainStore::new());
        store.set_anchor_point(&chain[0].header.point()).unwrap();
        for block in &chain {
            store.store_header(&block.header).unwrap();
            store.roll_forward_chain(&block.header.point()).unwrap();
        }
        (store, chain)
    }

    fn store_blocks(store: Arc<InMemoryChainStore>, blocks: &[EncodedTestBlock]) {
        for block in blocks {
            store.store_block(&block.header.hash(), &block.raw).unwrap();
        }
    }
}
