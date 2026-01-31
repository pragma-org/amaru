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

use crate::consensus::{
    effects::{BaseOps, ConsensusOps},
    errors::{ConsensusError, ProcessingFailed, ValidationFailed},
    events::{ValidateBlockEvent, ValidateHeaderEvent},
    span::HasSpan,
};
use amaru_kernel::{IsHeader, RawBlock};
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
                let point = header.point();
                let peer2 = peer.clone();
                let blocks = eff
                    .base()
                    // TODO(network): which timeout to use?
                    .call(&manager, Duration::from_secs(5), move |cr| {
                        ManagerMessage::FetchBlocks {
                            peer: peer2,
                            from: point,
                            through: point,
                            cr,
                        }
                    })
                    .await
                    .unwrap_or_default();
                let Some(block) = blocks.blocks.into_iter().next() else {
                    eff.base()
                        .send(
                            &failures,
                            ValidationFailed::new(&peer, ConsensusError::FetchBlockFailed(point)),
                        )
                        .await;
                    return (downstream, failures, errors, manager);
                };

                let block = RawBlock::from(&*block);

                let result = eff.store().store_block(&header.hash(), &block);
                if let Err(e) = result {
                    eff.base()
                        .send(&errors, ProcessingFailed::new(&peer, e.into()))
                        .await;
                    return (downstream, failures, errors, manager);
                }

                let validated = ValidateBlockEvent::Validated {
                    peer,
                    header,
                    block,
                    span,
                };
                eff.base().send(&downstream, validated).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::{effects::mock_consensus_ops, errors::ValidationFailed};
    use amaru_kernel::{Peer, any_header, utils::tests::run_strategy};
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
        let block = vec![1u8; 128];
        let consensus_ops = mock_consensus_ops();
        consensus_ops.mock_base.return_blocks(Blocks {
            blocks: vec![block.clone()],
        });

        stage(make_state(), message, consensus_ops.clone()).await;

        let forwarded = ValidateBlockEvent::Validated {
            peer: peer.clone(),
            header,
            block: RawBlock::from(block.as_slice()),
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
