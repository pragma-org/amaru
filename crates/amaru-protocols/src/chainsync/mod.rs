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

mod initiator;
mod messages;
mod responder;

pub use initiator::{ChainSyncInitiator, ChainSyncInitiatorMsg, InitiatorMessage, InitiatorResult, initiator};
pub use messages::HeaderContent;
pub use responder::{ChainSyncResponder, ResponderMessage, responder};

/// Number of RequestNext we keep in flight to not be limited by round-trip time.
/// This value has been obtained by testing between European countries and may therefore be too low for
/// catching up across continents; that might not be a smart use-case, though, which is why we use this
/// value for now.
pub const PIPELINE_DEPTH: u8 = 10;

/// Bulk mailbox of the chain-sync initiator.
///
/// Up to [`PIPELINE_DEPTH`] local `RequestNext`s may be admitted while one `call`
/// is in progress, plus a few network and control messages.
pub const CHAINSYNC_INITIATOR_MAILBOX: usize = PIPELINE_DEPTH as usize + 4;

pub fn register_deserializers() -> amaru_pure_stage::DeserializerGuards {
    vec![messages::register_deserializers(), initiator::register_deserializers(), responder::register_deserializers()]
        .into_iter()
        .flatten()
        .collect()
}

pub use register::{register_chainsync_initiator, register_chainsync_responder};

mod register {
    use amaru_kernel::{Peer, Point};
    use amaru_ouroboros::ConnectionId;
    use amaru_pure_stage::{Effects, StageRef};

    use super::*;
    use crate::{
        connection::ConnectionMessage,
        mux::{Frame, MuxMessage},
        protocol::{Inputs, PROTO_N2N_CHAIN_SYNC, ingress_limit},
    };

    pub async fn register_chainsync_initiator(
        muxer: &StageRef<MuxMessage>,
        peer: Peer,
        conn_id: ConnectionId,
        pipeline: StageRef<ChainSyncInitiatorMsg>,
        eff: &Effects<ConnectionMessage>,
        tombstone: ConnectionMessage,
    ) -> StageRef<InitiatorMessage> {
        let chainsync = eff.stage_with_mailbox_size("chainsync", initiator(), CHAINSYNC_INITIATOR_MAILBOX).await;
        let chainsync = eff.supervise(chainsync, tombstone);
        let chainsync = eff.wire_up(chainsync, ChainSyncInitiator::new(peer, conn_id, muxer.clone(), pipeline)).await;
        eff.send(
            muxer,
            MuxMessage::Register {
                protocol: PROTO_N2N_CHAIN_SYNC.erase(),
                frame: Frame::OneCborItem,
                handler: chainsync.contramap(Inputs::Network),
                max_buffer: ingress_limit(PROTO_N2N_CHAIN_SYNC),
            },
        )
        .await;
        chainsync.contramap(Inputs::Local)
    }

    pub async fn register_chainsync_responder(
        muxer: &StageRef<MuxMessage>,
        upstream: Point,
        peer: Peer,
        conn_id: ConnectionId,
        eff: &Effects<ConnectionMessage>,
        tombstone: ConnectionMessage,
    ) -> StageRef<ResponderMessage> {
        let chainsync = eff.stage("chainsync-responder", responder()).await;
        let chainsync = eff.supervise(chainsync, tombstone);
        let chainsync = eff.wire_up(chainsync, ChainSyncResponder::new(upstream, peer, conn_id, muxer.clone())).await;
        eff.send(
            muxer,
            MuxMessage::Register {
                protocol: PROTO_N2N_CHAIN_SYNC.responder().erase(),
                frame: Frame::OneCborItem,
                handler: chainsync.contramap(Inputs::Network),
                max_buffer: ingress_limit(PROTO_N2N_CHAIN_SYNC.responder()),
            },
        )
        .await;
        chainsync.contramap(Inputs::Local)
    }
}

#[cfg(test)]
mod mailbox {
    use std::sync::OnceLock;

    use amaru_kernel::{Peer, Point};
    use amaru_ouroboros::ConnectionId;
    use amaru_pure_stage::{
        Effect, StageGraph,
        simulation::{Run, SimulationBuilder},
    };
    use tokio::runtime::{Builder, Runtime};

