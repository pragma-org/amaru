#![expect(dead_code)]
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

use super::{ForwardChainStage, ForwardEvent, PrettyPoint};
use crate::stages::PallasPoint;
use acto::{AcTokio, AcTokioRuntime, ActoCell, ActoInput, ActoRuntime};
use amaru_consensus::{
    IsHeader, Nonces,
    consensus::store::{ChainStore, ReadOnlyChainStore, StoreError},
};
use amaru_kernel::{Hash, Header, RawBlock, block::BlockValidationResult, from_cbor};
use gasket::{
    messaging::tokio::ChannelRecvAdapter,
    runtime::{Tether, spawn_stage},
};
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{
        Point,
        chainsync::{NextResponse, Tip},
    },
};
use std::{
    collections::BTreeMap, fs::File, future::Future, path::Path, str::FromStr, sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex, mpsc},
    time::timeout,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone)]
pub struct TestStore(BTreeMap<Hash<32>, Header>);

impl TestStore {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, hash: &Hash<32>) -> Option<&Header> {
        self.0.get(hash)
    }

    pub fn get_chain(&self, h: &str) -> Vec<Header> {
        let mut chain = Vec::new();
        let mut current = hash(h);
        while let Some(header) = self.get(&current) {
            chain.push(header.clone());
            let Some(parent) = header.parent() else {
                break;
            };
            current = parent;
        }
        chain.reverse();
        chain
    }

    pub fn get_tip(&self, h: &str) -> Tip {
        let header = self.get(&hash(h)).unwrap();
        Tip(header.pallas_point(), header.block_height())
    }

    pub fn get_point(&self, h: &str) -> Point {
        let header = self.get(&hash(h)).unwrap();
        header.pallas_point()
    }

    pub fn get_height(&self, h: &str) -> u64 {
        let header = self.get(&hash(h)).unwrap();
        header.block_height()
    }
}

impl ReadOnlyChainStore<Header> for TestStore {
    fn load_header(&self, hash: &Hash<32>) -> Option<Header> {
        self.0.get(hash).cloned()
    }
    fn get_nonces(&self, _header: &Hash<32>) -> Option<Nonces> {
        unimplemented!()
    }

    fn load_block(&self, _hash: &Hash<32>) -> Result<RawBlock, StoreError> {
        unimplemented!()
    }
}

impl ChainStore<Header> for TestStore {
    fn store_header(&mut self, hash: &Hash<32>, header: &Header) -> Result<(), StoreError> {
        self.0.insert(*hash, header.clone());
        Ok(())
    }

    fn put_nonces(&mut self, _header: &Hash<32>, _nonces: &Nonces) -> Result<(), StoreError> {
        unimplemented!()
    }

    fn era_history(&self) -> &amaru_slot_arithmetic::EraHistory {
        unimplemented!()
    }

    fn store_block(
        &mut self,
        _hash: &Hash<32>,
        _block: &amaru_kernel::RawBlock,
    ) -> Result<(), StoreError> {
        unimplemented!()
    }
}

pub const CHAIN_47: &str = "tests/data/chain41.json";
pub const TIP_47: &str = "fcb4a51804f14f3f5b5ad841199b557aed0187280f7855736bdb153b0d202bb6";
pub const LOST_47: &str = "bd41b102018a21e068d504e64b282512a3b7d5c3883b743aa070ad9244691125";
pub const BRANCH_47: &str = "64565f22fb23476baaa6f82e0e2d68636ceadabded697099fb376c23226bdf03";
pub const WINNER_47: &str = "66c90f54f9073cfc03a334f5b15b1617f6bf6fe6c892fad8368e16abe20b0f4f";

pub fn mk_store(path: impl AsRef<Path>) -> TestStore {
    let f = File::open(path).unwrap();
    let json: serde_json::Value = serde_json::from_reader(f).unwrap();
    let headers = json
        .pointer("/stakePools/chains")
        .unwrap()
        .as_array()
        .unwrap();

    let mut store = BTreeMap::new();

    for header in headers {
        let hash = header.pointer("/hash").unwrap().as_str().unwrap();
        let header = header.pointer("/header").unwrap().as_str().unwrap();
        let header = hex::decode(header).unwrap();
        store.insert(hash.parse().unwrap(), minicbor::decode(&header).unwrap());
    }

    TestStore(store)
}

