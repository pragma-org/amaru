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

use crate::{
    consensus::{errors::Errors, BlockFetchFailed, ValidateHeaderEvent},
    span::adopt_current_span,
    ConsensusError,
};
use amaru_kernel::{block::ValidateBlockEvent, peer::Peer, Point};
use amaru_ouroboros::IsHeader;
use pure_stage::{
    BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData, StageRef, Void,
};
use std::{collections::BTreeMap, future::ready};
use tracing::Instrument;

type State = (StageRef<ValidateBlockEvent, Void>, StageRef<Errors, Void>);

pub async fn stage(
    (downstream, errors): State,
    msg: ValidateHeaderEvent,
    eff: Effects<ValidateHeaderEvent, State>,
) -> State {
    let span = adopt_current_span(&msg);

    async move {
        match msg {
            ValidateHeaderEvent::Validated { peer, header, span } => {
                let point = header.point();
                let block = match eff
                    .external(FetchBlock::new(peer.clone(), point.clone()))
                    .await
                {
                    Ok(block) => block,
                    Err(error) => {
                        eff.send(
                            &errors,
                            Errors::BlockFetch(BlockFetchFailed::new(peer, point, error)),
                        )
                        .await;
                        return (downstream, errors);
                    }
                };
                eff.send(
                    &downstream,
                    ValidateBlockEvent::Validated { point, block, span },
                )
                .await;
            }
            ValidateHeaderEvent::Rollback {
                rollback_point,
                span,
                ..
            } => {
                eff.send(
                    &downstream,
                    ValidateBlockEvent::Rollback {
                        rollback_point,
                        span,
                    },
                )
                .await;
            }
        }
        (downstream, errors)
    }
    .instrument(span)
    .await
}

pub type FetchBlockFn = Box<
    dyn Fn(Point) -> BoxFuture<'static, Result<Vec<u8>, ConsensusError>> + Send + Sync + 'static,
>;

pub struct FetchBlockResource {
    pub sessions: BTreeMap<Peer, FetchBlockFn>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct FetchBlock {
    peer: Peer,
    point: Point,
}

impl FetchBlock {
    fn new(peer: Peer, point: Point) -> Self {
        Self { peer, point }
    }
}

impl ExternalEffect for FetchBlock {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let FetchBlock { peer, point } = *self;
        let resource = resources
            .get::<FetchBlockResource>()
            .expect("fetch block resource needed");
        let op = if let Some(fetch) = resource.sessions.get(&peer) {
            fetch(point)
        } else {
            Box::pin(ready(Err(ConsensusError::UnknownPeer(peer))))
        };
        Box::pin(async move {
            let res: <Self as ExternalEffectAPI>::Response = op.await;
            Box::new(res) as Box<dyn SendData>
        })
    }
}

impl ExternalEffectAPI for FetchBlock {
    type Response = Result<Vec<u8>, ConsensusError>;
}
