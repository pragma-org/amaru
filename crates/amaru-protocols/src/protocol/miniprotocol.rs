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
    mux::{HandlerMessage, MuxMessage},
    protocol::{Erased, NETWORK_SEND_TIMEOUT, ProtocolId},
};
use amaru_kernel::bytes::NonEmptyBytes;
use pure_stage::{Effects, SendData, StageRef, TryInStage, err};

/// An input to a miniprotocol handler stage.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Inputs<L> {
    Local(L),
    Network(HandlerMessage),
}

/// Outcome of a protocol step
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome<S, R> {
    pub send: Option<S>,
    pub result: Option<R>,
}

impl<S, D> Outcome<S, D> {
    pub fn send(self, send: S) -> Self {
        Self {
            send: Some(send),
            result: self.result,
        }
    }

    pub fn result(self, done: D) -> Self {
        Self {
            send: self.send,
            result: Some(done),
        }
    }
}

pub fn outcome<S, D>() -> Outcome<S, D> {
    Outcome {
        send: None,
        result: None,
    }
}

/// This tracks only the network protocol state, reacting to local decisions
/// (`Action`) or incoming network messages (`WireMsg`). It may emit information
/// via the `Out` type.
pub trait ProtocolState: Sized + SendData {
    type WireMsg: for<'de> minicbor::Decode<'de, ()> + minicbor::Encode<()>;
    type Action: std::fmt::Debug;
    type Out: std::fmt::Debug;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)>;
    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)>;
    fn local(&self, input: Self::Action) -> anyhow::Result<(Option<Self::WireMsg>, Self)>;
}

/// This tracks the stage state that is used to make decisions based on inputs
/// from the local node (`LocalIn`) or incoming network messages (`NetworkIn`).
/// It may emit network actions to be performed via the `Action` type.
#[expect(async_fn_in_trait)]
pub trait StageState<Proto: ProtocolState>: Sized + SendData {
    type LocalIn: SendData;

    async fn local<M>(
        self,
        proto: &Proto,
        input: Self::LocalIn,
        eff: &Effects<M>,
    ) -> anyhow::Result<(Self, Option<Proto::Action>)>;
    async fn network<M>(
        self,
        proto: &Proto,
        input: Proto::Out,
        eff: &Effects<M>,
    ) -> anyhow::Result<(Self, Option<Proto::Action>)>;
}

/// A miniprotocol is described using two states:
/// - `S`: the protocol state that tracks the network protocol state
/// - `S2`: the stage state that tracks the stage state
///
/// It is important to clearly separate these two, with `S2` being
/// responsible for decision making and `S` only following the protocol.
pub fn miniprotocol<Proto, Stage>(
    proto_id: ProtocolId<Erased>,
) -> impl AsyncFn(
    (Proto, Stage),
    Inputs<Stage::LocalIn>,
    Effects<Inputs<Stage::LocalIn>>,
) -> (Proto, Stage)
+ Send
+ 'static
where
    Proto: ProtocolState,
    Stage: AsRef<StageRef<MuxMessage>> + StageState<Proto>,
{
    enum LocalOrNetwork<L, A> {
        Local(L),
        Network(A),
        None,
    }

    async move |(mut proto, mut stage), input, eff| {
        // handle network input, if any
        let local_or_network = match input {
            Inputs::Network(wire_msg) => {
                let (result, msg) = if let HandlerMessage::FromNetwork(wire_msg) = wire_msg {
                    let wire_msg: Proto::WireMsg = minicbor::decode(&wire_msg)
                        .or_terminate(&eff, err("failed to decode message from network"))
                        .await;
                    (proto.network(wire_msg), "failed to step protocol state")
                } else {
                    (proto.init(), "failed to initialize protocol state")
                };
                let (outcome, s) = result.or_terminate(&eff, err(msg)).await;
                proto = s;
                if let Some(msg) = outcome.send {
                    let msg = NonEmptyBytes::encode(&msg);
                    eff.call(stage.as_ref(), NETWORK_SEND_TIMEOUT, move |cr| {
                        MuxMessage::Send(proto_id, msg.into(), cr)
                    })
                    .await;
                }
                outcome
                    .result
                    .map(LocalOrNetwork::Network)
                    .unwrap_or(LocalOrNetwork::None)
            }
            Inputs::Local(input) => LocalOrNetwork::Local(input),
        };

        // run decision making, if there was new information
        let action = match local_or_network {
            LocalOrNetwork::Local(local) => {
                let (s, action) = stage
                    .local(&proto, local, &eff)
                    .await
                    .or_terminate(&eff, err("failed to step stage state"))
                    .await;
                stage = s;
                action
            }
            LocalOrNetwork::Network(network) => {
                let (s, action) = stage
                    .network(&proto, network, &eff)
                    .await
                    .or_terminate(&eff, err("failed to step stage state"))
                    .await;
                stage = s;
                action
            }
            LocalOrNetwork::None => None,
        };

        // send network messages, if required
        if let Some(action) = action {
            let (outcome, s) = proto
                .local(action)
                .or_terminate(&eff, err("failed to step protocol state"))
                .await;
            proto = s;
            if let Some(msg) = outcome {
                let msg = NonEmptyBytes::encode(&msg);
                eff.call(stage.as_ref(), NETWORK_SEND_TIMEOUT, move |cr| {
                    MuxMessage::Send(proto_id, msg.into(), cr)
                })
                .await;
            }
        }

        (proto, stage)
    }
}
