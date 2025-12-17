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
    bytes::NonEmptyBytes,
    keepalive::messages::Message,
    mux::{HandlerMessage, MuxMessage},
    protocol::{NETWORK_SEND_TIMEOUT, PROTO_N2N_KEEP_ALIVE},
};
use pure_stage::{Effects, StageRef, TryInStage};
use std::time::Duration;

mod messages;
#[cfg(test)]
mod tests;

pub use messages::Cookie;

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct KeepAlive {
    muxer: StageRef<MuxMessage>,
    cookie: Cookie,
}

impl KeepAlive {
    pub fn new(muxer: StageRef<MuxMessage>, cookie: Cookie) -> Self {
        Self { muxer, cookie }
    }
}

pub async fn initiator(
    mut state: KeepAlive,
    msg: HandlerMessage,
    eff: Effects<HandlerMessage>,
) -> KeepAlive {
    match msg {
        HandlerMessage::Registered(_) => {}
        HandlerMessage::FromNetwork(non_empty_bytes) => {
            let msg: Message = minicbor::decode(&non_empty_bytes)
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to decode message from network");
                })
                .await;
            let Message::ResponseKeepAlive(cookie) = msg else {
                tracing::error!(?msg, "expected ResponseKeepAlive message");
                return eff.terminate().await;
            };
            if cookie != state.cookie {
                tracing::error!(?cookie, ?state, "cookie mismatch");
                return eff.terminate().await;
            }
            tracing::debug!(?cookie, "received ResponseKeepAlive message");

            eff.send(
                &state.muxer,
                MuxMessage::WantNext(PROTO_N2N_KEEP_ALIVE.erase()),
            )
            .await;

            // TODO keep track of timings and report up the tree
            state.cookie = state.cookie.next();
        }
    }
    eff.wait(Duration::from_secs(1)).await;
    eff.call(&state.muxer, NETWORK_SEND_TIMEOUT, |cr| {
        let msg = NonEmptyBytes::encode(&Message::KeepAlive(state.cookie));
        MuxMessage::Send(PROTO_N2N_KEEP_ALIVE.erase(), msg, cr)
    })
    .await;
    tracing::debug!(?state.cookie, "sending KeepAlive message");
    state
}

pub async fn responder(
    state: KeepAlive,
    msg: HandlerMessage,
    eff: Effects<HandlerMessage>,
) -> KeepAlive {
    match msg {
        HandlerMessage::Registered(_) => {}
        HandlerMessage::FromNetwork(non_empty_bytes) => {
            let msg: Message = minicbor::decode(&non_empty_bytes)
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to decode message from network");
                })
                .await;
            let Message::KeepAlive(cookie) = msg else {
                tracing::error!(?msg, "expected KeepAlive message");
                return eff.terminate().await;
            };
            eff.call(&state.muxer, NETWORK_SEND_TIMEOUT, |cr| {
                let msg = NonEmptyBytes::encode(&Message::ResponseKeepAlive(cookie));
                MuxMessage::Send(PROTO_N2N_KEEP_ALIVE.responder().erase(), msg, cr)
            })
            .await;
            eff.send(
                &state.muxer,
                MuxMessage::WantNext(PROTO_N2N_KEEP_ALIVE.responder().erase()),
            )
            .await;
        }
    }
    state
}
