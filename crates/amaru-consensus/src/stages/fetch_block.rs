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
    effects::{BaseOps, ConsensusOps},
    errors::{ConsensusError, ProcessingFailed, ValidationFailed},
    events::{ValidateBlockEvent, ValidateHeaderEvent},
    span::HasSpan,
};
use amaru_kernel::cardano::network_block::NetworkBlock;
use amaru_kernel::{IsHeader, Peer, Point};
use amaru_protocols::manager::ManagerMessage;
use pure_stage::StageRef;
use std::time::Duration;
use tracing::Instrument;

type State = (
    StageRef<ValidateBlockEvent>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
    StageRef<ManagerMessage>,
);

/// This stages fetches the full block from a peer after its header has been validated.
/// It then sends the full block to the downstream stage for validation and storage.
pub fn stage(
    (downstream, failures, errors, manager): State,
    msg: ValidateHeaderEvent,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "diffusion.fetch_block");
    async move {
        match msg {
            ValidateHeaderEvent::Validated { peer, header, span } => {
                match load_or_fetch_block(&manager, &eff, header.point(), &peer).await {
                    Ok(Some(block)) => {
                        let validated = ValidateBlockEvent::Validated {
                            peer,
                            header,
                            block,
                            span,
                        };
                        eff.base().send(&downstream, validated).await;
                    }
                    Ok(None) => {
                        eff.base()
                            .send(
                                &failures,
                                ValidationFailed::new(
                                    &peer,
                                    ConsensusError::FetchBlockFailed(header.point()),
                                ),
                            )
                            .await;
                    }
                    Err(e) => {
                        eff.base()
                            .send(&errors, ProcessingFailed::new(&peer, e))
                            .await;
                    }
                }
            }
            ValidateHeaderEvent::Rollback {
                peer,
                rollback_point,
                span,
                ..
            } => {
                eff.base()
                    .send(
                        &downstream,
                        ValidateBlockEvent::Rollback {
                            peer,
                            rollback_point,
                            span,
                        },
                    )
                    .await
            }
        }
        (downstream, failures, errors, manager)
    }
    .instrument(span)
}

/// Check if we already downloaded a given block or fetch it from the peer.
async fn load_or_fetch_block(
    manager: &StageRef<ManagerMessage>,
    eff: &impl ConsensusOps,
    point: Point,
    peer: &Peer,
) -> anyhow::Result<Option<NetworkBlock>> {
    if let Some(block) = eff.store().load_block(&point.hash())? {
        tracing::trace!(%point, "block already in store, skipping fetch");
        Ok(Some(NetworkBlock::try_from(block)?))
    } else {
        fetch_block(manager, eff, point, peer).await
    }
}

/// Fetch a block from a given peer by calling the Manager and use the connection for that specific
/// peer.
async fn fetch_block(
    manager: &StageRef<ManagerMessage>,
    eff: &impl ConsensusOps,
    point: Point,
    peer: &Peer,
) -> anyhow::Result<Option<NetworkBlock>> {
    let peer_clone = peer.clone();
    let blocks = eff
        .base()
        // TODO(network): which timeout to use?
        .call(manager, Duration::from_secs(5), move |cr| {
            ManagerMessage::FetchBlocks {
                peer: peer_clone,
                from: point,
                through: point,
                cr,
            }
        })
        .await
        .unwrap_or_default();
    let Some(block) = blocks.blocks.into_iter().next() else {
        return Ok(None);
    };

    eff.store().store_block(&point.hash(), &block.raw_block())?;
    Ok(Some(block))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{effects::mock_consensus_ops, errors::ValidationFailed};
    use amaru_kernel::cardano::network_block::make_network_block;
    use amaru_kernel::{Peer, TESTNET_ERA_HISTORY, any_header, utils::tests::run_strategy};
    use amaru_protocols::blockfetch::Blocks;
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_block_that_can_be_fetched_is_sent_downstream() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run_strategy(any_header());
        let message = ValidateHeaderEvent::Validated {
            peer: peer.clone(),
            header: header.clone(),
            span: Span::current(),
        };
        let block = make_network_block(&header, &TESTNET_ERA_HISTORY);
        let consensus_ops = mock_consensus_ops();
        consensus_ops.mock_base.return_blocks(Blocks {
            blocks: vec![block.clone()],
        });

        stage(make_state(), message, consensus_ops.clone()).await;

        let forwarded = ValidateBlockEvent::Validated {
            peer: peer.clone(),
            header,
            block,
            span: Span::current(),
        };
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{forwarded:?}")]
            )])
        );
        Ok(())
    }

    // HELPERS

    fn make_state() -> State {
        let downstream: StageRef<ValidateBlockEvent> = StageRef::named_for_tests("downstream");
        let failures: StageRef<ValidationFailed> = StageRef::named_for_tests("failures");
        let errors: StageRef<ProcessingFailed> = StageRef::named_for_tests("errors");
        let manager: StageRef<ManagerMessage> = StageRef::named_for_tests("manager");
        (downstream, failures, errors, manager)
    }
}
