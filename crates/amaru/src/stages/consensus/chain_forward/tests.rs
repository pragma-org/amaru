use super::{client_state::find_headers_between, ForwardEvent, ForwardStage};
use acto::{AcTokio, AcTokioRuntime, ActoCell, ActoInput, ActoRuntime};
use amaru_consensus::consensus::store::{ChainStore, Nonces, StoreError};
use amaru_kernel::{Hash, Header};
use amaru_ledger::BlockValidationResult;
use gasket::messaging::tokio::ChannelRecvAdapter;
use gasket::runtime::spawn_stage;
use pallas_network::facades::PeerClient;
use pallas_network::miniprotocols::chainsync::{NextResponse, Tip};
use pallas_network::miniprotocols::Point;
use std::future::Future;
use std::sync::Arc;
use std::{collections::HashMap, fs::File, path::Path, str::FromStr};
use tokio::sync::{mpsc, Mutex};
use tracing_subscriber::EnvFilter;

struct TestStore(HashMap<Hash<32>, Header>);

impl TestStore {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn get(&self, hash: &Hash<32>) -> Option<&Header> {
        self.0.get(hash)
    }
}

impl ChainStore<Header> for TestStore {
    fn load_header(&self, hash: &Hash<32>) -> Option<Header> {
        self.0.get(hash).cloned()
    }

    fn store_header(&mut self, hash: &Hash<32>, header: &Header) -> Result<(), StoreError> {
        self.0.insert(*hash, header.clone());
        Ok(())
    }

    fn get_nonces(&self, _header: &Hash<32>) -> Option<Nonces> {
        todo!()
    }

    fn put_nonces(&mut self, _header: &Hash<32>, _nonces: Nonces) -> Result<(), StoreError> {
        todo!()
    }

    fn era_history(&self) -> &slot_arithmetic::EraHistory {
        todo!()
    }
}

fn mk_store(path: impl AsRef<Path>) -> TestStore {
    let f = File::open(path).unwrap();
    let json: serde_json::Value = serde_json::from_reader(f).unwrap();
    let headers = json
        .pointer("/stakePools/chains")
        .unwrap()
        .as_array()
        .unwrap();

    let mut store = HashMap::new();

    for header in headers {
        let hash = header.pointer("/hash").unwrap().as_str().unwrap();
        let header = header.pointer("/header").unwrap().as_str().unwrap();
        let header = hex::decode(header).unwrap();
        store.insert(hash.parse().unwrap(), minicbor::decode(&header).unwrap());
    }

    TestStore(store)
}

fn hash(s: &str) -> Hash<32> {
    Hash::<32>::from_str(s).unwrap()
}

fn hex(s: &str) -> Vec<u8> {
    hex::decode(s).unwrap()
}

const CHAIN_41: &str = "tests/data/chain41.json";

const TIP_41: &str = "fcb4a51804f14f3f5b5ad841199b557aed0187280f7855736bdb153b0d202bb6";
const TIP_41_SLOT: u64 = 990;
const TIP_41_HEIGHT: u64 = 47;

const LOST_41: &str = "bd41b102018a21e068d504e64b282512a3b7d5c3883b743aa070ad9244691125";
const LOST_41_SLOT: u64 = 188;

const WINNER_41: &str = "66c90f54f9073cfc03a334f5b15b1617f6bf6fe6c892fad8368e16abe20b0f4f";
const WINNER_41_SLOT: u64 = 187;
const WINNER_41_HEIGHT: u64 = 8;

const BRANCH_41: &str = "64565f22fb23476baaa6f82e0e2d68636ceadabded697099fb376c23226bdf03";
const BRANCH_41_SLOT: u64 = 142;
const BRANCH_41_HEIGHT: u64 = 7;

const ROOT_41_SLOT: u64 = 31;

fn point(slot: u64, hash: &str) -> Point {
    Point::Specific(slot, hex(hash))
}

fn amaru_point(slot: u64, hash: &str) -> amaru_kernel::Point {
    amaru_kernel::Point::Specific(slot, hex(hash))
}

#[test]
fn test_mk_store() {
    let store = mk_store(CHAIN_41);
    assert_eq!(store.len(), 48);

    let mut current = hash(TIP_41);
    let mut chain = store.get(&current).unwrap().clone();

    assert_eq!(chain.header_body.slot, TIP_41_SLOT);

    while chain.header_body.prev_hash.is_some() {
        current = chain.header_body.prev_hash.unwrap();
        chain = store.get(&current).unwrap().clone();
    }

    assert_eq!(chain.header_body.slot, ROOT_41_SLOT);
}

#[test]
fn find_headers_between_tip_and_tip() {
    let store = mk_store(CHAIN_41);

    let tip = point(TIP_41_SLOT, TIP_41);
    let points = [point(TIP_41_SLOT, TIP_41)];

    let (ops, Tip(p, h)) = find_headers_between(&store, &tip, &points).unwrap();
    assert_eq!((ops, p, h), (vec![], tip, 47));
}

