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

use super::chain_follower::tests::ChainStoreExt;
use super::test_infra::{ClientMsg, FORK_47, LOST_47, TIP_47, TestChainForwarder, WINNER_47, hash};
use crate::stages::AsTip;
use crate::stages::consensus::forward_chain::to_pallas_tip;
use amaru_kernel::IsHeader;
use amaru_kernel::Point;
use amaru_network::point::to_network_point;

#[tokio::test]
async fn test_chain_sync() {
    let mut chain_forwarder = TestChainForwarder::new(LOST_47).await.unwrap();
    let mut client = chain_forwarder.connect().await;
    let chain = chain_forwarder.store.get_chain(TIP_47);
    let header142 = chain[6].clone();
    let (point, tip) = client
        .find_intersect(vec![to_network_point(header142.point())])
        .await;

    let lost = chain_forwarder
        .store
        .load_header(&hash(LOST_47))
        .unwrap()
        .clone();
    assert_eq!(point, Some(chain_forwarder.store.get_point(FORK_47)));
    assert_eq!(tip.0, to_network_point(lost.point()));
    assert_eq!(tip.1, lost.block_height().as_u64());

    let headers = client.recv_until_await().await;
    assert_eq!(
        headers,
        vec![ClientMsg::Backward(
            to_network_point(header142.point()),
            to_pallas_tip(lost.as_tip())
        )]
    );

    chain_forwarder.send_backward(FORK_47).await;
    chain_forwarder.send_forward(WINNER_47).await;
    chain_forwarder
        .send_forward(&chain[8].hash().to_string())
        .await;
    let msg = client.recv_after_await().await;
    assert_eq!(
        msg,
        // out tip comes out as chain[6] here because previously client.recv_until_await already
        // asked for the next op, which means the Backward got sent before the Forward
        // updated the `our_tip` pointer
        ClientMsg::Backward(
            to_network_point(header142.point()),
            to_pallas_tip(chain[6].as_tip())
        )
    );

    let headers = client.recv_until_await().await;
    assert_eq!(
        headers,
        vec![
            ClientMsg::Forward(chain[7].clone(), to_pallas_tip(chain[8].as_tip())),
            ClientMsg::Forward(chain[8].clone(), to_pallas_tip(chain[8].as_tip())),
        ]
    );
}

#[tokio::test]
async fn test_sync_optimising_rollback() {
    let mut setup = TestChainForwarder::new(LOST_47).await.unwrap();

    let mut client = setup.connect().await;
    client
        .find_intersect(vec![])
        .await
        .0
        .expect("no intersection");

    let msgs = client.recv_n::<5>().await;
    let chain = setup.store.get_chain(TIP_47);
    let lost = setup.store.load_header(&hash(LOST_47)).unwrap().clone();
    assert_eq!(
        msgs,
        [
            ClientMsg::Backward(
                to_network_point(Point::Origin),
                to_pallas_tip(lost.as_tip())
            ),
            ClientMsg::Forward(chain[0].clone(), to_pallas_tip(lost.as_tip())),
            ClientMsg::Forward(chain[1].clone(), to_pallas_tip(lost.as_tip())),
            ClientMsg::Forward(chain[2].clone(), to_pallas_tip(lost.as_tip())),
            ClientMsg::Forward(chain[3].clone(), to_pallas_tip(lost.as_tip())),
        ]
    );

    setup.send_backward(FORK_47).await;
    setup.send_forward(&chain[7].hash().to_string()).await;
    setup.send_forward(&chain[8].hash().to_string()).await;
    setup.send_forward(&chain[9].hash().to_string()).await;

    let msgs = client.recv_until_await().await;
    assert_eq!(
        msgs,
        [
            ClientMsg::Forward(chain[4].clone(), to_pallas_tip(chain[9].as_tip())),
            ClientMsg::Forward(chain[5].clone(), to_pallas_tip(chain[9].as_tip())),
            ClientMsg::Forward(chain[6].clone(), to_pallas_tip(chain[9].as_tip())),
            ClientMsg::Forward(chain[7].clone(), to_pallas_tip(chain[9].as_tip())),
            ClientMsg::Forward(chain[8].clone(), to_pallas_tip(chain[9].as_tip())),
            ClientMsg::Forward(chain[9].clone(), to_pallas_tip(chain[9].as_tip())),
        ]
    );
}
