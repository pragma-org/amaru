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

use crate::consensus::effects::{BaseOps, ConsensusOps, NetworkOps};
use crate::consensus::errors::{ProcessingFailed, ValidationFailed};
use crate::consensus::span::HasSpan;
use amaru_kernel::consensus_events::{ValidateBlockEvent, ValidateHeaderEvent};
use amaru_kernel::{IsHeader, RawBlock};
use pure_stage::StageRef;
use tracing::Instrument;

type State = (
    StageRef<ValidateBlockEvent>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

/// This stages fetches the full block from a peer after its header has been validated.
/// It then sends the full block to the downstream stage for validation and storage.
pub fn stage(
    (downstream, failures, errors): State,
    msg: ValidateHeaderEvent,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "diffusion.fetch_block");
    async move {
        match msg {
            ValidateHeaderEvent::Validated { peer, header, span } => {
                let point = header.point();
                let block = match eff.network().fetch_block(peer.clone(), point).await {
                    Ok(block) => block,
                    Err(e) => {
                        eff.base()
                            .send(&failures, ValidationFailed::new(&peer, e))
                            .await;
                        return (downstream, failures, errors);
                    }
                };

                let block = RawBlock::from(&*block);

                let result = eff.store().store_block(&header.hash(), &block);
                if let Err(e) = result {
                    eff.base()
                        .send(&errors, ProcessingFailed::new(&peer, e.into()))
                        .await;
                    return (downstream, failures, errors);
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
        (downstream, failures, errors)
    }
    .instrument(span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::effects::mock_consensus_ops;
    use crate::consensus::errors::ValidationFailed;
    use amaru_kernel::is_header::tests::{any_header, run};
    use amaru_kernel::peer::Peer;
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_block_that_can_be_fetched_is_sent_downstream() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run(any_header());
        let message = ValidateHeaderEvent::Validated {
            peer: peer.clone(),
            header: header.clone(),
            span: Span::current(),
        };
        let block = vec![1u8; 128];
        let consensus_ops = mock_consensus_ops();
        consensus_ops.mock_network.return_block(Ok(block.clone()));

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
        let downstream: StageRef<ValidateBlockEvent> = StageRef::named("downstream");
        let failures: StageRef<ValidationFailed> = StageRef::named("failures");
        let errors: StageRef<ProcessingFailed> = StageRef::named("errors");
        (downstream, failures, errors)
    }
}
