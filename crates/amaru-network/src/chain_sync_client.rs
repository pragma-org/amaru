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

use crate::{point::to_network_point, session::PeerSession};
use amaru_consensus::{RawHeader, consensus::ChainSyncEvent};
use amaru_kernel::Point;
use amaru_kernel::cbor;
use amaru_kernel::network::NetworkName;
use amaru_kernel::peer::Peer;
use pallas_codec::Fragment;
use pallas_network::miniprotocols::Point as NetworkPoint;
use pallas_network::miniprotocols::chainsync::{
    Client, ClientError, HeaderContent, IntersectResponse, Message, NextResponse, Tip,
};
use pallas_network::miniprotocols::chainsync::{ClientError, HeaderContent, NextResponse, Tip};
use pallas_traverse::MultiEraHeader;
use std::future::Future;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;
use tracing::{Level, Span, instrument};

const MAX_BATCH_SIZE: usize = 10;

#[derive(Debug, thiserror::Error)]
pub enum ChainSyncClientError {
    #[error("Failed to decode header: {0}")]
    HeaderDecodeError(String),
    #[error("Network error: {0}")]
    NetworkError(ClientError),
    #[error("No intersection found for points: {points:?}")]
    NoIntersectionFound { points: Vec<Point> },
}

pub fn to_traverse(header: &HeaderContent) -> Result<MultiEraHeader<'_>, ChainSyncClientError> {
    let out = match header.byron_prefix {
        Some((subtag, _)) => MultiEraHeader::decode(header.variant, Some(subtag), &header.cbor),
        None => MultiEraHeader::decode(header.variant, None, &header.cbor),
    };

    out.map_err(|e| ChainSyncClientError::HeaderDecodeError(e.to_string()))
}

#[async_trait::async_trait(?Send)]
pub trait ChainSync<C> {
    async fn recv_while_can_await(&mut self) -> Result<NextResponse<C>, ClientError>;

    async fn find_intersect(
        &mut self,
        points: Vec<NetworkPoint>,
    ) -> Result<IntersectResponse, ClientError>;
}

#[async_trait::async_trait(?Send)]
impl<C> ChainSync<C> for Client<C>
where
    Message<C>: Fragment,
{
    async fn recv_while_can_await(&mut self) -> Result<NextResponse<C>, ClientError> {
        self.recv_while_can_await().await
    }

    async fn find_intersect(
        &mut self,
        points: Vec<NetworkPoint>,
    ) -> Result<IntersectResponse, ClientError> {
        self.find_intersect(points).await
    }
}

#[async_trait::async_trait(?Send)]
impl<C> ChainSync<C> for &mut Client<C>
where
    Message<C>: Fragment,
{
    async fn recv_while_can_await(&mut self) -> Result<NextResponse<C>, ClientError> {
        self.recv_while_can_await().await
    }

    async fn find_intersect(
        &mut self,
        points: Vec<NetworkPoint>,
    ) -> Result<IntersectResponse, ClientError> {
        self.find_intersect(points).await
    }
}

/// Handles chain synchronization network operations
pub struct ChainSyncClient<C> {
    peer: Peer,
    client: Box<dyn ChainSync<C> + Sync + Send>,
    intersection: Vec<Point>,
}

struct PullBuffer {
    buffer: Vec<HeaderContent>,
}

impl PullBuffer {
    fn new() -> Self {
        PullBuffer { buffer: vec![] }
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }

    fn forward(&mut self, header: HeaderContent) {
        self.buffer.push(header)
    }

    fn rollback(&mut self, _point: &NetworkPoint) -> RollbackHandling {
        RollbackHandling::Handled
    }
}

enum RollbackHandling {
    Handled,
    BeforeBatch,
}

#[derive(Debug)]
pub enum PullResult {
    ForwardBatch(Vec<HeaderContent>),
    RollBack(Point),
    Nothing,
}

trait NetworkHeader {
    fn content(self) -> HeaderContent;
}

impl NetworkHeader for HeaderContent {
    fn content(self) -> HeaderContent {
        self
    }
}

impl<C: NetworkHeader> ChainSyncClient<C> {
    pub fn new(
        _peer_session: PeerSession,
        _intersection: Vec<Point>,
        _catching_up: Arc<RwLock<bool>>,
    ) -> Self {
        unimplemented!()
    }

    pub async fn find_intersection(&mut self) -> Result<(), ChainSyncClientError> {
        let points: Vec<NetworkPoint> = self
            .intersection
            .iter()
            .cloned()
            .map(to_network_point)
            .collect();

        let point = self
            .client
            .find_intersect(points)
            .await
            .map_err(ChainSyncClientError::NetworkError)?
            .0;

        point.ok_or(ChainSyncClientError::NoIntersectionFound {
            points: self.intersection.clone(),
        })?;
        Ok(())
    }

    pub fn roll_forward(
        &mut self,
        header: &HeaderContent,
    ) -> Result<ChainSyncEvent, ChainSyncClientError> {
        let header = to_traverse(header)?;
        let point = Point::Specific(header.slot(), header.hash().to_vec());

        let raw_header: RawHeader = header.cbor().to_vec();

        let event = ChainSyncEvent::RollForward {
            peer: self.peer.clone(),
            point,
            raw_header,
            span: Span::current(),
        };

        Ok(event)
    }