pub fn hash(s: &str) -> Hash<32> {
    Hash::<32>::from_str(s).unwrap()
}

pub fn hex(s: &str) -> Vec<u8> {
    hex::decode(s).unwrap()
}

pub fn point(slot: u64, hash: &str) -> Point {
    Point::Specific(slot, hex(hash))
}

pub fn amaru_point(slot: u64, hash: &str) -> amaru_kernel::Point {
    amaru_kernel::Point::Specific(slot, hex(hash))
}

pub struct Setup {
    pub store: TestStore,
    runtime: AcTokio,
    event: mpsc::Receiver<ForwardEvent>,
    block: mpsc::Sender<gasket::messaging::Message<BlockValidationResult>>,
    _tether: Tether,
    port: u16,
}

impl Setup {
    pub fn new(our_tip: &str) -> Setup {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();

        let store = mk_store(CHAIN_47);
        let runtime = AcTokio::new("test", 1).unwrap();
        let (port_tx, mut port_rx) = mpsc::channel(8);
        let downstream = runtime
            .spawn_actor(
                "test",
                |mut cell: ActoCell<ForwardEvent, AcTokioRuntime>| async move {
                    while let ActoInput::Message(msg) = cell.recv().await {
                        port_tx.send(msg).await.unwrap();
                    }
                },
            )
            .me;
        let (block_tx, block_rx) = mpsc::channel(8);
        let mut stage = ForwardChainStage::new(
            Some(downstream),
            Arc::new(Mutex::new(store.clone())),
            42,
            "127.0.0.1:0",
            1,
            store.get_tip(our_tip),
        );
        stage.upstream.connect(ChannelRecvAdapter::Mpsc(block_rx));
        let tether = spawn_stage(stage, Default::default());

        tracing::info!("stage state: {:?}", tether.check_state());
        let port = block_on(&runtime, port_rx.recv()).unwrap();
        let ForwardEvent::Listening(port) = port else {
            panic!("expected listening event, got {:?}", port);
        };
        assert_ne!(port, 0);
        tracing::info!(
            "stage ({:?}) listening on port {}",
            tether.check_state(),
            port
        );

        Setup {
            store,
            runtime,
            event: port_rx,
            block: block_tx,
            _tether: tether,
            port,
        }
    }

    pub fn send_validated(&mut self, s: &str) {
        let header = self.store.get(&hash(s)).unwrap();
        let point = header.point();
        let block_height = header.block_height();
        let span = tracing::debug_span!("whatever");

        let f = self.block.send(
            BlockValidationResult::BlockValidated {
                point: point.clone(),
                span,
                block: RawBlock::from(&[] as &[u8]),
                block_height,
            }
            .into(),
        );
        tracing::info!("sending block validated");
        block_on(&self.runtime, f).unwrap();
        tracing::info!("waiting for forward event");
        let p = block_on(&self.runtime, self.event.recv()).unwrap();
        let ForwardEvent::Forward(p) = p else {
            panic!("expected forward event, got {:?}", p);
        };
        assert_eq!(p, point.pallas_point());
    }

    pub fn send_backward(&mut self, s: &str) {
        let point = self.store.get(&hash(s)).unwrap().point();
        let span = tracing::debug_span!("whatever");
        let f = self.block.send(
            BlockValidationResult::RolledBackTo {
                rollback_point: point.clone(),
                span,
            }
            .into(),
        );
        tracing::info!("sending block roll backward");
        block_on(&self.runtime, f).unwrap();
        tracing::info!("waiting for backward event");
        let p = block_on(&self.runtime, self.event.recv()).unwrap();
        let ForwardEvent::Backward(p) = p else {
            panic!("expected backward event, got {:?}", p);
        };
        assert_eq!(p, point.pallas_point());
    }

