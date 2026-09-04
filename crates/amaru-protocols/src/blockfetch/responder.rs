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
//! `Repeat<Send<Block>>` for the bodies, then `BatchDone` and `WantNext`.

use amaru_kernel::{IsHeader, NetworkPoint, NonEmptyVec, Peer, Point, RawBlock};
use amaru_metrics::protocol::ServedBlockCountMetrics;
use amaru_observability::{debug, error};
use amaru_pure_stage::{
    DeserializerGuards, Effects, StageRef, Void, define_role_tag, make_states, on_receive, typestate::prelude::*,
};

use super::{BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch};
use crate::{
    metrics_effects::{Metrics, MetricsOps},
    mux::{Frame, HandlerMessage, MuxMessage},
    protocol::{Inputs, Internal, MuxClient, PROTO_N2N_BLOCK_FETCH, Pull, ToMux, WantNext, from_wire},
    store_effects::Store,
};

/// Maximum number of blocks that can be streamed for a single request
pub const MAX_FETCHED_BLOCKS: usize = 1000;

make_states!(pub Proto { Idle; Done });

define_role_tag!(pub ToInitiator);

on_receive!(Idle as ServerIdleIn {
    Pull => { Send<ToMux, WantNext> => Idle }
    RequestRange => {
        Send<ToInitiator, StartBatch>, Repeat<Send<ToInitiator, Block>>, Send<ToInitiator, BatchDone>, Send<ToMux, WantNext> => Idle
        | Send<ToInitiator, NoBlocks>, Send<ToMux, WantNext> => Idle
    }
    ClientDone => { Done }
});

/// Range of points to fetch, newest first, at least one point.
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub struct PointsRange(NonEmptyVec<Point>);

type Mail = Inputs<Void>;

impl PointsRange {
    /// Create a points range with a single point
    pub fn singleton(first: Point) -> PointsRange {
        PointsRange(NonEmptyVec::singleton(first))
    }

    /// Create a points range from a vector of points.
    pub fn from_vec(vec: Vec<Point>) -> Option<PointsRange> {
        NonEmptyVec::try_from(vec).ok().map(PointsRange)
    }

    #[cfg(test)]
    pub fn points(&self) -> Vec<Point> {
        self.0.to_vec()
    }

    /// Load the first available block in the current range (the block is expected to be found).
    /// Each time we attempt to fetch a block we pop its point from the current_range.
    async fn next_block(self, store: &Store) -> anyhow::Result<(RawBlock, Option<PointsRange>)> {
        // points are stored from most recent to oldest, so we pop from the end
        let (last, rest) = self.0.pop();
        let last_hash = last.hash();
        let stored_block =
            store.load_block(&last_hash).await?.ok_or_else(|| anyhow::anyhow!("block {} was pruned", last_hash))?;
        Ok((stored_block, rest.map(PointsRange)))
    }