#[test]
fn find_headers_between_tip_and_branch() {
    let store = mk_store(CHAIN_41);

    let tip = point(TIP_41_SLOT, TIP_41);
    let points = [point(BRANCH_41_SLOT, BRANCH_41)];
    let peer = point(BRANCH_41_SLOT, BRANCH_41);

    let (ops, Tip(p, h)) = find_headers_between(&store, &tip, &points).unwrap();
    assert_eq!(
        (ops.len() as u64, p, h),
        (TIP_41_HEIGHT - BRANCH_41_HEIGHT, peer, BRANCH_41_HEIGHT)
    );
}

#[test]
fn find_headers_between_tip_and_branches() {
    let store = mk_store(CHAIN_41);

    let tip = point(TIP_41_SLOT, TIP_41);
    // Note that the below scheme does not match the documented behaviour, which shall pick the first from
    // the list that is on the same chain. But that doesn't make sense to me at all.
    let points = [
        point(BRANCH_41_SLOT, BRANCH_41), // this will lose to the (taller) winner
        point(LOST_41_SLOT, LOST_41),     // this is not on the same chain
        point(WINNER_41_SLOT, WINNER_41), // this is the winner after the branch
    ];
    let peer = point(WINNER_41_SLOT, WINNER_41);

    let (ops, Tip(p, h)) = find_headers_between(&store, &tip, &points).unwrap();
    assert_eq!(
        (ops.len() as u64, p, h),
        (TIP_41_HEIGHT - WINNER_41_HEIGHT, peer, WINNER_41_HEIGHT)
    );
}

#[test]
fn find_headers_between_tip_and_lost() {
    let store = mk_store(CHAIN_41);

    let tip = point(TIP_41_SLOT, TIP_41);
    let points = [point(LOST_41_SLOT, LOST_41)];

    let result = find_headers_between(&store, &tip, &points);
    assert!(result.is_none(), "{result:?}");
}

#[test]
fn test_chain_sync() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    // step 0a: prepare the store
    let store = Arc::new(Mutex::new(mk_store(CHAIN_41)));

    // step 0b: prepare actor to forward downstream traffic
    let runtime = AcTokio::new("test", 1).unwrap();
    let (port_tx, mut port_rx) = mpsc::channel(1);
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

    // step 0c: prepare a little utility
    fn block_on<F: Future>(runtime: &AcTokio, f: F) -> F::Output {
        runtime.with_rt(|rt| rt.block_on(f)).unwrap()
    }

    // step 1: prepare the stage
    let (block_tx, block_rx) = mpsc::channel(1);
    let mut stage = ForwardStage::new(Some(downstream), store, 42, "127.0.0.1:0");
    stage.upstream.connect(ChannelRecvAdapter::Mpsc(block_rx));
    let tether = spawn_stage(stage, Default::default());

    // step 2: wait for the listening event
    println!("stage state 1: {:?}", tether.check_state());
    let port = block_on(&runtime, port_rx.recv()).unwrap();
    let ForwardEvent::Listening(port) = port else {
        panic!("expected listening event, got {:?}", port);
    };
    assert_ne!(port, 0);
    println!("stage state 2: {:?}", tether.check_state());

    // step 3: send the block validated event to inform the stage of the current tip
    let span = tracing::debug_span!("whatever");

    let validated = block_on(&runtime, {
        let block_tx = &block_tx;
        async move {
            println!("sending block validated");
            block_tx
                .send(
                    BlockValidationResult::BlockValidated(amaru_point(TIP_41_SLOT, TIP_41), span)
                        .into(),
                )
                .await
                .unwrap();
            println!("waiting for forward event");
            port_rx.recv().await.unwrap()
        }
    });
    let ForwardEvent::Forward(p) = validated else {
        panic!("expected forward event, got {:?}", validated);
    };
    assert_eq!(p, point(TIP_41_SLOT, TIP_41));

    // step 4a: connect to the stage and prove that it is still alive
    println!("stage state 3: {:?}", tether.check_state());
    let mut client = block_on(
        &runtime,
        PeerClient::connect(&format!("127.0.0.1:{port}"), 42),
    )
    .unwrap();

    // step 4b: find the intersection point
    let (p, t) = block_on(&runtime, {
        client
            .chainsync()
            .find_intersect(vec![point(BRANCH_41_SLOT, BRANCH_41)])
    })
    .unwrap();

    assert_eq!(p, Some(point(BRANCH_41_SLOT, BRANCH_41)));
    assert_eq!(t.0, point(TIP_41_SLOT, TIP_41));
    assert_eq!(t.1, TIP_41_HEIGHT);

    // step 5: pull headers from the stage
    let headers = block_on(&runtime, async move {
        let mut headers = Vec::new();
        while let Ok(response) = client.chainsync().request_next().await {
            match response {
                NextResponse::RollForward(header, _) => headers.push(header),
                NextResponse::RollBackward(_, _) => panic!("unexpected roll backward"),
                NextResponse::Await => break,
            }
        }
        headers
    });
    assert_eq!(headers.len() as u64, TIP_41_HEIGHT - BRANCH_41_HEIGHT);

    // prove that this is still alive - otherwise gasket will kill the stage
    drop(block_tx);

    // Note: there’s no way to shut down the gasket stage without logging to ERRORs, sorry
}
