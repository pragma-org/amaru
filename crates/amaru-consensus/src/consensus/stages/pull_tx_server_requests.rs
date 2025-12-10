// Copyright 2024 PRAGMA
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

use crate::consensus::effects::ReceiveTxServerRequestEffect;
use amaru_ouroboros_traits::TxServerRequest;
use pure_stage::{Effects, StageRef};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// This stage continuously pulls tx submission server requests and
/// forwards them downstream for processing.
pub async fn stage(
    downstream: StageRef<TxServerRequest>,
    _msg: NextTxRequest,
    eff: Effects<NextTxRequest>,
) -> StageRef<TxServerRequest> {
    let span = tracing::trace_span!("tx_submission.tx_server_request.wait");
    if let Ok(mut msg) = eff
        .external(ReceiveTxServerRequestEffect)
        .instrument(span)
        .await
    {
        let span = tracing::trace_span!("tx_submission.tx_server_request");
        span.set_parent(msg.span().context()).ok();
        let entered = span.enter();

        msg.set_span(span.clone());

        drop(entered);
        async {
            eff.send(&downstream, msg).await;
            eff.send(eff.me_ref(), NextTxRequest).await;
        }
        .instrument(span)
        .await;
        downstream
    } else {
        eff.send(eff.me_ref(), NextTxRequest).await;
        downstream
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NextTxRequest;