    /// Return a points range:
    ///  - Check that `from` <= `through`
    ///  - Check that there is a valid path of block from `from` to `through` in the chain store.
    ///  - Check that we don't return too many headers to avoid getting over the protocol limits.
    ///  - Return None if any of the above checks fail and return the points range otherwise.
    pub async fn request_range(
        store: &Store,
        from: NetworkPoint,
        through: NetworkPoint,
    ) -> anyhow::Result<Option<PointsRange>> {
        // make sure that from <= through
        if from > through {
            debug!(protocols::blockfetch::responder::RANGE_REFUSED, from, through, reason = "inverted_range");
            return Ok(None);
        };

        if from == through {
            return if let Some(block) = store.load_block(&from.hash()).await? {
                match block.decode_header() {
                    Ok(header) => Ok(Some(PointsRange::singleton(header.point()))),
                    Err(_) => Ok(None),
                }
            } else {
                Ok(None)
            };
        }

        let mut current_hash = through.hash();
        let mut result = vec![];
        loop {
            if result.len() >= MAX_FETCHED_BLOCKS {
                debug!(
                    protocols::blockfetch::responder::RANGE_REFUSED,
                    from,
                    through,
                    reason = "exceeds_max_blocks",
                    max_blocks = MAX_FETCHED_BLOCKS
                );
                return Ok(None);
            }
            let Some(block) = store.load_block(&current_hash).await? else {
                return Ok(None);
            };
            if let Ok(header) = block.decode_header() {
                result.push(header.point());
                // if we found the from point, we're done
                if current_hash == from.hash() {
                    break;
                }
                // if we reached a slot before 'from', abort
                if header.slot() < from.slot_or_default() {
                    return Ok(None);
                }
                if let Some(parent_hash) = header.parent_hash() {
                    current_hash = parent_hash
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }
        Ok(PointsRange::from_vec(result))
    }
}

impl IntoRoleMail<ToInitiator, StartBatch> for MuxClient {
    fn encode(&self, start: StartBatch) -> MuxMessage {
        self.encode_send(Message::from(start))
    }
}

impl IntoRoleMail<ToInitiator, NoBlocks> for MuxClient {
    fn encode(&self, no_blocks: NoBlocks) -> MuxMessage {
        self.encode_send(Message::from(no_blocks))
    }
}

impl IntoRoleMail<ToInitiator, Block> for MuxClient {
    fn encode(&self, block: Block) -> MuxMessage {
        self.encode_send(Message::from(block))
    }
}

impl IntoRoleMail<ToInitiator, BatchDone> for MuxClient {
    fn encode(&self, done: BatchDone) -> MuxMessage {
        self.encode_send(Message::from(done))
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
}

impl Instance {
    fn new(mux: MuxClient, peer: Peer) -> Self {
        Self { proto: initial_state::<Idle>().into(), mux, peer }
    }
}

async fn instance(inst: Instance, mail: Mail, eff: Effects<Mail>) -> Instance {
    let mail = match mail {
        Inputs::Network(HandlerMessage::Registered(_)) => Inputs::Internal(Internal::Pull),
        mail @ (Inputs::Local(_) | Inputs::Network(HandlerMessage::FromNetwork(_)) | Inputs::Internal(_)) => mail,
    };
    let Instance { proto, mux, peer } = inst;
    let proto = match proto {
        Proto::Idle(idle) => match idle.convert_input(mail) {
            Ok(ServerIdleIn::Pull(pull)) => idle.receive(pull, eff).send(&mux, WantNext).await.finish().into(),
            Ok(ServerIdleIn::RequestRange(range)) => {
                let store = Store::new(eff.clone());
                match PointsRange::request_range(&store, range.from, range.through).await {
                    Ok(Some(mut points)) => {
                        let metrics_eff = eff.clone();
                        let for_err = eff.clone();
                        let metrics = Metrics::new(&metrics_eff);
                        let mut session = idle.receive(range, eff).send(&mux, StartBatch).await;
                        loop {
                            let (block, rest) = match points.next_block(&store).await {
                                Ok(pair) => pair,
                                Err(err) => return invalid(peer, "Streaming", err, for_err).await,
                            };
                            metrics.record(ServedBlockCountMetrics { count: 1 }.into()).await;
                            session = session.send(&mux, Block { body: block.to_vec() }).await;
                            match rest {
                                Some(next) => points = next,
                                None => break,
                            }
                        }
                        session.discard_repeat().send(&mux, BatchDone).await.send(&mux, WantNext).await.finish().into()
                    }
                    Ok(None) => {
                        idle.receive(range, eff).send(&mux, NoBlocks).await.send(&mux, WantNext).await.finish().into()
                    }
                    Err(err) => return invalid(peer, idle.name(), err, eff).await,
                }
            }
            Ok(ServerIdleIn::ClientDone(done)) => idle.receive(done, eff).finish().into(),
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
    Instance { proto, mux, peer }
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
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<Void> {
    let mux = MuxClient::new(muxer.clone(), PROTO_N2N_BLOCK_FETCH.responder().erase());
    let blockfetch = eff.stage("blockfetch", instance).await;
    let blockfetch = eff.supervise(blockfetch, tombstone);
    let blockfetch = eff.wire_up(blockfetch, Instance::new(mux, peer)).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_BLOCK_FETCH.responder().erase(),
            frame: Frame::OneCborItem,
            handler: blockfetch.contramap(Inputs::Network),
            max_buffer: 2_500_000,
        },
    )
    .await;
    blockfetch.contramap(Inputs::Local)
}

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, OnceLock};

    use amaru_kernel::{
        BlockHeight, EraHistory, EraName, IsHeader, NetworkPoint, NonEmptyBytes, Slot, any_fake_header,
        any_headers_chain, any_headers_chain_with_root,
        cardano::network_block::{EncodedTestBlock, NetworkBlock, make_encoded_chain},
        cbor,
        utils::tests::run_strategy,
    };
    use amaru_ouroboros_traits::{WriteChainStore, in_memory_chain_store::InMemoryChainStore};
    use amaru_pure_stage::{
        StageGraph,
        simulation::{Run, SimulationBuilder, simulation_builder::run_test},
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

    fn star_send<Tag, T>() -> String {
        format!("Repeat<{}>", send_desc::<Tag, T>())
    }

    #[test]
    fn responder_receive_allowances() {
        assert_eq!(remaining::<Idle, Pull>(), format!("{} => Idle", send_desc::<ToMux, WantNext>()));
        assert_eq!(
            remaining::<Idle, RequestRange>(),
            format!(
                "{}, {}, {}, {} => Idle | {}, {} => Idle",
                send_desc::<ToInitiator, StartBatch>(),
                star_send::<ToInitiator, Block>(),
                send_desc::<ToInitiator, BatchDone>(),
                send_desc::<ToMux, WantNext>(),
                send_desc::<ToInitiator, NoBlocks>(),
                send_desc::<ToMux, WantNext>()
            )
        );
        assert_eq!(remaining::<Idle, ClientDone>(), "=> Done");
    }

    #[test]
    fn decode_network_block() {
        let as_hex = "820785828a1a002cc8f51a04994d195820f27eddec5e782552e6ef408cff7c4a27e505fe54c20a717027d97e1c91da9d7c5820064effe4fa426184a911159fa803a9c1092459cd0b8f3e584ef9513955be0f5558201e5d0dcf77643d89a94353493859a21b47672015fb652b51f922617e4b27da8982584042d0edd71e6cac29e45f61eabbcce4f803f2ff78bce9fa295d11cb7c3cddb60f7694faaea787183fd604267d8114b57453493c963c7485405838cd79a261013a5850bc8672b4ff2db478e5b21364bfa9f0a2f5265e5ac56b261ce3dcb7ac57301a8362573eef2ae23eb2540915704534d1c0af8eace59a25c130629af7600b175b5e234b376961e2fd12b37de5213e8eff0304582029571d16f081709b3c48651860077bebf9340abb3fc7133443c54f1f5a5edcf1845820ee1d7c2bd6978e3bc8a47fc478424a9efd797f16813164db292320e3728f6de5091902465840f69f8974108be5df23dd0dad2f0e888e5c1702c35c678f3b7a2802f272666ea8a7c9b9f6e786e761d4cb747159d68b7d8f43bceae6ab4e543795d8aded59c302820a005901c06063a37f6f01765b34bceb2651e40a69e3bc31b35fd6c952415175844132250cdcbafd19c39952f471f7318a5cc3e45f54dadc9067bb6d25dac8b76f0bea5106c2f45235fac710d3e78d259af37fd617ed9e372626c5b080359ba1bf5150df764365e0faedfe66ab7e338f7aec558e0a192f4f744b473fbe669013ade2cd144c7742c3ff1d78002af59b0f1b45807bce21f592d23596c54d37095b52a8f942c763f5f014aa161fc18123054a618e8ecb9256c392c3bebcb30e10b2c4bef64f4c3b0aea29a4378a53b6d061c9000b510c0bf76d87171fb357faeb54087718fea0ee33e048d4a1aa8a831f7f9148ebbbb2d79f58c61268e1e1369ae88e2369e65e57169cc477726944790423f9dee584fb9eceeee79a447c075ada7bceb6a28699f0721415d3d0ab8f20b77410bc5faf296ce126cb73b9aaab208b9844d95d127ccaefac37c323cc1957aad3350c2d176916593aa854be50e7c36857adcf51800d490ce082908c5a1aceb8fd51fffc67abaf2c09c1f957bc2e009b8a76394402211eac5ff26c2e5d69aa2c6f4a0e4f2ac28c1482b4706916a0c876d56952b1db18af64658f6249db7fe7e7e366fd2a0f869472d38edb6145404f556025ea0066228080a080";
        let bytes = hex::decode(as_hex).expect("valid hex");
        let network_block: NetworkBlock = minicbor::decode(&bytes).expect("a valid network block");
        assert_eq!(network_block.era_tag(), EraName::Conway);
    }

    #[test]
    fn test_request_range_invalid_from_greater_than_through() {
        let (store, chain) = make_store_with_chain(5);
        let result = request_range(store, chain[3].header.point(), chain[1].header.point());
        assert_eq!(result, None, "should return None when from > through");
    }

    #[test]
    fn test_request_range_single_point_block_exists() {
        let (store, chain) = make_store_with_chain(3);
        store_blocks(store.clone(), &chain[1..2]);

        let result = request_range(store, chain[1].header.point(), chain[1].header.point());
        assert_eq!(result, Some(PointsRange::singleton(chain[1].header.point())));
    }

    #[test]
    fn test_request_range_single_point_block_missing() {
        let (store, chain) = make_store_with_chain(3);
        let result = request_range(store, chain[1].header.point(), chain[1].header.point());
        assert_eq!(result, None, "should return None when from == through but block doesn't exist");
    }

    #[test]
    fn test_request_range_valid_chain() {
        let (store, chain) = make_store_with_chain(5);
        store_blocks(store.clone(), &chain);
        let result = request_range(store, chain[0].header.point(), chain[4].header.point());
        assert_eq!(
            result,
            PointsRange::from_vec(vec![
                chain[4].header.point(),
                chain[3].header.point(),
                chain[2].header.point(),
                chain[1].header.point(),
                chain[0].header.point(),
            ])
        );
    }

    #[test]
    fn test_request_range_missing_block_in_chain() {
        let (store, chain) = make_store_with_chain(5);

        // Store blocks for all headers except one in the middle
        store_blocks(store.clone(), &chain[..2]);
        store_blocks(store.clone(), &chain[3..]);

        let result = request_range(store, chain[0].header.point(), chain[4].header.point());
        assert_eq!(result, None, "should return None when a block is missing in the chain");
    }

    #[test]
    fn test_request_range_missing_header_in_chain() {
        let chain = make_encoded_chain(run_strategy(any_headers_chain(5)), &EraHistory::default());
        let store = Arc::new(InMemoryChainStore::new());

        store.set_anchor_point(&chain[0].header.point()).unwrap();

        // Store only some headers (skip one in the middle)
        for (i, block) in chain.iter().enumerate() {
            if i != 2 {
                store.store_header(&block.header).unwrap();
                store.roll_forward_chain(&block.header.point()).unwrap();
            }
        }
        store_blocks(store.clone(), &chain[..2]);
        store_blocks(store.clone(), &chain[3..]);

        let result = request_range(store, chain[0].header.point(), chain[4].header.point());
        assert_eq!(result, None, "should return None when a header is missing in the chain");
    }

    #[test]
    fn test_request_range_no_parent_hash_before_from() {
        let genesis = NetworkPoint::Specific(Slot::from(10), run_strategy(any_fake_header()).hash());
        let (store, chain) = make_store_with_chain_starting_from(5, genesis);

        let result = request_range(
            store,
            NetworkPoint::Specific(Slot::from(2), run_strategy(any_fake_header()).hash()),
            chain[3].header.point(),
        );
        assert_eq!(result, None, "should return None when we hit genesis before finding from");
    }

    #[test]
    fn test_request_range_slot_before_from_abort() {
        // Create a chain with 5 headers
        let (store, chain) = make_store_with_chain(5);
        store_blocks(store.clone(), &chain);

        // Create a 'from' point that has a slot within the chain range but with a non-existent hash.
        // When traversing backwards from 'through', we'll pass the slot of 'from' without finding it,
        // and then hit a block with a slot before 'from', triggering the abort condition.
        let from_slot = chain[2].header.slot();
        let non_existent_hash = run_strategy(any_fake_header()).hash();
        let from = NetworkPoint::Specific(from_slot, non_existent_hash);

        let result = request_range(store, from, chain[4].header.point());
        assert_eq!(result, None, "should return None when we reach a slot before 'from' without finding 'from'");
    }

    #[test]
    fn test_request_range_exactly_max_blocks() {
        // Create a chain longer than MAX_BLOCKS
        let (store, chain) = make_store_with_chain(MAX_FETCHED_BLOCKS);
        store_blocks(store.clone(), &chain);

        let result = request_range(store, chain[0].header.point(), chain[MAX_FETCHED_BLOCKS - 1].header.point());

        assert_eq!(result.unwrap().points().len(), MAX_FETCHED_BLOCKS);
    }

    #[test]
    fn test_request_range_max_blocks_limit() {
        // Create a chain longer than MAX_BLOCKS
        let chain_length = MAX_FETCHED_BLOCKS + 1;
        let (store, chain) = make_store_with_chain(chain_length);
        store_blocks(store.clone(), &chain);

        let result = request_range(store, chain[0].header.point(), chain[chain_length - 1].header.point());
        assert_eq!(result, None, "should return None when the requested range exceeds MAX_BLOCKS limit");
    }

    #[test]
    fn test_next_block_single_point() {
        let (store, chain) = make_store_with_chain(3);
        store_blocks(store.clone(), &chain);

        let (block, remaining_range) = next_block(store, PointsRange::singleton(chain[1].header.point()));

        // Should return the block for the single point
        let network_block: NetworkBlock = block.try_into().unwrap();
        assert_eq!(network_block.decode_header().unwrap().point(), chain[1].header.point());

        // Should have no remaining range
        assert_eq!(remaining_range, None);
    }

    #[test]
    fn test_next_block_multiple_points() {
        let (store, chain) = make_store_with_chain(5);
        store_blocks(store.clone(), &chain);

        let (block, remaining_range) = next_block(
            store,
            PointsRange::from_vec(vec![chain[2].header.point(), chain[1].header.point(), chain[0].header.point()])
                .unwrap(),
        );

        // Should return the first block
        let network_block: NetworkBlock = block.try_into().unwrap();
        assert_eq!(network_block.decode_header().unwrap().point(), chain[0].header.point());

        // Should have remaining points
        assert_eq!(remaining_range, PointsRange::from_vec(vec![chain[2].header.point(), chain[1].header.point()]));
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
        network.resources().put::<ResourceHeaderStore>(store);
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(handler_b, Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001)));
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
        network.resources().put::<ResourceHeaderStore>(store);
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(handler_b, Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001)));
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

