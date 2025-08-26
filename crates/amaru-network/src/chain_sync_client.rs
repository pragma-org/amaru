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
use amaru_kernel::peer::Peer;
use pallas_codec::Fragment;
use pallas_network::facades::PeerClient;
use pallas_network::miniprotocols::Point as NetworkPoint;
use pallas_network::miniprotocols::chainsync::{
    Client, ClientError, HeaderContent, IntersectResponse, Message, NextResponse, Tip,
};
use pallas_traverse::MultiEraHeader;
use std::error::Error;
use std::fmt::Debug;
use std::fmt::{self, Display};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::Span;

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
    async fn request_next(&mut self) -> Result<NextResponse<C>, ClientError>;

    async fn recv_while_can_await(&mut self) -> Result<NextResponse<C>, ClientError>;

    async fn recv_while_must_reply(&mut self) -> Result<NextResponse<C>, ClientError>;

    async fn find_intersect(
        &mut self,
        points: Vec<NetworkPoint>,
    ) -> Result<IntersectResponse, ClientError>;

    async fn has_agency(&mut self) -> bool;
}

#[async_trait::async_trait(?Send)]
impl<C> ChainSync<C> for Client<C>
where
    Message<C>: Fragment,
{
    async fn request_next(&mut self) -> Result<NextResponse<C>, ClientError> {
        self.request_next().await
    }

    async fn recv_while_can_await(&mut self) -> Result<NextResponse<C>, ClientError> {
        self.recv_while_can_await().await
    }

    async fn recv_while_must_reply(&mut self) -> Result<NextResponse<C>, ClientError> {
        self.recv_while_must_reply().await
    }

    async fn find_intersect(
        &mut self,
        points: Vec<NetworkPoint>,
    ) -> Result<IntersectResponse, ClientError> {
        self.find_intersect(points).await
    }

    async fn has_agency(&mut self) -> bool {
        Client::has_agency(self)
    }
}

#[async_trait::async_trait(?Send)]
impl ChainSync<HeaderContent> for PeerClient {
    async fn request_next(&mut self) -> Result<NextResponse<HeaderContent>, ClientError> {
        self.chainsync().request_next().await
    }

    async fn recv_while_can_await(&mut self) -> Result<NextResponse<HeaderContent>, ClientError> {
        self.chainsync().recv_while_can_await().await
    }

    async fn recv_while_must_reply(&mut self) -> Result<NextResponse<HeaderContent>, ClientError> {
        self.chainsync().recv_while_must_reply().await
    }

    async fn find_intersect(
        &mut self,
        points: Vec<NetworkPoint>,
    ) -> Result<IntersectResponse, ClientError> {
        self.chainsync().find_intersect(points).await
    }

    async fn has_agency(&mut self) -> bool {
        self.chainsync().has_agency().await
    }
}

#[async_trait::async_trait(?Send)]
impl ChainSync<HeaderContent> for Arc<Mutex<PeerClient>> {
    async fn request_next(&mut self) -> Result<NextResponse<HeaderContent>, ClientError> {
        self.lock().await.chainsync().request_next().await
    }

    async fn recv_while_can_await(&mut self) -> Result<NextResponse<HeaderContent>, ClientError> {
        self.lock().await.chainsync().recv_while_can_await().await
    }

    async fn recv_while_must_reply(&mut self) -> Result<NextResponse<HeaderContent>, ClientError> {
        self.lock().await.chainsync().recv_while_must_reply().await
    }

    async fn find_intersect(
        &mut self,
        points: Vec<NetworkPoint>,
    ) -> Result<IntersectResponse, ClientError> {
        self.lock().await.chainsync().find_intersect(points).await
    }

    async fn has_agency(&mut self) -> bool {
        self.lock().await.chainsync().has_agency().await
    }
}