    pub fn connect(&self) -> Client {
        let client = block_on(
            &self.runtime,
            PeerClient::connect(&format!("127.0.0.1:{}", self.port), 42),
        )
        .unwrap();
        Client {
            runtime: self.runtime.clone(),
            client,
        }
    }

    pub fn check_header(&self, s: &str, h: &Header) {
        let header = self.store.get(&hash(s)).unwrap();
        assert_eq!(header.header_body, h.header_body);
    }
}

pub struct Client {
    runtime: AcTokioRuntime,
    client: PeerClient,
}

impl Client {
    pub fn find_intersect(&mut self, points: Vec<Point>) -> (Option<Point>, Tip) {
        block_on(
            &self.runtime,
            self.client.chainsync().find_intersect(points),
        )
        .unwrap()
    }

    pub fn recv_until_await(&mut self) -> Vec<ClientMsg> {
        let mut ops = Vec::new();
        while let Ok(response) = block_on(&self.runtime, self.client.chainsync().request_next()) {
            match response {
                NextResponse::RollForward(header, tip) => {
                    ops.push(ClientMsg::Forward(from_cbor(&header.cbor).unwrap(), tip))
                }
                NextResponse::RollBackward(point, tip) => ops.push(ClientMsg::Backward(point, tip)),
                NextResponse::Await => break,
            }
        }
        ops
    }

    pub fn recv_n<const N: usize>(&mut self) -> [ClientMsg; N] {
        let mut ops = Vec::new();
        for _ in 0..N {
            let msg = block_on(&self.runtime, self.client.chainsync().request_next()).unwrap();
            match msg {
                NextResponse::RollForward(header, tip) => {
                    ops.push(ClientMsg::Forward(from_cbor(&header.cbor).unwrap(), tip))
                }
                NextResponse::RollBackward(point, tip) => ops.push(ClientMsg::Backward(point, tip)),
                NextResponse::Await => break,
            }
        }
        ops.try_into().unwrap()
    }

    pub fn recv_after_await(&mut self) -> ClientMsg {
        let msg = block_on(
            &self.runtime,
            self.client.chainsync().recv_while_can_await(),
        )
        .unwrap();
        match msg {
            NextResponse::RollForward(header, tip) => {
                ClientMsg::Forward(from_cbor(&header.cbor).unwrap(), tip)
            }
            NextResponse::RollBackward(point, tip) => ClientMsg::Backward(point, tip),
            NextResponse::Await => panic!("unexpected await"),
        }
    }
}

#[derive(Clone)]
pub enum ClientMsg {
    Forward(Header, Tip),
    Backward(Point, Tip),
}

impl std::fmt::Debug for ClientMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Forward(header, tip) => f
                .debug_struct("Forward")
                .field(
                    "header",
                    &(header.block_height(), PrettyPoint(&header.pallas_point())),
                )
                .field("tip", &(tip.1, PrettyPoint(&tip.0)))
                .finish(),
            Self::Backward(point, tip) => f
                .debug_struct("Backward")
                .field("point", &PrettyPoint(point))
                .field("tip", &(tip.1, PrettyPoint(&tip.0)))
                .finish(),
        }
    }
}

impl PartialEq for ClientMsg {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ClientMsg::Forward(lh, lt), ClientMsg::Forward(rh, rt)) => {
                lh == rh && (&lt.0, lt.1) == (&rt.0, rt.1)
            }
            (ClientMsg::Backward(lp, lt), ClientMsg::Backward(rp, rt)) => {
                lp == rp && (&lt.0, lt.1) == (&rt.0, rt.1)
            }
            _ => false,
        }
    }
}
fn block_on<F: Future>(runtime: &AcTokioRuntime, f: F) -> F::Output {
    runtime
        .with_rt(|rt| {
            let _x = rt.enter();
            rt.block_on(timeout(Duration::from_secs(1), f))
        })
        .unwrap()
        .unwrap()
}