    #[test]
    fn close_idle_goes_done() {
        let mut network = SimulationBuilder::default();
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(handler_b, Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001)));
        network.preload(&handler, [Inputs::Network(HandlerMessage::Registered(proto())), wire(ClientDone)]).unwrap();
        let mut running = network.run(test_runtime());
        running.run(Run::skip_wakeups()).assert_idle();
        let log = running.get_state(&mux).cloned().unwrap();
        assert!(log.sends.is_empty());
        assert_eq!(log.wants, 1);
        assert!(matches!(running.get_state(&handler).unwrap().proto, Proto::Done(_)));
    }

    #[test]
    fn start_batch_while_idle_terminates() {
        let mut network = SimulationBuilder::default();
        let mux = network.stage("mux", mux_step);
        let mux_ref = mux.sender();
        let _mux = network.wire_up(mux, MuxLog::default());
        let handler_b = network.stage("bf", instance);
        let handler = network.wire_up(handler_b, Instance::new(MuxClient::new(mux_ref, proto()), Peer::for_test(3001)));
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

    /// Invoke the PointsRange::request_range method via a stage
    fn request_range(
        store: Arc<InMemoryChainStore>,
        from: impl Into<NetworkPoint>,
        through: impl Into<NetworkPoint>,
    ) -> Option<PointsRange> {
        let from = from.into();
        let through = through.into();
        match run_points_range_test(store, PointsRangeTestMsg::RequestRange { from, through }) {
            PointsRangeTestResult::RequestRange(result) => result,
            PointsRangeTestResult::NextBlock(_) => unreachable!(),
        }
    }

    /// Invoke the PointsRange::next_block method via a stage
    fn next_block(store: Arc<InMemoryChainStore>, range: PointsRange) -> (RawBlock, Option<PointsRange>) {
        match run_points_range_test(store, PointsRangeTestMsg::NextBlock { range }) {
            PointsRangeTestResult::NextBlock(result) => result,
            PointsRangeTestResult::RequestRange(_) => unreachable!(),
        }
    }

    /// Invoke the PointsRange methods via a stage
    fn run_points_range_test(store: Arc<InMemoryChainStore>, msg: PointsRangeTestMsg) -> PointsRangeTestResult {
        run_test(
            |resources| {
                resources.put::<ResourceHeaderStore>(store.clone());
            },
            msg,
            |_state, msg, eff| async move {
                match msg {
                    PointsRangeTestMsg::RequestRange { from, through } => Some(PointsRangeTestResult::RequestRange(
                        PointsRange::request_range(&Store::new(eff.clone()), from, through).await.unwrap(),
                    )),
                    PointsRangeTestMsg::NextBlock { range } => Some(PointsRangeTestResult::NextBlock(
                        range.next_block(&Store::new(eff.clone())).await.unwrap(),
                    )),
                }
            },
        )
        .unwrap()
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    enum PointsRangeTestMsg {
        RequestRange { from: NetworkPoint, through: NetworkPoint },
        NextBlock { range: PointsRange },
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    enum PointsRangeTestResult {
        RequestRange(Option<PointsRange>),
        NextBlock((RawBlock, Option<PointsRange>)),
    }
}