    use super::{
        CHAINSYNC_INITIATOR_MAILBOX, ChainSyncInitiatorMsg, PIPELINE_DEPTH, register_chainsync_initiator,
        register_chainsync_responder,
    };
    use crate::{connection::ConnectionMessage, mux::MuxMessage};

    fn test_runtime() -> &'static tokio::runtime::Handle {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| Builder::new_multi_thread().enable_all().build().unwrap()).handle()
    }

    fn trace_guards() -> amaru_pure_stage::DeserializerGuards {
        crate::deserializers::register_deserializers()
    }

    /// `stage_name` appends `-{n}`. `chainsync-1` matches `"chainsync"`; `chainsync-responder-2` does not.
    fn is_numbered_stage(name: &str, prefix: &str) -> bool {
        name.strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
    }

    #[test]
    fn chainsync_initiator_mailbox_is_pipeline_depth_plus_4() {
        let _guards = trace_guards();
        assert_eq!(PIPELINE_DEPTH, 10);
        assert_eq!(CHAINSYNC_INITIATOR_MAILBOX, 14);
        assert_eq!(CHAINSYNC_INITIATOR_MAILBOX, usize::from(PIPELINE_DEPTH) + 4);

        let mut network = SimulationBuilder::default();
        let boot = network.stage("boot", async |(): (), _: ConnectionMessage, eff| {
            let mux = eff.stage("mux", async |s: (), _: MuxMessage, _eff| s).await;
            let mux = eff.wire_up(mux, ()).await;
            let pipeline = eff.stage("pipeline", async |s: (), _: ChainSyncInitiatorMsg, _eff| s).await;
            let pipeline = eff.wire_up(pipeline, ()).await;
            let _initiator = register_chainsync_initiator(
                &mux,
                Peer::for_test(1),
                ConnectionId::initial(),
                pipeline,
                &eff,
                ConnectionMessage::Disconnect,
            )
            .await;
        });
        let boot = network.wire_up(boot, ());
        let mut running = network.run(test_runtime());
        running.breakpoint(
            "cs-mail",
            |eff| matches!(eff, Effect::WireStage { name, .. } if is_numbered_stage(name.as_str(), "chainsync")),
        );
        running.enqueue_msg(&boot, [ConnectionMessage::Disconnect]);
        running.run(Run::default()).assert_breakpoint("cs-mail");
        let hit = running.breakpoint_effect();
        let Effect::WireStage { mailbox_size, .. } = hit.effect() else {
            panic!("expected the chain-sync initiator to be wired");
        };
        assert_eq!(*mailbox_size, 14);
        assert_eq!(*mailbox_size, CHAINSYNC_INITIATOR_MAILBOX);
    }

    #[test]
    fn chainsync_responder_keeps_the_default_mailbox() {
        let _guards = trace_guards();
        let mut network = SimulationBuilder::default();
        let boot = network.stage("boot", async |(): (), _: ConnectionMessage, eff| {
            let mux = eff.stage("mux", async |s: (), _: MuxMessage, _eff| s).await;
            let mux = eff.wire_up(mux, ()).await;
            let _responder = register_chainsync_responder(
                &mux,
                Point::Origin,
                Peer::for_test(1),
                ConnectionId::initial(),
                &eff,
                ConnectionMessage::Disconnect,
            )
            .await;
        });
        let boot = network.wire_up(boot, ());
        let mut running = network.run(test_runtime());
        running.breakpoint("cs-mail", |eff| {
            matches!(eff, Effect::WireStage { name, .. } if is_numbered_stage(name.as_str(), "chainsync-responder"))
        });
        running.enqueue_msg(&boot, [ConnectionMessage::Disconnect]);
        running.run(Run::default()).assert_breakpoint("cs-mail");
        let hit = running.breakpoint_effect();
        let Effect::WireStage { mailbox_size, .. } = hit.effect() else {
            panic!("expected the chain-sync responder to be wired");
        };
        assert_eq!(*mailbox_size, 10);
        assert_eq!(*mailbox_size, amaru_pure_stage::DEFAULT_MAILBOX_SIZE);
    }
}
