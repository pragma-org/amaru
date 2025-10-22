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

use crate::stages::PallasPoint;
use crate::stages::consensus::forward_chain::client_protocol::PrettyPoint;
use crate::stages::consensus::forward_chain::tcp_forward_chain_server::TcpForwardChainServer;
use amaru_consensus::consensus::effects::{ForwardEvent, ForwardEventListener};
use amaru_consensus::consensus::tip::AsHeaderTip;
use amaru_kernel::{Hash, Header, HeaderHash, from_cbor};
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros_traits::{BlockHeader, ChainStore, IsHeader};
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{
        Point,
        chainsync::{NextResponse, Tip},
    },
};
use std::{fs::File, path::Path, str::FromStr, sync::Arc};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

pub const CHAIN_47: &str = "tests/data/chain41.json";
pub const TIP_47: &str = "fcb4a51804f14f3f5b5ad841199b557aed0187280f7855736bdb153b0d202bb6";
pub const LOST_47: &str = "bd41b102018a21e068d504e64b282512a3b7d5c3883b743aa070ad9244691125";
pub const FORK_47: &str = "64565f22fb23476baaa6f82e0e2d68636ceadabded697099fb376c23226bdf03";
pub const WINNER_47: &str = "66c90f54f9073cfc03a334f5b15b1617f6bf6fe6c892fad8368e16abe20b0f4f";

pub fn mk_store(path: impl AsRef<Path>) -> Arc<dyn ChainStore<BlockHeader>> {
    let f = File::open(path).unwrap();
    let json: serde_json::Value = serde_json::from_reader(f).unwrap();
    let headers = json
        .pointer("/stakePools/chains")
        .unwrap()
        .as_array()
        .unwrap();

    let store = InMemConsensusStore::new();
    let mut anchor_set = false;

    for header in headers {
        let header = header.pointer("/header").unwrap().as_str().unwrap();
        let header = hex::decode(header).unwrap();
        let header: BlockHeader = minicbor::decode(&header).unwrap();
        if !anchor_set {
            store.set_anchor_hash(&header.hash()).unwrap();
            anchor_set = true
        }
        store.store_header(&header).unwrap();
    }
    Arc::new(store)
}

pub fn hash(s: &str) -> HeaderHash {
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
    pub store: Arc<dyn ChainStore<BlockHeader>>,
    listener: TcpForwardChainServer<BlockHeader>,
    port: u16,
}

impl Setup {
    pub async fn new(our_tip: &str) -> anyhow::Result<Setup> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();

        let store = mk_store(CHAIN_47);
        let header = store.load_header(&hash(our_tip)).unwrap();

        let tcp_listener = TcpListener::bind("127.0.0.1:0").await?;

        let port = tcp_listener.local_addr()?.port();
        let listener = TcpForwardChainServer::create(
            store.clone(),
            tcp_listener,
            42,
            1,
            header.as_header_tip(),
        )?;

        Ok(Setup {
            store,
            listener,
            port,
        })
    }

    pub async fn send_forward(&mut self, s: &str) {
        let header = self.store.load_header(&hash(s)).unwrap();
        tracing::info!("sending forward event");
        self.listener
            .send(ForwardEvent::Forward(header))
            .await
            .unwrap();
    }

    pub async fn send_backward(&mut self, s: &str) {
        let rollback_header = self.store.load_header(&hash(s)).unwrap();
        tracing::info!("sending rollback event");
        self.listener
            .send(ForwardEvent::Backward(rollback_header.as_header_tip()))
            .await
            .unwrap();
    }

    pub async fn connect(&self) -> Client {
        let client = PeerClient::connect(&format!("127.0.0.1:{}", self.port), 42)
            .await
            .unwrap();
        Client { client }
    }

    pub fn check_header(&self, s: &str, h: &Header) {
        let header = self.store.load_header(&hash(s)).unwrap();
        assert_eq!(header.header_body().clone(), h.header_body);
    }
}

pub struct Client {
    client: PeerClient,
}

impl Client {
    pub async fn find_intersect(&mut self, points: Vec<Point>) -> (Option<Point>, Tip) {
        self.client
            .chainsync()
            .find_intersect(points)
            .await
            .unwrap()
    }

    pub async fn recv_until_await(&mut self) -> Vec<ClientMsg> {
        let mut ops = Vec::new();
        while let Ok(response) = self.client.chainsync().request_next().await {
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

    pub async fn recv_n<const N: usize>(&mut self) -> [ClientMsg; N] {
        let mut ops = Vec::new();
        for _ in 0..N {
            let msg = self.client.chainsync().request_next().await.unwrap();
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

    pub async fn recv_after_await(&mut self) -> ClientMsg {
        let msg = self
            .client
            .chainsync()
            .recv_while_can_await()
            .await
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
    Forward(BlockHeader, Tip),
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