#[async_trait::async_trait(?Send)]
impl<C> ChainSync<C> for &mut Client<C>
where
    Message<C>: Fragment,
{
    async fn request_next(&mut self) -> Result<NextResponse<C>, ClientError> {
        self.request_next().await
    }

    async fn recv_while_can_await(&mut self) -> Result<NextResponse<C>, ClientError> {
        self.recv_while_can_await().await
    }

    async fn recv_while_must_reply(&mut self) -> Result<NextResponse<C>, ClientError> {
        self.recv_while_must_reply().await
    }

    async fn find_intersect(
        &mut self,
        points: Vec<NetworkPoint>,
    ) -> Result<IntersectResponse, ClientError> {
        self.find_intersect(points).await
    }

    async fn has_agency(&mut self) -> bool {
        Client::has_agency(self)
    }
}

/// Handles chain synchronization network operations
pub struct ChainSyncClient<C> {
    peer: Peer,
    client: Box<dyn ChainSync<C> + Sync + Send>,
    intersection: Vec<Point>,
    max_batch_size: usize,
}

struct PullBuffer<C> {
    buffer: Vec<C>,
}

impl<C: NetworkHeader + Debug> PullBuffer<C> {
    fn new() -> Self {
        PullBuffer { buffer: vec![] }
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }

    fn forward(&mut self, header: C) {
        self.buffer.push(header)
    }

    fn rollback(&mut self, point: &NetworkPoint) -> RollbackHandling {
        let find_index = || {
            for (i, h) in self.buffer.iter().enumerate() {
                if let Ok(header_point) = h.point() {
                    if header_point == *point {
                        return Some(i);
                    }
                }
            }
            None
        };

        match find_index() {
            Some(i) => self.buffer.truncate(i + 1),
            None => return RollbackHandling::BeforeBatch,
        }

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
    RollBack(Point, Tip),
    Nothing,
}

impl Display for PullResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PullResult::ForwardBatch(header_contents) => write!(
                f,
                "Batch: {}",
                header_contents
                    .iter()
                    .map(|HeaderContent { cbor, .. }| hex::encode(cbor))
                    .collect::<Vec<String>>()
                    .join("\n")
            ),
            PullResult::RollBack(point, tip) => write!(f, "Rollback: {}, tip: {:?}", point, tip),
            PullResult::Nothing => write!(f, "No result"),
        }
    }
}

pub trait NetworkHeader {
    fn content(self) -> HeaderContent;
    fn point(&self) -> Result<NetworkPoint, impl Error>;
}

impl NetworkHeader for HeaderContent {
    fn content(self) -> HeaderContent {
        self
    }

    fn point(&self) -> Result<NetworkPoint, impl Error> {
        to_traverse(self)
            .map(|header| NetworkPoint::Specific(header.slot(), header.hash().to_vec()))
    }
}

impl<C: NetworkHeader + Debug> ChainSyncClient<C> {
    pub async fn find_intersection(&mut self) -> Result<(), ChainSyncClientError> {
        let points: Vec<NetworkPoint> = self.intersection.iter().map(to_network_point).collect();

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

        while batch.len() < self.max_batch_size {
            let response = self
                .client
                .request_next()
                .await
                .map_err(ChainSyncClientError::NetworkError)?;

            match response {
                NextResponse::RollForward(content, _tip) => batch.forward(content),
                NextResponse::RollBackward(point, tip) => match batch.rollback(&point) {
                    RollbackHandling::Handled => (),
                    RollbackHandling::BeforeBatch => {
                        return Ok(PullResult::RollBack(
                            crate::point::from_network_point(&point),
                            tip,
                        ));
                    }
                },
                NextResponse::Await => break,
            }
        }

        Ok(PullResult::ForwardBatch(
            batch.buffer.into_iter().map(|h| h.content()).collect(),
        ))
    }

    pub async fn await_next(&mut self) -> Result<NextResponse<C>, ChainSyncClientError> {
        self.client
            .recv_while_can_await()
            .await
            .map_err(ChainSyncClientError::NetworkError)
    }

    pub async fn has_agency(&mut self) -> bool {
        self.client.has_agency().await
    }

}