    pub fn roll_back(
        &self,
        rollback_point: Point,
        _tip: Tip,
    ) -> Result<ChainSyncEvent, ChainSyncClientError> {
        let peer = &self.peer;
        Ok(ChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point,
            span: Span::current(),
        })
    }

    pub async fn pull_batch(&mut self) -> Result<PullResult, ChainSyncClientError> {
        let mut batch = PullBuffer::new();

        while batch.len() < MAX_BATCH_SIZE {
            let response = self
                .client
                .recv_while_can_await()
                .await
                .map_err(|_| unimplemented!())?;

            match response {
                NextResponse::RollForward(content, _tip) => batch.forward(content.content()),
                NextResponse::RollBackward(point, _tip) => match batch.rollback(&point) {
                    RollbackHandling::Handled => (),
                    RollbackHandling::BeforeBatch => {
                        return Ok(PullResult::RollBack(crate::point::from_network_point(
                            &point,
                        )));
                    }
                },
                NextResponse::Await => break,
            }
        }

        Ok(PullResult::ForwardBatch(batch.buffer))
    }

    // pub async fn await_next(
    //     &mut self,
    // ) -> Result<NextResponse<HeaderDecodeError>, ChainSyncClientError> {
    //     self.no_longer_catching_up();
    //     let mut peer_client = self.peer_session.lock().await;
    //     let client = (*peer_client).chainsync();

    //     match client.recv_while_must_reply().await {
    //         Ok(result) => Ok(result),
    //         Err(e) => {
    //             tracing::error!(reason = %e, "failed while awaiting for next block");
    //             Err(ChainSyncClientError::NetworkError(e))
    //         }
    //     }
    // }

    // pub async fn has_agency(&self) -> bool {
    //     let mut peer_client = self.peer_session.lock().await;
    //     let client = (*peer_client).chainsync();
    //     client.has_agency()
    // }

    pub fn new1(client: Box<dyn ChainSync<C> + Sync + Send>, intersection: &Vec<Point>) -> Self {
        ChainSyncClient {
            client,
            peer: Peer::new("alice"),
            intersection: intersection.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_consensus::consensus::chain_selection::generators::generate_headers_anchored_at;
    use amaru_kernel::Point;
    use amaru_ouroboros::fake::FakeHeader;
    use amaru_ouroboros_traits::IsHeader;
    use async_trait::async_trait;
    use pallas_network::miniprotocols::Point as NetworkPoint;
    use pallas_network::miniprotocols::chainsync::{
        ClientError, HeaderContent, IntersectResponse, NextResponse, Tip,
    };
    use tokio::sync::Mutex;

    use crate::{
        chain_sync_client::{ChainSyncClient, PullResult},
        point::to_network_point,
        session::PeerSession,
    };

    use super::{ChainSync, NetworkHeader};

    struct FakeContent(FakeHeader);

    impl NetworkHeader for FakeContent {
        fn content(self) -> HeaderContent {
            HeaderContent {
                variant: 6,
                byron_prefix: None,
                cbor: vec![],
            }
        }
    }

    struct MockNetworkClient {
        responses: Vec<NextResponse<FakeContent>>,
        intersection: Option<IntersectResponse>,
    }

    #[async_trait::async_trait(?Send)]
    impl ChainSync<FakeContent> for MockNetworkClient {
        async fn recv_while_can_await(&mut self) -> Result<NextResponse<FakeContent>, ClientError> {
            if !self.responses.is_empty() {
                let next = self.responses.pop().unwrap();
                Ok(next)
            } else {
                Ok(NextResponse::Await)
            }
        }

        async fn find_intersect(
            &mut self,
            points: Vec<NetworkPoint>,
        ) -> Result<IntersectResponse, ClientError> {
            self.intersection
                .clone()
                .ok_or(ClientError::IntersectionNotFound)
        }
    }

    impl MockNetworkClient {
        fn new(
            responses: Vec<NextResponse<FakeContent>>,
            intersection: Option<IntersectResponse>,
        ) -> Self {
            Self {
                responses,
                intersection,
            }
        }
    }

    #[tokio::test]
    async fn batch_all_headers_from_forward() {
        let headers = generate_headers_anchored_at(None, 3);
        let tip_header = headers[2];
        let contents: Vec<FakeContent> = headers.into_iter().map(FakeContent).collect();

        let mock_client = MockNetworkClient::new(
            contents
                .into_iter()
                .map(|content| {
                    NextResponse::RollForward(
                        content,
                        Tip(
                            to_network_point(tip_header.point()),
                            tip_header.block_height(),
                        ),
                    )
                })
                .collect(),
            None,
        );
        let mut chain_sync_client =
            ChainSyncClient::new1(Box::new(mock_client), &vec![Point::Origin]);

        let result = chain_sync_client.pull_batch().await.unwrap();

        match result {
            PullResult::ForwardBatch(header_contents) => assert_eq!(
                3, // PullResult::ForwardBatch(contents.iter().map(|c| c.content()).collect()),
                header_contents.len()
            ),
            PullResult::RollBack(point) => {
                panic!("expected batch of headers, got rollback {}", point)
            }
            PullResult::Nothing => panic!("got Nothing, expected a batch of headers"),
        }
    }

    #[tokio::test]
    async fn intersect_succeeds_given_underlying_client_succeeds() {
        let expected_intersection = (
            Some(to_network_point(Point::Origin)),
            Tip(to_network_point(Point::Origin), 0),
        );
        let mock_client = MockNetworkClient::new(vec![], Some(expected_intersection));

        let chain_sync_client = ChainSyncClient::new1(Box::new(mock_client), &vec![Point::Origin]);

        let result = chain_sync_client.find_intersection().await.unwrap();

        assert_eq!((), result);
    }
}
