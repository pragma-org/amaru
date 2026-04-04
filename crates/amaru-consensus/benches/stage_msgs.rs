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

use std::{hint::black_box, time::Duration};

use amaru_consensus::stages::{
    adopt_chain::AdoptChainMsg,
    fetch_blocks::FetchBlocksMsg,
    select_chain::SelectChainMsg,
    track_peers::{DeferReqNextMsg, TrackPeersMsg},
    validate_block::ValidateBlockMsg,
};
use amaru_kernel::{
    BlockHeader, BlockHeight, EraName, Point, TESTNET_ERA_HISTORY, Tip, any_header_hash,
    cardano::network_block::{NetworkBlock, make_block},
    make_header,
    utils::tests::run_strategy,
};
use amaru_ouroboros::ConnectionId;
use amaru_protocols::chainsync::{ChainSyncInitiatorMsg, HeaderContent, InitiatorMessage, InitiatorResult};
use criterion::{Criterion, criterion_group, criterion_main};
use pure_stage::{SendData, serde::to_cbor};

fn stage_msgs(c: &mut Criterion) {
    let mut group = c.benchmark_group("Stage Messages");
    group.measurement_time(Duration::from_secs(5));

    let point = Point::Specific(1_234_567_890.into(), run_strategy(any_header_hash()));
    let bh = BlockHeight::from(123_456_789);
    let tip = Tip::new(point, bh);

    let msg = ValidateBlockMsg::new(tip, point, bh);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("ValidateBlockMsg", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = AdoptChainMsg::new(tip, bh);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("AdoptChainMsg", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = SelectChainMsg::TipFromUpstream(tip, point);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("SelectChainMsg::TipFromUpstream", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = SelectChainMsg::BlockValidationResult(tip, true);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("SelectChainMsg::BlockValidationResult", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = SelectChainMsg::FetchNextFrom(point);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("SelectChainMsg::FetchNextFrom", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = FetchBlocksMsg::NewTip(tip, point);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("FetchBlocksMsg::NewTip", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = FetchBlocksMsg::Timeout(1000);
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("FetchBlocksMsg::Timeout", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let block = make_block();
    #[allow(clippy::expect_used)]
    let nb = NetworkBlock::new(&TESTNET_ERA_HISTORY, &block).expect("minimal network block");
    let header = BlockHeader::from(make_header(1234, 12345, None));
    let header_content = HeaderContent::new(&header, EraName::Conway);

    let msg = DeferReqNextMsg::Poll;
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("DeferReqNextMsg::Poll", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = FetchBlocksMsg::Block(nb.clone());
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("FetchBlocksMsg::Block", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: amaru_kernel::Peer { name: "peer1".to_string() },
        conn_id: ConnectionId::initial(),
        handler: pure_stage::StageRef::<InitiatorMessage>::named_for_tests("test"),
        msg: InitiatorResult::Initialize,
    });
    let msg: Box<dyn SendData> = Box::new(msg);
    group.bench_function("TrackPeersMsg::FromUpstream", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: amaru_kernel::Peer { name: "peer1".to_string() },
        conn_id: ConnectionId::initial(),
        handler: pure_stage::StageRef::<InitiatorMessage>::named_for_tests("test"),
        msg: InitiatorResult::RollForward(header_content.clone(), tip),
    });
    let msg: Box<dyn SendData> = Box::new(msg);
    group
        .bench_function("TrackPeersMsg::FromUpstream(RollForward)", |b| b.iter(|| black_box(to_cbor(black_box(&msg)))));

    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10));
    targets = stage_msgs
);
criterion_main!(benches);