pub fn new_with_peer(peer: PeerClient, intersection: &[Point]) -> ChainSyncClient<HeaderContent> {
    let client = Box::new(peer);
    ChainSyncClient {
        client,
        peer: Peer::new("alice"),
        intersection: intersection.to_vec(),
        max_batch_size: MAX_BATCH_SIZE,
    }
}

pub fn new_with_session(
    peer: PeerSession,
    intersection: &[Point],
) -> ChainSyncClient<HeaderContent> {
    let client = Box::new(peer.peer_client.clone());
    ChainSyncClient {
        client,
        peer: peer.peer.clone(),
        intersection: intersection.to_vec(),
        max_batch_size: MAX_BATCH_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{ChainSync, ChainSyncClientError, MAX_BATCH_SIZE, NetworkHeader};
    use crate::{
        chain_sync_client::{ChainSyncClient, PullResult},
        point::to_network_point,
    };
    use amaru_consensus::consensus::generators::generate_headers_anchored_at;
    use amaru_kernel::{peer::Peer, to_cbor, Point};
    use amaru_ouroboros::fake::FakeHeader;
    use amaru_ouroboros_traits::IsHeader;
    use pallas_network::miniprotocols::Point as NetworkPoint;
    use pallas_network::miniprotocols::chainsync::{
        ClientError, HeaderContent, IntersectResponse, NextResponse, Tip,
    };

    fn new_from_box<C>(
        client: Box<dyn ChainSync<C> + Sync + Send>,
        intersection: &[Point],
        max_batch_size: usize,
    ) -> ChainSyncClient<C> {
        ChainSyncClient {
            client,
            peer: Peer::new("alice"),
            intersection: intersection.to_vec(),
            max_batch_size,
        }
    }

    #[tokio::test]
    async fn batch_returns_all_available_forwards_until_await() {
        let mock_client = NetworkClientBuilder::new().forward_headers(3).build();
        let mut chain_sync_client =
            new_from_box(Box::new(mock_client), &[Point::Origin], MAX_BATCH_SIZE);

        let result = chain_sync_client.pull_batch().await.unwrap();

        assert!(
            matches!(result,  PullResult::ForwardBatch(header_contents) if 3 == header_contents.len())
        );
    }

    #[tokio::test]
    async fn batch_returns_trimmed_headers_given_rollback_within_batch() {
        let mock_client = NetworkClientBuilder::new()
            .forward_headers(3)
            .rollback_to(1)
            .build();
        let mut chain_sync_client =
            new_from_box(Box::new(mock_client), &[Point::Origin], MAX_BATCH_SIZE);

        let result = chain_sync_client.pull_batch().await.unwrap();

        assert!(
            matches!(result,  PullResult::ForwardBatch(header_contents) if 2 == header_contents.len())
        );
    }

    #[tokio::test]
    async fn batch_returns_rollback_given_point_is_outside_current_batch() {
        let mock_client = NetworkClientBuilder::new()
            .forward_headers(6)
            .rollback_to(1)
            .build();
        let rollback_point = mock_client.point_at(1);
        let mut chain_sync_client =
            new_from_box(Box::new(mock_client), &[Point::Origin], 4);

        // first batch is ignored
        let _ = chain_sync_client.pull_batch().await.unwrap();
        let result = chain_sync_client.pull_batch().await.unwrap();

        assert!(
            matches!(result, PullResult::RollBack(point, _) if rollback_point == to_network_point(&point))
        );
    }

    #[tokio::test]
    async fn intersect_succeeds_given_underlying_client_succeeds() {
        let expected_intersection = (
            Some(to_network_point(&Point::Origin)),
            Tip(to_network_point(&Point::Origin), 0),
        );
        let mock_client = MockNetworkClient::new(vec![], Some(expected_intersection));

        let mut chain_sync_client =
            new_from_box(Box::new(mock_client), &[Point::Origin], MAX_BATCH_SIZE);

        chain_sync_client.find_intersection().await.unwrap();
    }

    // support code

    #[derive(Debug)]
    struct FakeContent(NetworkPoint, FakeHeader);

    impl NetworkHeader for FakeContent {
        fn content(self) -> HeaderContent {
            HeaderContent {
                variant: 6,
                byron_prefix: None,
                cbor: to_cbor(&self.1),
            }
        }

        fn point(&self) -> Result<NetworkPoint, impl Error> {
            Ok::<NetworkPoint, ChainSyncClientError>(self.0.clone())
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
                let next = self.responses.remove(0);
                Ok(next)
            } else {
                Ok(NextResponse::Await)
            }
        }

        async fn recv_while_must_reply(
            &mut self,
        ) -> Result<NextResponse<FakeContent>, ClientError> {
            if !self.responses.is_empty() {
                let next = self.responses.remove(0);
                Ok(next)
            } else {
                // NOTE: supposed to have reached tip
                Ok(NextResponse::Await)
            }
        }

        async fn request_next(&mut self) -> Result<NextResponse<FakeContent>, ClientError> {
            if !self.responses.is_empty() {
                let next = self.responses.remove(0);
                Ok(next)
            } else {
                Ok(NextResponse::Await)
            }
        }

        async fn find_intersect(
            &mut self,
            _points: Vec<NetworkPoint>,
        ) -> Result<IntersectResponse, ClientError> {
            self.intersection
                .clone()
                .ok_or(ClientError::IntersectionNotFound)
        }

        async fn has_agency(&mut self) -> bool {
            unimplemented!()
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

        fn point_at(&self, index: usize) -> NetworkPoint {
            self.responses
                .get(index)
                .map_or(NetworkPoint::Origin, point_of)
        }
    }

    struct NetworkClientBuilder {
        responses: Vec<NextResponse<FakeContent>>,
        intersection: Option<IntersectResponse>,
    }

    impl NetworkClientBuilder {
        fn new() -> Self {
            NetworkClientBuilder {
                responses: vec![],
                intersection: None,
            }
        }

        fn forward_headers(&mut self, num: u32) -> &mut Self {
            let mut responses = generate_forwards(num);
            self.responses.append(&mut responses);
            self
        }

        fn rollback_to(&mut self, index: usize) -> &mut Self {
            let rollback_point = point_of(&self.responses[index]);
            self.responses.push(NextResponse::RollBackward(
                rollback_point.clone(),
                tip_of(&self.responses),
            ));
            self
        }

        fn build(&mut self) -> MockNetworkClient {
            let responses = self.responses.drain(0..).collect();
            MockNetworkClient {
                responses,
                intersection: self.intersection.clone(),
            }
        }
    }

    fn generate_forwards(length: u32) -> Vec<NextResponse<FakeContent>> {
        let headers = generate_headers_anchored_at(None, length);
        let tip_header = headers[2];
        headers
            .into_iter()
            .map(|h| FakeContent(to_network_point(&h.point()), h))
            .map(|content| {
                NextResponse::RollForward(
                    content,
                    Tip(
                        to_network_point(&tip_header.point()),
                        tip_header.block_height(),
                    ),
                )
            })
            .collect()
    }

    fn tip_of(responses: &[NextResponse<FakeContent>]) -> Tip {
        let origin_tip = Tip(NetworkPoint::Origin, 0);
        responses.last().map_or(origin_tip.clone(), |r| match r {
            NextResponse::RollForward(FakeContent(point, _header), _tip) => {
                Tip(point.clone(), responses.len() as u64)
            }
            NextResponse::RollBackward(..) | NextResponse::Await => origin_tip,
        })
    }

    fn point_of(response: &NextResponse<FakeContent>) -> NetworkPoint {
        match response {
            NextResponse::RollForward(h, _tip) => {
                h.point().expect("there should be a point in a FakeContent")
            }
            NextResponse::RollBackward(point, _tip) => point.clone(),
            NextResponse::Await => panic!("no point for await"),
        }
    }
}
